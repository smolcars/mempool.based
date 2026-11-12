#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use mempool_based::{SECURITY_HEADERS, router};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const SECRET: &str = "outside-the-web-dir";

/// A temp dir holding `secret.txt` beside a `web/` dir with an app shell and one asset.
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
    std::fs::write(root.join("web/assets/app.js"), "console.log(1)").unwrap();
    root
}

/// Starts the server on port 0 and returns its address.
async fn start() -> String {
    let web = fixture().join("web");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move { axum::serve(listener, router(&web)).await.unwrap() });
    addr
}

struct Reply {
    status: u16,
    head: String,
    body: String,
}

/// Sends the request target as written, so traversal paths reach the server unnormalized.
async fn get(addr: &str, target: &str) -> Reply {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let req = format!("GET {target} HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).await.unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    let status = head[9..12].parse().unwrap();
    Reply {
        status,
        head: head.to_ascii_lowercase(),
        body: body.to_owned(),
    }
}

#[tokio::test]
async fn healthz_is_ok_json() {
    let addr = start().await;
    let r = get(&addr, "/healthz").await;
    assert_eq!(r.status, 200);
    assert!(r.head.contains("content-type: application/json"));
    let json: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(json["status"], "ok");
}

#[tokio::test]
async fn serves_assets_and_falls_back_to_the_app_shell() {
    let addr = start().await;
    let asset = get(&addr, "/assets/app.js").await;
    assert_eq!(asset.status, 200);
    assert_eq!(asset.body, "console.log(1)");

    for path in ["/", "/block/123", "/signet/tx/abc"] {
        let r = get(&addr, path).await;
        assert_eq!(r.status, 200, "{path}");
        assert!(r.head.contains("content-type: text/html"), "{path}");
        assert_eq!(r.body, "<!doctype html>app-shell", "{path}");
    }
}

#[tokio::test]
async fn api_paths_do_not_fall_back_to_the_app_shell() {
    let addr = start().await;
    for path in [
        "/api",
        "/api/",
        "/api/blocks/tip/height",
        "/app",
        "/app/",
        "/app/anything",
    ] {
        let r = get(&addr, path).await;
        assert_eq!(r.status, 404, "{path}");
        assert!(!r.body.contains("app-shell"), "{path}");
    }
}

#[tokio::test]
async fn security_headers_on_every_response() {
    let addr = start().await;
    for path in [
        "/healthz",
        "/",
        "/assets/app.js",
        "/no/such/page",
        "/api/nope",
    ] {
        let r = get(&addr, path).await;
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
    let addr = start().await;
    for path in [
        "/../secret.txt",
        "/assets/../../secret.txt",
        "/%2e%2e/secret.txt",
        "/..%2fsecret.txt",
        "/assets/%2e%2e%2f%2e%2e%2fsecret.txt",
        "/..%5csecret.txt",
    ] {
        let r = get(&addr, path).await;
        assert!(
            !r.body.contains(SECRET),
            "{path} leaked a file outside the web dir"
        );
    }
}
