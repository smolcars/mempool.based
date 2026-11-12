//! The mempool history routes end to end: the server on port 0 over a history table filled
//! here with holes, the mempool engine off, a mock RPC that counts calls, and an Esplora and
//! Electrum that do not answer. Nothing here reaches a real node.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

use axum::routing::any;
use engine::history::{self, DAY, Flow, HOUR, Level, MINUTE, Point, Totals};
use engine::store::{Options, Store};
use rbitcoin_space::{App, Config};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn dir(name: &str) -> Dir {
    let d = std::env::temp_dir().join(format!(
        "rbitcoin-space-statistics-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    Dir(d)
}

fn point(time: u64, samples: u64) -> Point {
    let mut bands = [0; 10];
    bands[0] = 30_000_000 * samples;
    bands[5] = 1_000 * samples;
    bands[9] = 400 * samples;
    Point {
        time,
        samples,
        sums: Totals {
            count: 70_000 * samples,
            vsize: bands.iter().sum(),
            fees: 9_000_000 * samples,
            min_fee: 100 * samples,
            bands,
            flow: Flow {
                in_count: 120 * samples,
                in_vsize: 45_000 * samples,
                out_count: 100 * samples,
                out_vsize: 40_000 * samples,
                mined_count: 90 * samples,
                mined_vsize: 39_000 * samples,
            },
        },
    }
}

/// Every minute of the last three hours (and the next two, so a minute turning during the test
/// leaves nothing owed), a hole of an hour before that, two more hours of minutes, and hours
/// from 20 to 10 days ago.
async fn fill(dir: &Path) -> u64 {
    let now = now() / MINUTE * MINUTE;
    let store = Store::open(dir, "regtest", Options::default()).unwrap();
    let minutes: Vec<Point> = (0..6 * 60)
        .filter(|i| !(180..240).contains(i))
        .map(|i| point(now + 2 * MINUTE - i * MINUTE, 1))
        .collect();
    history::insert(&store, Level::Minute, minutes)
        .await
        .unwrap();
    let hours: Vec<Point> = (10 * 24..20 * 24)
        .map(|h| point((now / HOUR - h) * HOUR, 60))
        .collect();
    history::insert(&store, Level::Hour, hours).await.unwrap();
    store.close().await;
    now
}

/// The server over `dir` (or no data directory), and the count of RPC calls it makes.
async fn start(dir: Option<&Path>) -> (String, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let rpc = axum::Router::new().fallback(any(move |body: String| {
        // The readiness check calls `getblockchaininfo` on its own schedule; count the rest.
        if !body.contains("getblockchaininfo") {
            counter.fetch_add(1, Ordering::SeqCst);
        }
        async { "{}" }
    }));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let rpc_port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, rpc).await });
    let mut vars: HashMap<&str, String> = [
        ("REGTEST_RPC_HOST", "127.0.0.1".to_owned()),
        ("REGTEST_RPC_PORT", rpc_port.to_string()),
        ("REGTEST_RPC_USER", "u".to_owned()),
        ("REGTEST_RPC_PASSWORD", "p".to_owned()),
        ("REGTEST_ESPLORA_URL", "http://127.0.0.1:1".to_owned()),
        ("REGTEST_ELECTRUM_ADDR", "127.0.0.1:1".to_owned()),
        ("BACKFILL_ENABLED", "false".to_owned()),
        // History is read from the store; the engine would poll the node.
        ("REGTEST_MEMPOOL_ENABLED", "false".to_owned()),
    ]
    .into_iter()
    .collect();
    if let Some(d) = dir {
        vars.insert("DATA_DIR", d.display().to_string());
    }
    let mut config = Config::from_vars(|k| vars.get(k).cloned()).unwrap();
    config.limits.client_rate_per_sec = 10_000;
    config.limits.client_burst = 10_000;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(App::new(&config).serve(listener, std::future::pending()));
    (addr, calls)
}

#[derive(Debug)]
struct Reply {
    status: u16,
    head: String,
    body: Vec<u8>,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.head
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{name}: ")))
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }

    fn text(&self) -> String {
        String::from_utf8(self.body.clone()).unwrap()
    }

    fn data_from(&self) -> Option<u64> {
        self.header("x-data-from-time").map(|t| t.parse().unwrap())
    }
}

async fn request(addr: &str, method: &str, target: &str) -> Reply {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let req = format!("{method} {target} HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.unwrap();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
    Reply {
        status: head[9..12].parse().unwrap(),
        head,
        body: raw[split + 4..].to_vec(),
    }
}

async fn get(addr: &str, target: &str) -> Reply {
    request(addr, "GET", target).await
}

#[tokio::test]
async fn statistics_answer_from_the_store_in_the_reference_shape_with_coverage() {
    let d = dir("shape");
    let now = fill(&d.0).await;
    let (addr, calls) = start(Some(&d.0)).await;

    // Two hours, every minute sampled: complete, a point a minute, newest first.
    let r = get(&addr, "/regtest/api/v1/statistics/2h").await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.header("content-type"), Some("application/json"));
    assert_eq!(r.header("access-control-allow-origin"), Some("*"));
    assert_eq!(r.header("x-data-complete"), Some("true"));
    let v = r.json();
    let points = v.as_array().unwrap();
    assert!((120..=124).contains(&points.len()), "{}", points.len());
    assert!(
        points
            .windows(2)
            .all(|w| w[0]["added"].as_u64() > w[1]["added"].as_u64())
    );
    assert_eq!(r.data_from(), points.last().unwrap()["added"].as_u64());
    // Fields in the reference's order and types.
    let first = r.text();
    assert!(
        first.starts_with(r#"[{"added":"#)
            && first.contains(r#","count":70000,"vbytes_per_second":750,"mempool_byte_weight":120005600,"total_fee":9000000,"min_fee":0.1,"vsizes":[30000000,"#),
        "{}",
        &first[..300]
    );
    let vsizes = points[0]["vsizes"].as_array().unwrap();
    assert_eq!(vsizes.len(), 39);
    // Band 5 (1.5 to 2 sat/vB) is in the 1-to-2 range, band 9 (10 and up) at 10 to 12.
    assert_eq!(
        (vsizes[1].as_u64(), vsizes[8].as_u64()),
        (Some(1_000), Some(400))
    );

    // A day holds the hole: not complete, and the hour missing has no points.
    let r = get(&addr, "/regtest/api/v1/statistics/24h").await;
    assert_eq!(r.header("x-data-complete"), Some("false"));
    let v = r.json();
    let times: Vec<u64> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["added"].as_u64().unwrap())
        .collect();
    let hole = (now - 237 * MINUTE)..=(now - 178 * MINUTE);
    assert!(!times.iter().any(|t| hole.contains(t)));
    assert!(times.contains(&(now - 238 * MINUTE)) && times.contains(&(now - 177 * MINUTE)));
    assert_eq!(r.data_from(), times.last().copied());
    assert_eq!(r.data_from(), Some(now + 2 * MINUTE - 359 * MINUTE));

    // A week drops the two fields the reference drops, and averages ten minutes a point.
    let r = get(&addr, "/regtest/api/v1/statistics/1w").await;
    let v = r.json();
    let keys: Vec<&String> = v[0].as_object().unwrap().keys().collect();
    assert_eq!(keys.len(), 5);
    assert!(!r.text().contains("total_fee") && !r.text().contains("mempool_byte_weight"));
    assert!(
        v.as_array()
            .unwrap()
            .iter()
            .all(|p| p["added"].as_u64().unwrap() % 600 == 0)
    );

    // A month reaches the hour rows; their means are the same mempool.
    let r = get(&addr, "/regtest/api/v1/statistics/1m").await;
    assert_eq!(r.header("x-data-complete"), Some("false"));
    let v = r.json();
    let oldest = v.as_array().unwrap().last().unwrap();
    assert_eq!(oldest["count"], 70_000);
    assert_eq!(oldest["vbytes_per_second"], 750);
    assert_eq!(r.data_from(), oldest["added"].as_u64());
    assert!(r.data_from().unwrap() <= now - 19 * DAY);

    // Every interval answers; `all` begins at the oldest row.
    for i in [
        "2h", "24h", "1w", "1m", "3m", "6m", "1y", "2y", "3y", "4y", "all",
    ] {
        let r = get(&addr, &format!("/regtest/api/v1/statistics/{i}")).await;
        assert_eq!(r.status, 200, "{i}");
        assert!(r.json().as_array().unwrap().len() <= 1_460, "{i}");
    }

    // Our own shape: oldest first, means per minute, with what was sampled.
    let r = get(&addr, "/regtest/app/mempool/history/24h").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("x-data-complete"), Some("false"));
    let v = r.json();
    assert_eq!(v["interval"], "24h");
    assert_eq!(v["step"], 60);
    assert_eq!(v["complete"], false);
    assert_eq!(
        v["first"].as_u64(),
        Some((now / HOUR - (20 * 24 - 1)) * HOUR)
    );
    // Five hours of minutes are owed and sampled (the two ahead are not owed); the rest is not.
    let (sampled, minutes) = (
        v["sampled"].as_u64().unwrap(),
        v["minutes"].as_u64().unwrap(),
    );
    assert!((24 * 60 - 1..=24 * 60).contains(&minutes), "{minutes}");
    assert!((297..=300).contains(&sampled), "{sampled}");
    let p = &v["points"][0];
    assert_eq!(p["samples"], 1);
    assert_eq!(p["count"], 70_000);
    assert_eq!(p["bands"].as_array().unwrap().len(), 10);
    assert_eq!(p["mined_vsize"], 39_000);
    assert_eq!(p["min_fee_rate"], 100);
    let m = get(&addr, "/regtest/app/mempool/history/1m").await.json();
    assert_eq!(m["step"], 3600);
    assert_eq!(m["points"][0]["samples"], 60);
    assert_eq!(m["points"][0]["in_vsize"], 45_000);

    // Again: from memory, the same bytes.
    let again = get(&addr, "/regtest/api/v1/statistics/2h").await;
    assert_eq!(again.status, 200);
    assert_eq!(
        request(&addr, "HEAD", "/regtest/api/v1/statistics/2h")
            .await
            .status,
        200
    );

    // Strict parameters, refused before any work.
    for bad in ["5h", "2H", "24h ", "1d", "", "all/x"] {
        let r = get(&addr, &format!("/regtest/api/v1/statistics/{bad}")).await;
        assert!(r.status == 400 || r.status == 404, "{bad}: {}", r.status);
    }
    let r = get(&addr, "/regtest/api/v1/statistics/5h").await;
    assert_eq!(
        (r.status, r.json()["error"].as_str()),
        (400, Some("invalid interval"))
    );
    assert_eq!(
        get(&addr, "/regtest/app/mempool/history/1d").await.status,
        400
    );

    assert_eq!(calls.load(Ordering::SeqCst), 0, "no node call");
}

#[tokio::test]
async fn without_a_data_directory_the_routes_say_so() {
    let (addr, calls) = start(None).await;
    for target in [
        "/regtest/api/v1/statistics/2h",
        "/regtest/app/mempool/history/2h",
    ] {
        let r = get(&addr, target).await;
        assert_eq!(r.status, 503);
        assert_eq!(
            r.json()["error"],
            "mempool history needs a data directory on this server"
        );
    }
    // An empty table is an empty history, not an error.
    let d = dir("empty");
    let (addr, _) = start(Some(&d.0)).await;
    let r = get(&addr, "/regtest/api/v1/statistics/all").await;
    assert_eq!((r.status, r.text().as_str()), (200, "[]"));
    assert_eq!(r.header("x-data-complete"), Some("false"));
    assert_eq!(r.header("x-data-from-time"), None);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
