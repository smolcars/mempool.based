//! `/api/v1/difficulty-adjustment` end to end: the app against a scripted Electrum server and a
//! mock Esplora on port 0. Nothing here reaches a real node.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::HashMap;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::body::{Body, to_bytes};
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{Method, Request, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use bitcoin::block::{Header, Version};
use bitcoin::consensus::encode::serialize_hex;
use bitcoin::hashes::Hash;
use bitcoin::{BlockHash, CompactTarget, TxMerkleNode};
use logging::{Format, Level, Settings};
use rbitcoin_space::{App, Config};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::TcpListener;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::time::timeout;
use tower::ServiceExt;

const WAIT: Duration = Duration::from_secs(5);
const ROUTE: &str = "/api/v1/difficulty-adjustment";
/// Mainnet's minimum difficulty, and a target twice as hard.
const LIMIT: u32 = 0x1d00_ffff;
const DOUBLE: u32 = 0x1c7f_ff80;
const T0: u32 = 1_790_000_000;

// The node side.

/// What the mock Esplora serves: block JSON by height, a count of `/blocks` requests, a switch
/// that makes every request fail, and the chain-tip stamp its answers carry.
#[derive(Clone, Default)]
struct Esplora {
    blocks: Arc<Mutex<HashMap<u32, Value>>>,
    hits: Arc<AtomicUsize>,
    down: Arc<AtomicBool>,
    stamp: Arc<Mutex<Option<(BlockHash, u32)>>>,
}

impl Esplora {
    async fn start(&self) -> String {
        async fn blocks(Path(height): Path<u32>, State(e): State<Esplora>) -> impl IntoResponse {
            e.hits.fetch_add(1, Ordering::SeqCst);
            if e.down.load(Ordering::SeqCst) {
                return (StatusCode::INTERNAL_SERVER_ERROR, "down").into_response();
            }
            let mut res = match e.blocks.lock().unwrap().get(&height) {
                Some(b) => (StatusCode::OK, json!([b]).to_string()).into_response(),
                None => (StatusCode::NOT_FOUND, "Block not found").into_response(),
            };
            if let Some((hash, height)) = *e.stamp.lock().unwrap() {
                let h = res.headers_mut();
                h.insert("x-bitcoin-chain-tip", hash.to_string().parse().unwrap());
                h.insert("x-bitcoin-chain-tip-height", height.into());
            }
            res
        }
        let app = axum::Router::new()
            .route("/blocks/{height}", get(blocks))
            .with_state(self.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        url
    }

    fn add(&self, height: u32, h: &Header) {
        let block = json!({
            "id": h.block_hash().to_string(), "height": height, "version": 0x2000_0000,
            "timestamp": h.time, "tx_count": 1, "size": 300, "weight": 1200,
            "merkle_root": h.merkle_root.to_string(),
            "previousblockhash": h.prev_blockhash.to_string(),
            "mediantime": h.time, "nonce": h.nonce, "bits": h.bits.to_consensus(),
            "difficulty": 1.0,
        });
        self.blocks.lock().unwrap().insert(height, block);
    }
}

/// One connection from the server's Electrum client.
struct Peer {
    lines: Lines<BufReader<OwnedReadHalf>>,
    wr: OwnedWriteHalf,
}

impl Peer {
    async fn accept(listener: &TcpListener) -> Peer {
        let (stream, _) = timeout(WAIT, listener.accept()).await.unwrap().unwrap();
        let (rd, wr) = stream.into_split();
        Peer {
            lines: BufReader::new(rd).lines(),
            wr,
        }
    }

    async fn expect(&mut self, method: &str) -> Value {
        let line = timeout(WAIT, self.lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let req: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(req["method"], method, "{req}");
        req
    }

    /// Fails if the server sends anything within a short wait.
    async fn silent(&mut self) {
        let next = timeout(Duration::from_millis(300), self.lines.next_line()).await;
        assert!(next.is_err(), "unexpected request: {next:?}");
    }

    async fn send(&mut self, msg: Value) {
        let line = format!("{msg}\n");
        self.wr.write_all(line.as_bytes()).await.unwrap();
    }

    async fn reply(&mut self, req: &Value, result: Value) {
        self.send(json!({"jsonrpc": "2.0", "id": req["id"], "result": result}))
            .await;
    }

    async fn handshake(&mut self, height: u32, tip: &Header) {
        let req = self.expect("server.version").await;
        self.reply(&req, json!(["rbitcoin-electrs 0.9.2", "1.6"]))
            .await;
        let req = self.expect("blockchain.headers.subscribe").await;
        self.reply(&req, json!({"height": height, "hex": serialize_hex(tip)}))
            .await;
    }

    async fn push_tip(&mut self, height: u32, tip: &Header) {
        let params = json!([{"height": height, "hex": serialize_hex(tip)}]);
        self.send(
            json!({"jsonrpc": "2.0", "method": "blockchain.headers.subscribe", "params": params}),
        )
        .await;
    }
}

/// `n` linked headers after `prev`, `spacing` seconds apart from `time`.
fn chain(prev: BlockHash, n: u32, time: u32, spacing: u32, bits: u32, salt: u32) -> Vec<Header> {
    let mut out: Vec<Header> = Vec::new();
    for i in 0..n {
        out.push(Header {
            version: Version::TWO,
            prev_blockhash: out.last().map_or(prev, Header::block_hash),
            merkle_root: TxMerkleNode::all_zeros(),
            time: time + i * spacing,
            bits: CompactTarget::from_consensus(bits),
            nonce: salt * 100_000 + i,
        });
    }
    out
}

fn header(time: u32, bits: u32) -> Header {
    chain(BlockHash::all_zeros(), 1, time, 0, bits, 9)[0]
}

/// The app with mainnet pointed at the mocks, and the Electrum listener it connects to.
async fn start(esplora: &Esplora) -> (App, TcpListener) {
    capture();
    let electrum = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let vars: HashMap<&str, String> = HashMap::from([
        ("MAINNET_RPC_HOST", "127.0.0.1".to_owned()),
        ("MAINNET_RPC_PORT", "1".to_owned()),
        ("MAINNET_RPC_USER", "u".to_owned()),
        ("MAINNET_RPC_PASSWORD", "p".to_owned()),
        ("MAINNET_ESPLORA_URL", esplora.start().await),
        (
            "MAINNET_ELECTRUM_ADDR",
            electrum.local_addr().unwrap().to_string(),
        ),
    ]);
    let mut config = Config::from_vars(|k| vars.get(k).cloned()).unwrap();
    config.limits.client_burst = 10_000;
    config.limits.client_rate_per_sec = 10_000;
    (App::new(&config), electrum)
}

// The client side.

struct Answer {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: Value,
    raw: String,
}

async fn request(app: &App, method: Method, path: &str) -> Answer {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 9))));
    let res = app.router().oneshot(req).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    Answer {
        status,
        headers,
        body,
        raw: String::from_utf8(bytes.to_vec()).unwrap(),
    }
}

async fn get_answer(app: &App) -> Answer {
    request(app, Method::GET, ROUTE).await
}

/// Polls until the answer is for `height`.
async fn answer_at(app: &App, height: u32) -> Answer {
    let deadline = Instant::now() + WAIT;
    loop {
        let a = get_answer(app).await;
        if a.status == StatusCode::OK && hv(&a, "x-bitcoin-chain-tip-height") == height.to_string()
        {
            return a;
        }
        assert!(
            Instant::now() < deadline,
            "no answer at {height}: {}",
            a.body
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn hv(a: &Answer, name: &str) -> String {
    a.headers
        .get(name)
        .map(|v| v.to_str().unwrap().to_owned())
        .unwrap_or_default()
}

fn close(a: &Value, b: f64) -> bool {
    (a.as_f64().unwrap() - b).abs() < 1e-6
}

// The log. Global per test binary, so each test looks for lines only it can produce.

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
            max_bytes_per_sec: logging::DEFAULT_MAX_BYTES_PER_SEC,
        };
        assert!(logging::init(settings, c.clone()));
        c
    })
}

async fn logged(f: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + WAIT;
    loop {
        logging::flush(Duration::from_secs(1));
        let raw = String::from_utf8(capture().0.lock().unwrap().clone()).unwrap();
        if let Some(v) = raw
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .find(|v| f(v))
        {
            return v;
        }
        assert!(Instant::now() < deadline, "not logged");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

// Tests.

#[tokio::test]
async fn answers_from_memory_in_the_reference_shape() {
    let esplora = Esplora::default();
    // The period starting at 2016 began at difficulty 2, after a first period at the minimum.
    let first = header(T0, DOUBLE);
    esplora.add(2016, &first);
    esplora.add(0, &header(T0 - 1_209_600, LIMIT));
    let (app, electrum) = start(&esplora).await;

    // Before the first tip there is no answer: a 503 the client can retry, in the error model.
    let before = get_answer(&app).await;
    assert_eq!(before.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(before.body, json!({"error": "not ready yet; retry"}));
    assert_eq!(hv(&before, "retry-after"), "1");
    assert_eq!(hv(&before, "cache-control"), "no-store");
    assert_eq!(hv(&before, "access-control-allow-origin"), "*");

    // The tip: 1000 blocks into the period at 540 s each.
    let tip = header(T0 + 1000 * 540, DOUBLE);
    let mut peer = Peer::accept(&electrum).await;
    peer.handshake(3016, &tip).await;
    let a = answer_at(&app, 3016).await;
    // In the reference's order, which `Value` would not keep.
    let keys = [
        "progressPercent",
        "difficultyChange",
        "estimatedRetargetDate",
        "remainingBlocks",
        "remainingTime",
        "previousRetarget",
        "previousTime",
        "nextRetargetHeight",
        "timeAvg",
        "adjustedTimeAvg",
        "timeOffset",
        "expectedBlocks",
    ];
    let at: Vec<usize> = keys
        .iter()
        .map(|k| a.raw.find(&format!("\"{k}\":")).unwrap())
        .collect();
    assert!(at.is_sorted(), "{}", a.raw);
    assert_eq!(a.body.as_object().unwrap().len(), keys.len());
    let b = &a.body;
    for k in [
        "estimatedRetargetDate",
        "remainingBlocks",
        "remainingTime",
        "previousTime",
        "nextRetargetHeight",
        "timeAvg",
        "adjustedTimeAvg",
        "timeOffset",
    ] {
        assert!(b[k].is_u64() || b[k].is_i64(), "{k} is an integer: {b}");
    }
    assert!(close(&b["progressPercent"], 1000.0 / 2016.0 * 100.0));
    assert_eq!(b["remainingBlocks"], 1016);
    assert_eq!(b["nextRetargetHeight"], 4032);
    assert_eq!(b["previousTime"], T0);
    assert_eq!(b["timeAvg"], 540_000);
    assert_eq!(b["adjustedTimeAvg"], 540_000);
    assert_eq!(b["timeOffset"], 0);
    assert_eq!(b["remainingTime"], 1016 * 540_000);
    assert!(close(&b["previousRetarget"], 100.0), "{b}");
    assert!(b["difficultyChange"].is_f64() && b["expectedBlocks"].is_f64());
    assert_eq!(hv(&a, "cache-control"), "public, max-age=1");
    assert_eq!(hv(&a, "x-bitcoin-chain-tip"), tip.block_hash().to_string());
    assert_eq!(hv(&a, "access-control-allow-origin"), "*");
    assert!(
        a.headers.get("age").is_none(),
        "a current answer has no age"
    );
    logged(|v| v["event"] == "difficulty_computed" && v["height"] == 3016).await;

    // Requests are answered from memory: no Esplora fetch, no Electrum request.
    let hits = esplora.hits.load(Ordering::SeqCst);
    assert_eq!(
        hits, 2,
        "the period's first block and the previous period's"
    );
    for _ in 0..100 {
        assert_eq!(get_answer(&app).await.status, StatusCode::OK);
    }
    assert_eq!(
        request(&app, Method::GET, &format!("{ROUTE}?cb=1"))
            .await
            .status,
        StatusCode::OK
    );
    let head = request(&app, Method::HEAD, ROUTE).await;
    assert_eq!((head.status, head.body), (StatusCode::OK, Value::Null));
    let post = request(&app, Method::POST, ROUTE).await;
    assert_eq!(post.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(post.body, json!({"error": "method not allowed"}));
    // A disabled network has no such route.
    let signet = request(&app, Method::GET, &format!("/signet{ROUTE}")).await;
    assert_eq!(signet.status, StatusCode::NOT_FOUND);
    peer.silent().await;
    assert_eq!(esplora.hits.load(Ordering::SeqCst), hits);
}

#[tokio::test]
async fn a_reorg_across_a_period_boundary_moves_the_answer() {
    let esplora = Esplora::default();
    let first = header(T0, LIMIT);
    esplora.add(2016, &first);
    esplora.add(0, &header(T0 - 1_209_600, LIMIT));
    let (app, electrum) = start(&esplora).await;

    // The tip is the period's last block, 4031; the window the client re-reads on a reorg is
    // the 144 blocks up to the new tip.
    let old = chain(BlockHash::all_zeros(), 143, T0 + 1872 * 600, 600, LIMIT, 1);
    let mut peer = Peer::accept(&electrum).await;
    peer.handshake(4031, &old[142]).await;
    let a = answer_at(&app, 4031).await;
    assert_eq!(a.body["nextRetargetHeight"], 4032);
    assert_eq!(a.body["remainingBlocks"], 1);

    // A competing branch replaces 4031 and adds 4032, the first block of the next period, at
    // twice the difficulty.
    let fork = chain(old[141].block_hash(), 1, T0 + 2015 * 600, 0, LIMIT, 2)[0];
    let next = chain(fork.block_hash(), 1, T0 + 2016 * 600, 0, DOUBLE, 3)[0];
    peer.push_tip(4032, &next).await;
    let req = peer.expect("blockchain.block.headers").await;
    assert_eq!(req["params"], json!([3889, 144]));
    let window: Vec<String> = old[..142]
        .iter()
        .chain([&fork, &next])
        .map(serialize_hex)
        .collect();
    peer.reply(&req, json!({"count": 144, "headers": window, "max": 2016}))
        .await;

    let b = answer_at(&app, 4032).await;
    assert_eq!(hv(&b, "x-bitcoin-chain-tip"), next.block_hash().to_string());
    assert_eq!(b.body["nextRetargetHeight"], 6048);
    assert_eq!(b.body["remainingBlocks"], 2016);
    assert_eq!(b.body["progressPercent"], 0.0);
    assert_eq!(b.body["previousTime"], T0 + 2016 * 600);
    // The new period's first block is the tip itself; only the previous period's was fetched.
    assert!(close(&b.body["previousRetarget"], 100.0), "{}", b.body);
    logged(|v| v["event"] == "difficulty_computed" && v["height"] == 4032).await;
}

#[tokio::test]
async fn the_last_answer_is_served_with_its_age_while_the_node_is_unavailable() {
    let esplora = Esplora::default();
    esplora.add(4032, &header(T0, LIMIT));
    esplora.add(2016, &header(T0 - 1_209_600, LIMIT));
    let (app, electrum) = start(&esplora).await;
    let tips = chain(BlockHash::all_zeros(), 2, T0 + 600, 600, LIMIT, 4);
    let mut peer = Peer::accept(&electrum).await;
    peer.handshake(4033, &tips[0]).await;
    answer_at(&app, 4033).await;

    // Esplora fails: a new tip cannot be computed, so the last answer stays, with an `Age`.
    esplora.down.store(true, Ordering::SeqCst);
    peer.push_tip(4034, &tips[1]).await;
    let failed = logged(|v| v["event"] == "difficulty_refresh_failed" && v["height"] == 4034).await;
    assert_eq!(failed["error"], "unavailable");
    assert_eq!(failed["failures"], 1);
    let stale = get_answer(&app).await;
    assert_eq!(stale.status, StatusCode::OK);
    assert_eq!(hv(&stale, "x-bitcoin-chain-tip-height"), "4033");
    assert!(stale.headers.get("age").is_some());

    // It comes back: the retry picks the new tip up without another block.
    esplora.down.store(false, Ordering::SeqCst);
    let fresh = answer_at(&app, 4034).await;
    assert!(fresh.headers.get("age").is_none());

    // The Electrum connection drops: the answer is still served, now with its age.
    peer.wr.shutdown().await.unwrap();
    drop(peer);
    let deadline = Instant::now() + WAIT;
    loop {
        let a = get_answer(&app).await;
        assert_eq!(a.status, StatusCode::OK);
        if a.headers.get("age").is_some() {
            assert_eq!(hv(&a, "x-bitcoin-chain-tip-height"), "4034");
            break;
        }
        assert!(Instant::now() < deadline, "no age after the node went away");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn blocks_read_at_another_tip_are_not_combined() {
    let esplora = Esplora::default();
    esplora.add(2016, &header(T0, LIMIT));
    esplora.add(0, &header(T0 - 1_209_600, LIMIT));
    let tip = header(T0 + 50 * 600, LIMIT);
    // Esplora has already moved to another branch when the task reads it.
    let other = header(T0 + 51 * 600, LIMIT).block_hash();
    *esplora.stamp.lock().unwrap() = Some((other, 2067));
    let (app, electrum) = start(&esplora).await;
    let mut peer = Peer::accept(&electrum).await;
    peer.handshake(2066, &tip).await;
    // Expected while blocks arrive: a debug-level retry, never the warning.
    let retried =
        logged(|v| v["event"] == "difficulty_refresh_retried" && v["height"] == 2066).await;
    assert_eq!(
        (retried["level"].as_str(), retried["retries"].as_u64()),
        (Some("debug"), Some(1))
    );
    assert_eq!(
        get_answer(&app).await.status,
        StatusCode::SERVICE_UNAVAILABLE
    );

    // Esplora agrees with the tip again: the retry computes it.
    *esplora.stamp.lock().unwrap() = Some((tip.block_hash(), 2066));
    let a = answer_at(&app, 2066).await;
    assert_eq!(a.body["remainingBlocks"], 1966);
    let raw = String::from_utf8(capture().0.lock().unwrap().clone()).unwrap();
    assert!(
        !raw.lines()
            .any(|l| l.contains("difficulty_refresh_failed") && l.contains("tip_moved")),
        "a moved tip is not a failure"
    );
}
