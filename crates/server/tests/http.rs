#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, State};
use axum::http::Request;
use axum::routing::{get, post};
use mempool_based::{App, Config, IMMUTABLE, Limits, NetworkContext, SECURITY_HEADERS};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tower::ServiceExt;

const SECRET: &str = "outside-the-web-dir";
const PANIC_MESSAGE: &str = "probe panic with internal detail";

/// A temp dir holding `secret.txt` beside a `web/` dir with an app shell, one asset and fake
/// precompressed variants whose bodies say which file was served.
fn fixture() -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let root = std::env::temp_dir().join(format!(
        "mempool-based-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(root.join("web/assets")).unwrap();
    std::fs::write(root.join("secret.txt"), SECRET).unwrap();
    std::fs::write(root.join("web/index.html"), "<!doctype html>app-shell").unwrap();
    std::fs::write(root.join("web/index.html.br"), "shell-br").unwrap();
    std::fs::write(root.join("web/assets/app.js"), "console.log(1)").unwrap();
    std::fs::write(root.join("web/assets/app.js.br"), "app-br").unwrap();
    std::fs::write(root.join("web/assets/app.js.gz"), "app-gz").unwrap();
    root.join("web")
}

/// Mainnet, signet and testnet4 enabled; regtest disabled. Nothing listens on the endpoints.
fn config(limits: Limits) -> Config {
    let mut vars = std::collections::HashMap::new();
    for net in ["MAINNET", "SIGNET", "TESTNET4"] {
        for (k, v) in [
            ("RPC_HOST", "127.0.0.1"),
            ("RPC_PORT", "1"),
            ("RPC_USER", "u"),
            ("RPC_PASSWORD", "probe-password"),
            ("ESPLORA_URL", "http://127.0.0.1:1"),
            ("ELECTRUM_ADDR", "127.0.0.1:1"),
        ] {
            vars.insert(format!("{net}_{k}"), v.to_owned());
        }
    }
    let mut c = Config::from_vars(|k| vars.get(k).cloned()).unwrap();
    c.web_dir = fixture();
    c.limits = limits;
    c
}

/// Probe handlers mounted on every enabled network.
fn probes() -> Router<Arc<NetworkContext>> {
    Router::new()
        .route(
            "/api/net",
            get(|State(ctx): State<Arc<NetworkContext>>| async move { ctx.config.network.name() }),
        )
        .route(
            "/api/echo",
            post(|body: Bytes| async move { body.len().to_string() }),
        )
        .route(
            "/api/json",
            post(|axum::Json(v): axum::Json<serde_json::Value>| async move { v.to_string() }),
        )
        .route(
            "/api/slow",
            get(|| async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                "done"
            }),
        )
        .route(
            "/api/panic",
            get(|| async {
                if PANIC_MESSAGE.is_empty() {
                    "unreachable"
                } else {
                    panic!("{PANIC_MESSAGE}")
                }
            }),
        )
}

fn limits() -> Limits {
    Limits {
        client_burst: 1000,
        client_rate_per_sec: 1000,
        ..Limits::default()
    }
}

/// Starts the server on port 0 and returns its address.
async fn start(limits: Limits) -> String {
    start_with(config(limits)).await
}

async fn start_with(config: Config) -> String {
    let app = App::with_routes(&config, probes());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(app.serve(listener, std::future::pending()));
    addr
}

#[derive(Debug)]
struct Reply {
    status: u16,
    head: String,
    body: String,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.head
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{name}: ")))
    }
}

/// Sends `head` (request line and headers, no blank line) and `body` exactly as written.
async fn send(addr: &str, head: &str, body: &[u8]) -> Reply {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let req = format!("{head}\r\nHost: test\r\nConnection: close\r\n\r\n");
    let mut bytes = req.into_bytes();
    bytes.extend_from_slice(body);
    // The server may answer and close before reading an oversized body.
    let _ = stream.write_all(&bytes).await;
    let mut raw = Vec::new();
    let _ = stream.read_to_end(&mut raw).await;
    let raw = String::from_utf8_lossy(&raw);
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    Reply {
        status: head[9..12].parse().unwrap(),
        head: head.to_ascii_lowercase(),
        body: body.to_owned(),
    }
}

async fn get_path(addr: &str, target: &str) -> Reply {
    send(addr, &format!("GET {target} HTTP/1.1"), b"").await
}

fn assert_json_error(r: &Reply, status: u16, message: &str) {
    assert_eq!(r.status, status, "{r:?}");
    assert!(r.head.contains("content-type: application/json"), "{r:?}");
    let json: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(json, serde_json::json!({ "error": message }), "{r:?}");
}

#[tokio::test]
async fn healthz_is_ok_json() {
    let addr = start(limits()).await;
    let r = get_path(&addr, "/healthz").await;
    assert_eq!(r.status, 200);
    assert!(r.head.contains("content-type: application/json"));
    let json: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(json["status"], "ok");
}

#[tokio::test]
async fn app_config_lists_enabled_networks_at_the_root_only() {
    let addr = start(limits()).await;
    let r = get_path(&addr, "/app/config").await;
    assert_eq!(r.status, 200, "{r:?}");
    assert!(r.head.contains("content-type: application/json"));
    let json: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(
        json["networks"],
        serde_json::json!(["mainnet", "signet", "testnet4"])
    );
    assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
    assert_json_error(
        &get_path(&addr, "/signet/app/config").await,
        404,
        "not found",
    );

    // Still served when mainnet, which owns the root, is disabled.
    let mut c = config(limits());
    c.networks.retain(|n| n.network.name() == "signet");
    let addr = start_with(c).await;
    let json: serde_json::Value =
        serde_json::from_str(&get_path(&addr, "/app/config").await.body).unwrap();
    assert_eq!(json["networks"], serde_json::json!(["signet"]));
    assert_json_error(&get_path(&addr, "/app/other").await, 404, "not found");
}

#[tokio::test]
async fn each_enabled_network_gets_its_own_context() {
    let addr = start(limits()).await;
    for (path, name) in [
        ("/api/net", "mainnet"),
        ("/signet/api/net", "signet"),
        ("/testnet4/api/net", "testnet4"),
    ] {
        let r = get_path(&addr, path).await;
        assert_eq!(r.status, 200, "{path}");
        assert_eq!(r.body, name, "{path}");
    }
}

#[tokio::test]
async fn disabled_and_unknown_networks_are_404() {
    let addr = start(limits()).await;
    for path in [
        "/regtest",
        "/regtest/",
        "/regtest/tx/abc",
        "/regtest/api/net",
        "/liquid/api/net",
        "/testnet/app/",
    ] {
        assert_json_error(&get_path(&addr, path).await, 404, "not found");
    }
}

#[tokio::test]
async fn api_paths_do_not_fall_back_to_the_app_shell() {
    let addr = start(limits()).await;
    for prefix in ["", "/signet", "/testnet4"] {
        for path in [
            "/api",
            "/api/",
            "/api/no/such/route",
            "/app",
            "/app/",
            "/app/x",
        ] {
            let r = get_path(&addr, &format!("{prefix}{path}")).await;
            assert_json_error(&r, 404, "not found");
        }
    }
}

#[tokio::test]
async fn pages_fall_back_to_the_app_shell() {
    let addr = start(limits()).await;
    let asset = get_path(&addr, "/assets/app.js").await;
    assert_eq!(asset.status, 200);
    assert_eq!(asset.body, "console.log(1)");
    for path in [
        "/",
        "/block/123",
        "/signet",
        "/signet/",
        "/signet/tx/abc",
        "/testnet4/",
    ] {
        let r = get_path(&addr, path).await;
        assert_eq!(r.status, 200, "{path}");
        assert!(r.head.contains("content-type: text/html"), "{path}");
        assert_eq!(r.body, "<!doctype html>app-shell", "{path}");
    }
}

#[tokio::test]
async fn precompressed_variants_and_cache_headers() {
    let addr = start(limits()).await;
    let asset = |enc: &'static str| {
        let addr = addr.clone();
        async move {
            send(
                &addr,
                &format!("GET /assets/app.js HTTP/1.1\r\nAccept-Encoding: {enc}"),
                b"",
            )
            .await
        }
    };
    let br = asset("gzip, br").await;
    assert_eq!(br.body, "app-br");
    assert_eq!(br.header("content-encoding"), Some("br"));
    assert_eq!(br.header("cache-control"), Some(IMMUTABLE));
    assert!(br.header("vary").unwrap().contains("accept-encoding"));
    let gz = asset("gzip").await;
    assert_eq!(gz.body, "app-gz");
    assert_eq!(gz.header("content-encoding"), Some("gzip"));
    let plain = asset("identity").await;
    assert_eq!(plain.body, "console.log(1)");
    assert_eq!(plain.header("content-encoding"), None);
    assert_eq!(plain.header("cache-control"), Some(IMMUTABLE));

    for path in ["/", "/block/1", "/signet/"] {
        let r = send(
            &addr,
            &format!("GET {path} HTTP/1.1\r\nAccept-Encoding: br"),
            b"",
        )
        .await;
        assert_eq!(r.body, "shell-br", "{path}");
        assert_eq!(r.header("content-encoding"), Some("br"), "{path}");
        assert_eq!(r.header("cache-control"), Some("no-cache"), "{path}");
    }

    // A missing asset must not be cached forever, nor be answered with the shell.
    let missing = get_path(&addr, "/assets/missing.js").await;
    assert_json_error(&missing, 404, "not found");
    assert_eq!(missing.header("cache-control"), Some("no-store"));
}

#[tokio::test]
async fn oversized_body_is_refused() {
    let addr = start(Limits {
        max_body_bytes: 1024,
        ..limits()
    })
    .await;
    let ok = send(
        &addr,
        "POST /api/echo HTTP/1.1\r\nContent-Length: 1024",
        &[b'a'; 1024],
    )
    .await;
    assert_eq!(ok.body, "1024");
    let big = send(
        &addr,
        "POST /api/echo HTTP/1.1\r\nContent-Length: 4096",
        &[b'a'; 4096],
    )
    .await;
    assert_json_error(&big, 413, "body too large");
    // Chunked, so there is no Content-Length to check up front.
    let chunk = format!("{:x}\r\n{}\r\n0\r\n\r\n", 4096, "a".repeat(4096));
    let chunked = send(
        &addr,
        "POST /signet/api/echo HTTP/1.1\r\nTransfer-Encoding: chunked",
        chunk.as_bytes(),
    )
    .await;
    assert_json_error(&chunked, 413, "body too large");
}

#[tokio::test]
async fn slow_request_times_out() {
    let addr = start(Limits {
        request_timeout: Duration::from_millis(200),
        ..limits()
    })
    .await;
    let t = Instant::now();
    assert_json_error(
        &get_path(&addr, "/api/slow").await,
        504,
        "request timed out",
    );
    assert!(t.elapsed() < Duration::from_millis(900));
}

#[tokio::test]
async fn slow_headers_and_idle_connections_are_closed() {
    let addr = start(Limits {
        request_timeout: Duration::from_millis(200),
        ..limits()
    })
    .await;
    let mut stream = TcpStream::connect(&addr).await.unwrap();
    stream
        .write_all(b"GET /healthz HTTP/1.1\r\nHost:")
        .await
        .unwrap();
    let mut buf = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut buf)).await;
    assert!(read.is_ok(), "server kept a half-sent request open");

    let mut idle = TcpStream::connect(&addr).await.unwrap();
    let read = tokio::time::timeout(Duration::from_secs(2), idle.read_to_end(&mut buf)).await;
    assert!(read.is_ok(), "server kept an idle connection open");
}

#[tokio::test]
async fn a_client_that_stops_reading_loses_its_connection() {
    let config = config(Limits {
        max_connections: 1,
        request_timeout: Duration::from_millis(300),
        ..limits()
    });
    std::fs::write(config.web_dir.join("assets/big.bin"), vec![0u8; 1 << 20]).unwrap();
    let addr = start_with(config).await;
    // Pipeline far more than the socket buffers hold, and never read.
    let mut stalled = TcpStream::connect(&addr).await.unwrap();
    let req = "GET /assets/big.bin HTTP/1.1\r\nHost: t\r\n\r\n".repeat(32);
    stalled.write_all(req.as_bytes()).await.unwrap();
    let t = Instant::now();
    let r = tokio::time::timeout(Duration::from_secs(5), get_path(&addr, "/healthz")).await;
    assert_eq!(
        r.expect("the stalled client kept the only slot").status,
        200
    );
    assert!(t.elapsed() < Duration::from_secs(3));
    drop(stalled);
}

#[tokio::test]
async fn connections_per_client_are_capped() {
    let addr = start(Limits {
        client_connections: 2,
        ..limits()
    })
    .await;
    let mut held = Vec::new();
    for _ in 0..2 {
        held.push(TcpStream::connect(&addr).await.unwrap());
    }
    // Give the accept loop time to take both slots.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut third = TcpStream::connect(&addr).await.unwrap();
    let _ = third
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: t\r\n\r\n")
        .await;
    let mut buf = Vec::new();
    let _ = third.read_to_end(&mut buf).await;
    assert!(buf.is_empty(), "third connection was served");
    drop(held);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(get_path(&addr, "/healthz").await.status, 200);
}

/// Drives the router directly, so requests can come from distinct peer addresses.
async fn from(app: &App, peer: &str, path: &str) -> u16 {
    let peer: SocketAddr = peer.parse().unwrap();
    let req = Request::get(path)
        .extension(ConnectInfo(peer))
        .body(Body::empty())
        .unwrap();
    app.router().oneshot(req).await.unwrap().status().as_u16()
}

#[tokio::test]
async fn a_client_over_its_rate_is_refused_while_others_are_served() {
    let app = App::with_routes(
        &config(Limits {
            client_rate_per_sec: 1,
            client_burst: 2,
            ..Limits::default()
        }),
        probes(),
    );
    let a = "192.0.2.1:1000";
    assert_eq!(from(&app, a, "/healthz").await, 200);
    assert_eq!(from(&app, a, "/api/net").await, 200);
    assert_eq!(from(&app, a, "/").await, 429);
    // Another port is the same client; another /64 is not.
    assert_eq!(from(&app, "192.0.2.1:2000", "/healthz").await, 429);
    assert_eq!(from(&app, "192.0.2.2:1000", "/healthz").await, 200);
    assert_eq!(from(&app, "[2001:db8::1]:1", "/healthz").await, 200);
    assert_eq!(from(&app, "[2001:db8::2]:1", "/healthz").await, 200);
    assert_eq!(from(&app, "[2001:db8::3]:1", "/healthz").await, 429);

    // And end to end over a socket, with the error body and Retry-After.
    let addr = start(Limits {
        client_rate_per_sec: 1,
        client_burst: 1,
        ..Limits::default()
    })
    .await;
    assert_eq!(get_path(&addr, "/healthz").await.status, 200);
    let r = get_path(&addr, "/healthz").await;
    assert_json_error(&r, 429, "too many requests");
    assert_eq!(r.header("retry-after"), Some("1"));
}

#[tokio::test]
async fn proxy_header_is_honoured_only_when_trusted() {
    let tight = Limits {
        client_rate_per_sec: 1,
        client_burst: 1,
        ..Limits::default()
    };
    let as_client = |addr: String, ip: &'static str| async move {
        send(
            &addr,
            &format!("GET /healthz HTTP/1.1\r\nX-Real-IP: {ip}"),
            b"",
        )
        .await
        .status
    };

    let untrusted = start(tight.clone()).await;
    assert_eq!(as_client(untrusted.clone(), "192.0.2.1").await, 200);
    assert_eq!(as_client(untrusted, "192.0.2.2").await, 429);

    let trusted = start(Limits {
        trusted_proxy_header: Some("x-real-ip".parse().unwrap()),
        ..tight
    })
    .await;
    assert_eq!(as_client(trusted.clone(), "192.0.2.1").await, 200);
    assert_eq!(as_client(trusted.clone(), "192.0.2.2").await, 200);
    assert_eq!(as_client(trusted, "192.0.2.1").await, 429);
}

#[tokio::test]
async fn concurrency_is_capped_per_client_and_globally() {
    let app = App::with_routes(
        &config(Limits {
            client_requests: 1,
            max_requests: 2,
            ..limits()
        }),
        probes(),
    );
    let slow = |peer: &'static str| {
        let app = app.clone();
        tokio::spawn(async move { from(&app, peer, "/api/slow").await })
    };
    let first = slow("192.0.2.1:1");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(from(&app, "192.0.2.1:1", "/healthz").await, 429);
    let second = slow("192.0.2.2:1");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(from(&app, "192.0.2.3:1", "/healthz").await, 503);
    assert_eq!(first.await.unwrap(), 200);
    assert_eq!(second.await.unwrap(), 200);
    assert_eq!(from(&app, "192.0.2.3:1", "/healthz").await, 200);
}

#[tokio::test]
async fn error_bodies_leak_nothing() {
    let addr = start(limits()).await;
    let web_dir = fixture();
    let replies = [
        (get_path(&addr, "/api/panic").await, 500, "internal error"),
        (
            send(
                &addr,
                "POST /api/json HTTP/1.1\r\nContent-Type: application/json\r\nContent-Length: 9",
                b"{\"secret\"",
            )
            .await,
            400,
            "bad request",
        ),
        (
            send(&addr, "POST /api/json HTTP/1.1\r\nContent-Length: 2", b"{}").await,
            415,
            "bad request",
        ),
        (
            send(&addr, "POST /healthz HTTP/1.1", b"").await,
            405,
            "method not allowed",
        ),
        (
            send(&addr, "DELETE / HTTP/1.1", b"").await,
            405,
            "method not allowed",
        ),
        (get_path(&addr, "/assets/nope.js").await, 404, "not found"),
    ];
    for (r, status, message) in &replies {
        assert_json_error(r, *status, message);
        for leak in [
            PANIC_MESSAGE,
            "secret",
            "serde",
            "probe-password",
            "127.0.0.1",
        ] {
            assert!(!r.body.contains(leak), "{leak} in {r:?}");
        }
        assert!(!r.body.contains(web_dir.parent().unwrap().to_str().unwrap()));
        for (name, _) in &SECURITY_HEADERS {
            assert!(r.head.contains(name.as_str()), "{r:?}");
        }
    }
    // The server is still up after the panic.
    assert_eq!(get_path(&addr, "/healthz").await.status, 200);
}

#[tokio::test]
async fn security_headers_on_every_response() {
    let addr = start(limits()).await;
    for path in [
        "/healthz",
        "/",
        "/assets/app.js",
        "/no/such/page",
        "/api/nope",
    ] {
        let r = get_path(&addr, path).await;
        for (name, value) in &SECURITY_HEADERS {
            let line = format!("{}: {}", name.as_str(), value.to_ascii_lowercase());
            assert!(r.head.contains(&line), "{path} lacks {line}");
        }
    }
    let csp = SECURITY_HEADERS[0].1;
    assert!(csp.starts_with("default-src 'self';"));
    assert!(!csp.contains("unsafe"));
}

#[tokio::test]
async fn path_traversal_is_refused() {
    let addr = start(limits()).await;
    for path in [
        "/../secret.txt",
        "/assets/../../secret.txt",
        "/%2e%2e/secret.txt",
        "/..%2fsecret.txt",
        "/assets/%2e%2e%2f%2e%2e%2fsecret.txt",
        "/..%5csecret.txt",
    ] {
        let r = get_path(&addr, path).await;
        assert!(
            !r.body.contains(SECRET),
            "{path} leaked a file outside the web dir"
        );
    }
}
