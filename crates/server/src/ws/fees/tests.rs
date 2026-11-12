//! The fee routes over HTTP on port 0, from answers set in memory, and the fallback to a fake
//! node's estimates. The task against real engines is in `ws/mempool/tests.rs`, beside its mock
//! node.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::extract::State;
use axum::routing::{get, post};
use bitcoin::BlockHash;
use bitcoin::hashes::Hash;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::*;
use crate::Config;
use crate::ws::Sockets;

/// A network whose node is nowhere (`127.0.0.1:1`) unless `node` names a fake one.
fn context(mempool: bool, node: Option<&str>) -> Arc<NetworkContext> {
    let (rpc_port, esplora) = node.map_or(("1".to_owned(), "http://127.0.0.1:1".to_owned()), |a| {
        (
            a.rsplit(':').next().unwrap().to_owned(),
            format!("http://{a}"),
        )
    });
    let vars: HashMap<&str, String> = [
        ("REGTEST_RPC_HOST", "127.0.0.1".to_owned()),
        ("REGTEST_RPC_PORT", rpc_port),
        ("REGTEST_RPC_USER", "u".to_owned()),
        ("REGTEST_RPC_PASSWORD", "p".to_owned()),
        ("REGTEST_ESPLORA_URL", esplora),
        ("REGTEST_ELECTRUM_ADDR", "127.0.0.1:1".to_owned()),
        ("REGTEST_MEMPOOL_ENABLED", mempool.to_string()),
    ]
    .into_iter()
    .collect();
    let mut config = Config::from_vars(|k| vars.get(k).cloned()).unwrap();
    config.ws.mempool_interval = Duration::from_millis(20);
    let sockets = Sockets::new(&config.ws, Duration::from_secs(1));
    Arc::new(NetworkContext::new(&config.networks[0], &config, &sockets).unwrap())
}

/// Serves the `/api/v1` routes for `ctx` on port 0.
async fn serve(ctx: &Arc<NetworkContext>) -> String {
    let app = crate::api_v1::routes().with_state(Arc::clone(ctx));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move { axum::serve(listener, app).await });
    addr
}

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

async fn get_path(addr: &str, path: &str) -> Reply {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).await.unwrap();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
    Reply {
        status: head[9..12].parse().unwrap(),
        head,
        body: String::from_utf8_lossy(&raw[split + 4..]).into_owned(),
    }
}

fn tip() -> ChainTip {
    ChainTip {
        hash: BlockHash::from_byte_array([7; 32]),
        height: 975_530,
    }
}

fn answer(source: Source) -> Answer {
    Answer {
        rates: Rates {
            levels: [2_633, 1_380, 628, 200],
            minimum: 100,
        },
        source,
        tip: (source == Source::Projected).then(tip),
        at: SystemTime::now() - Duration::from_secs(12),
    }
}

#[tokio::test]
async fn the_routes_answer_from_memory_in_the_references_shapes() {
    // The node is unreachable: anything that asked it would fail.
    let ctx = context(true, None);
    let addr = serve(&ctx).await;
    for path in ["recommended", "precise", "mempool-blocks"] {
        let r = get_path(&addr, &format!("/api/v1/fees/{path}")).await;
        assert_eq!(r.status, 503, "{path}");
        assert_eq!(r.header("retry-after"), Some("1"));
        assert_eq!(r.body, r#"{"error":"not ready yet; retry"}"#);
    }

    ctx.fees.set(Some(answer(Source::Projected)));
    let r = get_path(&addr, "/api/v1/fees/recommended?cache-buster=1").await;
    assert_eq!(r.status, 200);
    // Rounded up, at least 1: never less than the precise rate.
    assert_eq!(
        r.body,
        r#"{"fastestFee":3,"halfHourFee":2,"hourFee":1,"economyFee":1,"minimumFee":1}"#
    );
    assert_eq!(r.header("x-fee-source"), Some("projected"));
    assert_eq!(r.header("x-bitcoin-chain-tip-height"), Some("975530"));
    assert_eq!(r.header("cache-control"), Some("public, max-age=1"));
    assert_eq!(r.header("age"), None);
    let r = get_path(&addr, "/api/v1/fees/precise").await;
    assert_eq!(
        r.body,
        r#"{"fastestFee":2.633,"halfHourFee":1.38,"hourFee":0.628,"economyFee":0.2,"minimumFee":0.1}"#
    );

    // The node's estimates say so, and how old they are.
    ctx.fees.set(Some(answer(Source::Node)));
    let r = get_path(&addr, "/api/v1/fees/precise").await;
    assert_eq!(r.header("x-fee-source"), Some("node"));
    assert_eq!(r.header("x-bitcoin-chain-tip"), None);
    let age: u64 = r.header("age").unwrap().parse().unwrap();
    assert!((12..20).contains(&age), "{age}");
    // Past their limit they are not served, whenever the task last looked.
    ctx.fees.set(Some(Answer {
        at: SystemTime::now() - NODE_MAX_AGE,
        ..answer(Source::Node)
    }));
    assert_eq!(get_path(&addr, "/api/v1/fees/precise").await.status, 503);

    ctx.fees.set_blocks(Blocks {
        blocks: vec![
            MempoolBlock {
                size: 1_447_342,
                vsize: 997_945,
                txs: 2_222,
                fees: 3_205_963,
                median: 2_856,
                range: vec![2_000, 2_318, 3_013, 3_384, 4_021, 5_952, 200_711],
            },
            MempoolBlock {
                size: 0,
                vsize: 0,
                txs: 0,
                fees: 0,
                median: 0,
                range: Vec::new(),
            },
        ]
        .into(),
        tip: Some(tip()),
        stale: false,
        at: SystemTime::now(),
    });
    let r = get_path(&addr, "/api/v1/fees/mempool-blocks").await;
    assert_eq!(r.status, 200);
    assert_eq!(
        r.body,
        r#"[{"blockSize":1447342,"blockVSize":997945,"nTx":2222,"totalFees":3205963,"medianFee":2.856,"feeRange":[2.0,2.318,3.013,3.384,4.021,5.952,200.711]}]"#
    );
    assert_eq!(r.header("age"), None);
}

#[tokio::test]
async fn with_the_engine_off_mempool_blocks_says_so() {
    let ctx = context(false, None);
    let addr = serve(&ctx).await;
    let r = get_path(&addr, "/api/v1/fees/mempool-blocks").await;
    assert_eq!(r.status, 503);
    assert_eq!(
        r.body,
        r#"{"error":"the mempool engine is off on this server"}"#
    );
}

#[test]
fn the_topic_names_each_level_and_its_horizon() {
    let msg = message(Some(&answer(Source::Projected)));
    assert_eq!(msg["type"], "fees");
    assert_eq!(msg["source"], "projected");
    assert_eq!(msg["tip"]["height"], 975_530);
    assert_eq!(msg["min_fee_rate"], 100);
    let levels: Vec<(String, u64, u64, u64)> = msg["levels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            (
                l["name"].as_str().unwrap().to_owned(),
                l["blocks"].as_u64().unwrap(),
                l["minutes"].as_u64().unwrap(),
                l["rate"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        levels,
        [
            ("high".into(), 1, 10, 2_633),
            ("medium".into(), 3, 30, 1_380),
            ("low".into(), 6, 60, 628),
            ("no_rush".into(), 144, 1_440, 200),
        ]
    );
    assert_eq!(
        message(None).to_string(),
        r#"{"topic":"fees","type":"unavailable"}"#
    );
}

/// A node that answers `getmempoolinfo` and `/fee-estimates`, counting requests.
#[derive(Clone, Default)]
struct FakeNode {
    hits: Arc<AtomicUsize>,
}

async fn fake_node() -> (String, FakeNode) {
    async fn estimates(State(n): State<FakeNode>) -> String {
        n.hits.fetch_add(1, Ordering::SeqCst);
        r#"{"1":4.299,"2":2.453,"3":1.799,"6":1.597,"144":0.128,"1008":0.128}"#.to_owned()
    }
    async fn rpc(State(n): State<FakeNode>, body: String) -> String {
        n.hits.fetch_add(1, Ordering::SeqCst);
        let req: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(req["method"], "getmempoolinfo");
        serde_json::json!({
            "jsonrpc": "2.0", "id": req["id"], "error": null,
            "result": {
                "loaded": true, "size": 3, "bytes": 600, "usage": 3000, "maxmempool": 300_000_000,
                "total_fee": 0.00001, "mempoolminfee": 0.000_002, "minrelaytxfee": 0.000_001,
                "incrementalrelayfee": 0.000_001, "fullrbf": true, "limitclustercount": 64,
                "limitclustersize": 101_000, "unbroadcastcount": 0
            }
        })
        .to_string()
    }
    let node = FakeNode::default();
    let app = axum::Router::new()
        .route("/fee-estimates", get(estimates))
        .route("/", post(rpc))
        .with_state(node.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move { axum::serve(listener, app).await });
    (addr, node)
}

#[tokio::test]
async fn with_the_engine_off_the_nodes_estimates_are_served_and_marked() {
    let (node_addr, node) = fake_node().await;
    let ctx = context(false, Some(&node_addr));
    let task = tokio::spawn(run(Arc::clone(&ctx), None, Recommender::default()));
    // Nobody asked for fees: the node is not asked either.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(node.hits.load(Ordering::SeqCst), 0);
    assert!(ctx.fees.current().is_none());
    // A subscriber wants them.
    let (conn, mut rx) = crate::ws::mempool::tests::conn(64);
    ctx.hub.subscribe(Topic::Fees, &conn);
    let deadline = Instant::now() + Duration::from_secs(10);
    while ctx.fees.current().is_none() {
        assert!(Instant::now() < deadline, "never answered");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let a = ctx.fees.current().unwrap();
    assert_eq!(a.source, Source::Node);
    // The node's minimum, 0.2 sat/vB, lifts the slow levels.
    assert_eq!(a.rates.levels, [4_299, 1_799, 1_597, 200]);
    assert_eq!(a.rates.minimum, 200);
    let mut last = Value::Null;
    while let Ok(f) = rx.try_recv() {
        last = serde_json::from_slice(crate::ws::mempool::tests::open(&f).1).unwrap();
    }
    assert_eq!(last["source"], "node");
    assert_eq!(last["tip"], Value::Null);

    // Many requests, no node call: the answer is in memory, and the node is asked only by the
    // task, every 30 s.
    let addr = serve(&ctx).await;
    let before = node.hits.load(Ordering::SeqCst);
    for _ in 0..20 {
        let r = get_path(&addr, "/api/v1/fees/recommended").await;
        assert_eq!(
            r.body,
            r#"{"fastestFee":5,"halfHourFee":2,"hourFee":2,"economyFee":1,"minimumFee":1}"#
        );
        assert_eq!(r.header("x-fee-source"), Some("node"));
    }
    assert_eq!(node.hits.load(Ordering::SeqCst), before);
    task.abort();
}

#[tokio::test]
async fn an_old_answer_is_withdrawn_before_waiting_on_a_node_that_hangs() {
    // A node that accepts connections and never answers.
    let hang = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = hang.local_addr().unwrap().to_string();
    let ctx = context(false, Some(&addr));
    ctx.fees.set(Some(answer(Source::Projected)));
    let (conn, mut rx) = crate::ws::mempool::tests::conn(64);
    ctx.hub.subscribe(Topic::Fees, &conn);
    let task = tokio::spawn(run(Arc::clone(&ctx), None, Recommender::default()));
    let deadline = Instant::now() + Duration::from_secs(1);
    while ctx.fees.current().is_some() {
        assert!(
            Instant::now() < deadline,
            "the old answer stayed while the node hung"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let f = rx.recv().await.unwrap();
    let v: Value = serde_json::from_slice(crate::ws::mempool::tests::open(&f).1).unwrap();
    assert_eq!(v["type"], "unavailable");
    task.abort();
    drop(hang);
}
