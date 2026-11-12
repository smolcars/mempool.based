//! The request log and the Electrum reporter, against a real server on port 0. The logger is
//! global, so every test here shares one capture and finds its own lines by request id.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::Path;
use axum::routing::get;
use logging::{Format, Level, Settings};
use rbitcoin_space::config::{Network, RequestLog};
use rbitcoin_space::{App, Config, Limits, NetworkContext};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn capture() -> &'static Capture {
    static CAPTURE: OnceLock<Capture> = OnceLock::new();
    CAPTURE.get_or_init(|| {
        let c = Capture(Arc::default());
        let settings = Settings {
            format: Format::Json,
            level: Level::Debug,
        };
        assert!(logging::init(settings, c.clone()));
        c
    })
}

/// Every line written so far, each checked to be one JSON object.
fn logged() -> (String, Vec<Value>) {
    let c = capture();
    logging::flush(Duration::from_secs(5));
    let raw = String::from_utf8(c.0.lock().unwrap().clone()).unwrap();
    let lines = raw
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    (raw, lines)
}

fn events_for(id: &str) -> Vec<Value> {
    logged()
        .1
        .into_iter()
        .filter(|v| v["request_id"] == id)
        .collect()
}

fn config(vars: &[(&str, &str)], limits: Limits) -> Config {
    let mut map: HashMap<String, String> = HashMap::new();
    for net in ["MAINNET", "SIGNET"] {
        for (k, v) in [
            ("RPC_HOST", "127.0.0.1"),
            ("RPC_PORT", "1"),
            ("RPC_USER", "u"),
            ("RPC_PASSWORD", "probe-password"),
            ("ESPLORA_URL", "http://127.0.0.1:1"),
            ("ELECTRUM_ADDR", "127.0.0.1:1"),
        ] {
            map.insert(format!("{net}_{k}"), v.to_owned());
        }
    }
    for (k, v) in vars {
        map.insert((*k).to_owned(), (*v).to_owned());
    }
    let mut c = Config::from_vars(|k| map.get(k).cloned()).unwrap();
    c.web_dir = std::env::temp_dir().join("rbitcoin-space-no-web-dir");
    c.limits = limits;
    c
}

fn roomy() -> Limits {
    Limits {
        client_burst: 1000,
        client_rate_per_sec: 1000,
        ..Limits::default()
    }
}

async fn start(config: Config) -> String {
    capture();
    let probes: Router<Arc<NetworkContext>> = Router::new().route(
        "/app/lookup/{id}",
        get(|Path(id): Path<String>| async move { id }),
    );
    let app = App::with_routes(&config, probes);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(app.serve(listener, std::future::pending()));
    addr
}

/// GETs `target` and returns the status and the `X-Request-Id` header.
async fn get_id(addr: &str, target: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let req = format!("GET {target} HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.unwrap();
    let raw = String::from_utf8_lossy(&raw).to_ascii_lowercase();
    let status = raw[9..12].parse().unwrap();
    let id = raw
        .lines()
        .find_map(|l| l.strip_prefix("x-request-id: "))
        .expect("every response has a request id")
        .trim()
        .to_owned();
    (status, id)
}

const LOOKED_UP: &str = "d00dfeedd00dfeedd00dfeedd00dfeedd00dfeedd00dfeedd00dfeedd00dfeed";

#[tokio::test]
async fn one_event_per_request_with_the_template_and_never_the_path() {
    let addr = start(config(&[], roomy())).await;
    let (status, id) = get_id(
        &addr,
        &format!("/signet/app/lookup/{LOOKED_UP}?secret-query=1"),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(id.len(), 16);
    let events = events_for(&id);
    assert_eq!(events.len(), 1, "{events:?}");
    let e = &events[0];
    assert_eq!(e["event"], "http_request");
    assert_eq!(e["level"], "info");
    assert_eq!(e["network"], "signet");
    assert_eq!(e["method"], "GET");
    assert_eq!(e["route"], "/signet/app/lookup/{id}");
    assert_eq!(e["status"], 200);
    assert_eq!(e["bytes"], 64);
    assert!(e["duration_ms"].as_f64().unwrap() >= 0.0);
    assert!(e.get("client").is_none(), "no client address by default");

    let (raw, _) = logged();
    assert!(!raw.contains(LOOKED_UP), "the looked-up id reached the log");
    assert!(!raw.contains("secret-query"));
}

#[tokio::test]
async fn frontend_and_unknown_paths_are_labelled_not_echoed() {
    let addr = start(config(&[], roomy())).await;
    let (status, id) = get_id(&addr, "/address/bc1qhiddenaddressxyz").await;
    assert_eq!(status, 404, "no web dir in this test");
    let e = &events_for(&id)[0];
    assert_eq!(e["route"], "frontend");
    assert_eq!(e["network"], "mainnet");
    let (raw, _) = logged();
    assert!(!raw.contains("bc1qhiddenaddressxyz"));
}

#[tokio::test]
async fn a_refused_request_says_why() {
    let limits = Limits {
        client_burst: 1,
        client_rate_per_sec: 1,
        ..Limits::default()
    };
    // Each App has its own limiter, so other tests' requests do not share this bucket.
    let addr = start(config(&[], limits)).await;
    let (first, _) = get_id(&addr, "/signet/app/lookup/a").await;
    let (second, id) = get_id(&addr, "/signet/app/lookup/b").await;
    assert_eq!((first, second), (200, 429));
    let e = &events_for(&id)[0];
    assert_eq!(e["status"], 429);
    assert_eq!(e["refused"], "client_rate");
    assert_eq!(e["route"], "/signet/app/lookup/{id}");
}

#[tokio::test]
async fn api_requests_carry_the_pass_through_outcome() {
    let addr = start(config(&[], roomy())).await;
    let (status, id) = get_id(&addr, "/signet/api/blocks/tip/height").await;
    assert!(status >= 500, "nothing listens upstream: {status}");
    let e = &events_for(&id)[0];
    assert_eq!(e["route"], "/signet/api/{*rest}");
    assert_eq!(e["endpoint"], "tip_height");
    assert_eq!(e["upstream_error"], "unavailable");
    assert!(e["source"].is_string());
}

#[tokio::test]
async fn volume_setting_and_quiet_health_checks() {
    let all = start(config(&[], roomy())).await;
    let (status, id) = get_id(&all, "/healthz").await;
    assert_eq!(status, 200);
    assert!(
        events_for(&id).is_empty(),
        "a passing health check is not logged"
    );

    let failures = start(config(&[("LOG_REQUESTS", "failures")], roomy())).await;
    let (ok, ok_id) = get_id(&failures, "/app/lookup/x").await;
    let (missing, missing_id) = get_id(&failures, "/app/nothing").await;
    assert_eq!((ok, missing), (200, 404));
    assert!(events_for(&ok_id).is_empty());
    assert_eq!(events_for(&missing_id)[0]["status"], 404);

    let off = start(config(&[("LOG_REQUESTS", "off")], roomy())).await;
    let (status, id) = get_id(&off, "/app/nothing").await;
    assert_eq!(status, 404);
    assert!(events_for(&id).is_empty());
    assert_eq!(
        config(&[("LOG_REQUESTS", "off")], roomy()).log_requests,
        RequestLog::Off
    );
}

#[tokio::test]
async fn client_address_only_when_asked() {
    let addr = start(config(&[("LOG_CLIENT_ADDR", "true")], roomy())).await;
    let (_, id) = get_id(&addr, "/app/lookup/x").await;
    assert_eq!(events_for(&id)[0]["client"], "127.0.0.1");
}

#[tokio::test]
async fn electrum_failures_are_summarised() {
    capture();
    // A port that was just free and now refuses connections.
    let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = closed.local_addr().unwrap().to_string();
    drop(closed);
    let mut config = node::electrum::Config::new(addr);
    config.max_reconnect_delay = Duration::from_millis(50);
    let client = node::electrum::Client::spawn(config);
    let _reporter = rbitcoin_space::report::report_electrum(Network::Regtest, &client);

    let deadline = Instant::now() + Duration::from_secs(10);
    let attempts = loop {
        let attempts: Vec<u64> = logged()
            .1
            .iter()
            .filter(|v| v["event"] == "electrum_unreachable" && v["network"] == "regtest")
            .map(|v| v["attempts"].as_u64().unwrap())
            .collect();
        if attempts.len() >= 4 || Instant::now() > deadline {
            break attempts;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    // 1, 2, 4, 8… unless the watch coalesced two updates; each line still marks a new power of two.
    assert!(attempts.len() >= 4, "{attempts:?}");
    assert_eq!(attempts[0], 1);
    assert!(
        attempts.windows(2).all(|w| w[1].ilog2() > w[0].ilog2()),
        "{attempts:?}"
    );
}

#[tokio::test]
async fn electrum_reporter_sees_a_client_that_is_already_up() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    capture();
    // A minimal Electrum server: answers the handshake, then keeps the connection open.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (rd, mut wr) = stream.into_split();
        let mut lines = BufReader::new(rd).lines();
        let genesis = "0100000000000000000000000000000000000000000000000000000000000000000000003ba3edfd7a7b12b27ac72c3e67768f617fc81bc3888a51323a9fb8aa4b1e5e4a29ab5f49ffff001d1dac2b7c";
        while let Ok(Some(line)) = lines.next_line().await {
            let req: Value = serde_json::from_str(&line).unwrap();
            let result = match req["method"].as_str().unwrap() {
                "server.version" => serde_json::json!(["probe-electrum 1.0", "1.6"]),
                "blockchain.headers.subscribe" => serde_json::json!({"height": 0, "hex": genesis}),
                _ => Value::Null,
            };
            let reply = serde_json::json!({"jsonrpc": "2.0", "id": req["id"], "result": result});
            wr.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
        }
    });
    let client = node::electrum::Client::spawn(node::electrum::Config::new(addr));
    let mut health = client.health();
    tokio::time::timeout(Duration::from_secs(5), health.wait_for(|h| h.ready))
        .await
        .unwrap()
        .unwrap();

    let _reporter = rbitcoin_space::report::report_electrum(Network::Testnet4, &client);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let connected = logged()
            .1
            .into_iter()
            .find(|v| v["event"] == "electrum_connected" && v["network"] == "testnet4");
        if let Some(e) = connected {
            assert_eq!(e["server"], "probe-electrum 1.0");
            assert_eq!(e["protocol"], "1.6");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no electrum_connected for a ready client"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
