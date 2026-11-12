//! The Lightning routes and wiring end to end: the server on port 0 with Lightning on, its one
//! peer unreachable, over a store filled here, a mock RPC that counts calls, and an Esplora and
//! Electrum that do not answer. Nothing here reaches a real node or the Lightning network.

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
use std::time::Duration;

use axum::routing::any;
use bitcoin::hashes::Hash;
use bitcoin::{BlockHash, Txid};
use engine::lightning::{self, Block, Change, Close, EVERY, Kind, Level, Open, Point, Stats};
use engine::store::{Options, Store};
use mempool_based::{App, Config};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn dir(name: &str) -> Dir {
    let d = std::env::temp_dir().join(format!(
        "mempool-based-lightning-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    Dir(d)
}

fn block(height: u32) -> Block {
    Block {
        height,
        hash: BlockHash::from_byte_array([7; 32]),
        time: 1_700_000_000,
    }
}

fn open(n: u64) -> Open {
    Open {
        scid: (800_000 + n) << 40 | 3 << 16,
        node_one: [2; 33],
        node_two: [3; 33],
        capacity: 1_000_000,
        block: block(800_000 + u32::try_from(n).unwrap()),
    }
}

fn stats(n: u64) -> Stats {
    Stats {
        channels: 36_000 * n,
        nodes: 13_000 * n,
        capacity: 323_100_000_000 * n,
        tor_nodes: 6_000 * n,
        clearnet_nodes: 3_000 * n,
        clearnet_tor_nodes: 1_000 * n,
        unannounced_nodes: 3_000 * n,
        med_capacity: 2_000_000 * n,
        policies: 60_000 * n,
        fee_rate: 60_000 * 800 * n,
        base_fee: 60_000 * 900 * n,
        med_fee_rate: 100 * n,
        med_base_fee: 500 * n,
    }
}

/// 120 channels, 30 of them closed, and a snapshot every ten minutes for the last day.
async fn fill(dir: &Path) {
    let store = Store::open(dir, "regtest", Options::default()).unwrap();
    let mut changes: Vec<Change> = (0..120).map(|n| Change::Opened(open(n))).collect();
    changes.extend((0..30).map(|n| {
        Change::Closed(
            open(n),
            Close {
                block: block(800_200 + u32::try_from(n).unwrap()),
                txid: Txid::from_byte_array([9; 32]),
                kind: Kind::Cooperative,
                fee: Some(300),
            },
        )
    }));
    lightning::apply(&store, changes).await.unwrap();
    let now = lightning::unix_now() / EVERY * EVERY;
    let points = (0..=144)
        .map(|i| Point {
            time: now - i * EVERY,
            samples: 1,
            sums: stats(1),
        })
        .collect();
    lightning::insert(&store, Level::Minute, points)
        .await
        .unwrap();
    store.close().await;
}

/// A peer that cannot be reached: nothing listens on port 1.
const PEER: &str = "02aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa@127.0.0.1:1";

/// The server over `dir`, Lightning on or off, and the count of RPC calls it makes.
async fn start(
    dir: &Path,
    lightning: bool,
) -> (
    String,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
    tokio::sync::oneshot::Sender<()>,
) {
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
        ("REGTEST_MEMPOOL_ENABLED", "false".to_owned()),
        ("BACKFILL_ENABLED", "false".to_owned()),
        ("DATA_DIR", dir.display().to_string()),
    ]
    .into_iter()
    .collect();
    if lightning {
        vars.insert("REGTEST_LIGHTNING_ENABLED", "true".to_owned());
        vars.insert("REGTEST_LIGHTNING_PEERS", PEER.to_owned());
    }
    let mut config = Config::from_vars(|k| vars.get(k).cloned()).unwrap();
    config.limits.client_rate_per_sec = 10_000;
    config.limits.client_burst = 10_000;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(App::new(&config).serve(listener, async {
        let _ = stopped.await;
    }));
    (addr, calls, server, stop)
}

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
}

async fn get(addr: &str, target: &str) -> Reply {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let req = format!("GET {target} HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n");
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

#[tokio::test]
async fn routes_answer_from_the_store_with_strict_parameters_and_no_node_calls() {
    let d = dir("routes");
    fill(&d.0).await;
    // A corrupt graph file: the peer starts empty and says so.
    let ln_dir = d.0.join("lightning").join("regtest");
    std::fs::create_dir_all(&ln_dir).unwrap();
    std::fs::write(ln_dir.join("graph"), b"not a graph").unwrap();
    let (addr, calls, server, stop) = start(&d.0, true).await;

    let s = get(&addr, "/regtest/app/lightning/status").await.json();
    assert!(
        ["starting", "syncing"].contains(&s["state"].as_str().unwrap()),
        "{s}"
    );
    assert_eq!(s["peers_configured"], 1, "{s}");

    // Our latest snapshot, and the reference's shape of it.
    let r = get(&addr, "/regtest/app/lightning/statistics/latest").await;
    assert_eq!(r.status, 200);
    let v = r.json();
    assert_eq!(v["channels"], 36_000);
    assert_eq!(v["avg_capacity"], 323_100_000_000u64 / 36_000);
    assert_eq!(
        (v["avg_fee_rate"].clone(), v["med_fee_rate"].clone()),
        (800.into(), 100.into())
    );
    let r = get(&addr, "/regtest/api/v1/lightning/statistics/latest").await;
    assert_eq!(r.header("access-control-allow-origin"), Some("*"));
    let latest = &r.json()["latest"];
    assert_eq!(latest["channel_count"], 36_000);
    assert_eq!(latest["node_count"], 13_000);
    assert_eq!(latest["avg_base_fee_mtokens"], 900);
    assert!(latest["added"].as_str().unwrap().ends_with(":00.000Z"));
    // The reference's fields, in its order.
    let text = String::from_utf8(r.body.clone()).unwrap();
    let order = [
        "id",
        "added",
        "channel_count",
        "node_count",
        "total_capacity",
        "tor_nodes",
        "clearnet_nodes",
        "unannounced_nodes",
        "avg_capacity",
        "avg_fee_rate",
        "avg_base_fee_mtokens",
        "med_capacity",
        "med_fee_rate",
        "med_base_fee_mtokens",
        "clearnet_tor_nodes",
    ];
    let at: Vec<usize> = order
        .iter()
        .map(|k| text.find(&format!("\"{k}\":")).unwrap())
        .collect();
    assert!(at.windows(2).all(|w| w[0] < w[1]), "{text}");
    assert_eq!(latest.as_object().unwrap().len(), order.len());

    // A day of ten-minute snapshots: complete, newest first in the reference's shape.
    let r = get(&addr, "/regtest/api/v1/lightning/statistics/24h").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("x-data-complete"), Some("true"));
    let points = r.json();
    let points = points.as_array().unwrap();
    assert!((144..=145).contains(&points.len()), "{}", points.len());
    assert!(points[0]["added"].as_u64() > points[1]["added"].as_u64());
    let text = String::from_utf8(r.body.clone()).unwrap();
    assert!(
        text.starts_with(r#"[{"added":"#) && text.contains(r#","channel_count":36000,"total_capacity":323100000000,"tor_nodes":6000,"clearnet_nodes":3000,"unannounced_nodes":3000,"clearnet_tor_nodes":1000}"#),
        "{text}"
    );
    assert_eq!(points[0].as_object().unwrap().len(), 7);
    let a = get(&addr, "/regtest/app/lightning/statistics/1m")
        .await
        .json();
    assert_eq!(a["step"], 3_600);
    assert_eq!(a["complete"], false);
    // Hourly points, each the mean of its snapshots: all 145 are counted once.
    let samples: u64 = a["points"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["samples"].as_u64().unwrap())
        .sum();
    assert_eq!(samples, 145);
    assert_eq!(a["points"][0]["channels"], 36_000);
    for bad in ["5h", "1M", "", "24h%00"] {
        let r = get(&addr, &format!("/regtest/app/lightning/statistics/{bad}")).await;
        assert!(r.status == 400 || r.status == 404, "{bad}: {}", r.status);
    }

    // The newest opens and closes, capped.
    let o = get(&addr, "/regtest/app/lightning/channels/opened")
        .await
        .json();
    let o = o.as_array().unwrap();
    assert_eq!(o.len(), 25);
    assert_eq!(o[0]["id"], "800119x3x0");
    assert_eq!(o[0]["closed"], Value::Null);
    let c = get(&addr, "/regtest/app/lightning/channels/closed?limit=100")
        .await
        .json();
    let c = c.as_array().unwrap();
    assert_eq!(c.len(), 30);
    assert_eq!(c[0]["closed"]["block"]["height"], 800_229);
    assert_eq!(c[0]["closed"]["kind"], "cooperative");
    assert_eq!(c[0]["node_one"]["id"].as_str().unwrap().len(), 66);
    for q in [
        "limit=0",
        "limit=101",
        "limit=x",
        "page=2",
        "limit=5&limit=6",
    ] {
        let r = get(
            &addr,
            &format!("/regtest/app/lightning/channels/opened?{q}"),
        )
        .await;
        assert_eq!(r.status, 400, "{q}");
    }

    // The explorer around it works, and readiness reports Lightning without depending on it.
    assert_eq!(get(&addr, "/healthz").await.status, 200);
    let ready = get(&addr, "/readyz").await.json();
    assert!(
        ready["networks"]["regtest"]["lightning"].is_string(),
        "{ready}"
    );
    assert_eq!(
        get(&addr, "/regtest/api/v1/statistics/2h").await.status,
        200
    );

    assert_eq!(calls.load(Ordering::SeqCst), 0, "a route called the node");
    // The server stops promptly, the peer with it.
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn with_lightning_off_the_routes_say_so() {
    let d = dir("off");
    let (addr, _, _server, _stop) = start(&d.0, false).await;
    assert_eq!(
        get(&addr, "/regtest/app/lightning/status").await.json(),
        serde_json::json!({ "state": "off" })
    );
    for path in [
        "/regtest/app/lightning/statistics/latest",
        "/regtest/app/lightning/statistics/24h",
        "/regtest/app/lightning/channels/closed",
        "/regtest/api/v1/lightning/statistics/latest",
    ] {
        let r = get(&addr, path).await;
        assert_eq!(r.status, 503, "{path}");
        assert_eq!(r.json()["error"], "lightning is off on this network");
    }
    let ready = get(&addr, "/readyz").await.json();
    assert_eq!(ready["networks"]["regtest"]["lightning"], "off");
}

#[tokio::test]
async fn a_peer_that_cannot_start_leaves_the_explorer_working() {
    let d = dir("broken");
    fill(&d.0).await;
    // Where the peer's directory should be, a file: it cannot create its key there.
    std::fs::write(d.0.join("lightning"), b"in the way").unwrap();
    let (addr, calls, _server, _stop) = start(&d.0, true).await;
    let mut state = Value::Null;
    for _ in 0..100 {
        state = get(&addr, "/regtest/app/lightning/status").await.json()["state"].clone();
        if state == "failed" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(state, "failed");
    // What was stored is still served, and so is everything else.
    assert_eq!(
        get(&addr, "/regtest/app/lightning/channels/closed?limit=3")
            .await
            .json()
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(get(&addr, "/healthz").await.status, 200);
    assert_eq!(
        get(&addr, "/regtest/api/v1/statistics/2h").await.status,
        200
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
