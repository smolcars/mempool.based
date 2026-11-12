//! The WebSocket hub end to end: the server on port 0 against a scripted Electrum server and a
//! mock Esplora, driven by a raw WebSocket client. Nothing here reaches a real node.

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

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use bitcoin::block::{Header, Version};
use bitcoin::consensus::encode::serialize_hex;
use bitcoin::hashes::Hash;
use bitcoin::{BlockHash, CompactTarget, TxMerkleNode};
use logging::{Format, Level, Settings};
use mempool_based::{App, Config, WsLimits};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::sync::oneshot;
use tokio::time::timeout;

const WAIT: Duration = Duration::from_secs(5);

// The node side.

/// Block JSON the mock Esplora serves, by hash. Unknown hashes are a 404.
type Blocks = Arc<Mutex<HashMap<String, Value>>>;

async fn mock_esplora(blocks: Blocks) -> String {
    async fn block(Path(hash): Path<String>, State(blocks): State<Blocks>) -> impl IntoResponse {
        match blocks.lock().unwrap().get(&hash) {
            Some(b) => (StatusCode::OK, b.to_string()).into_response(),
            None => (StatusCode::NOT_FOUND, "Block not found").into_response(),
        }
    }
    let app = axum::Router::new()
        .route("/block/{hash}", get(block))
        .with_state(blocks);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });
    url
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

    async fn send(&mut self, msg: Value) {
        self.wr
            .write_all(format!("{msg}\n").as_bytes())
            .await
            .unwrap();
    }

    async fn reply(&mut self, req: &Value, result: Value) {
        self.send(json!({"jsonrpc": "2.0", "id": req["id"], "result": result}))
            .await;
    }

    async fn handshake(&mut self, height: u32, tip: &Header) {
        let req = self.expect("server.version").await;
        self.reply(&req, json!(["rbitcoin-electrs 0.9.1", "1.6"]))
            .await;
        let req = self.expect("blockchain.headers.subscribe").await;
        self.reply(&req, tip_json(height, tip)).await;
    }

    async fn push_tip(&mut self, height: u32, tip: &Header) {
        self.send(
            json!({"jsonrpc": "2.0", "method": "blockchain.headers.subscribe",
            "params": [tip_json(height, tip)]}),
        )
        .await;
    }
}

fn tip_json(height: u32, header: &Header) -> Value {
    json!({"height": height, "hex": serialize_hex(header)})
}

fn chain(prev: BlockHash, len: u32, salt: u32) -> Vec<Header> {
    let mut out: Vec<Header> = Vec::new();
    for i in 0..len {
        out.push(Header {
            version: Version::TWO,
            prev_blockhash: out.last().map_or(prev, Header::block_hash),
            merkle_root: TxMerkleNode::all_zeros(),
            time: 1_767_225_600,
            bits: CompactTarget::from_consensus(0x207f_ffff),
            nonce: salt * 1000 + i,
        });
    }
    out
}

/// A running server with its mocks. Headers `h[i]` sit at height `i`; `h[2]` is the first tip.
struct Harness {
    addr: String,
    electrum: TcpListener,
    peer: Peer,
    blocks: Blocks,
    stop: Option<oneshot::Sender<()>>,
    served: tokio::task::JoinHandle<()>,
    sockets: Arc<mempool_based::ws::Sockets>,
    h: Vec<Header>,
}

impl Harness {
    async fn start(ws: WsLimits) -> Harness {
        Self::start_with(ws, Duration::from_secs(2)).await
    }

    async fn start_with(ws: WsLimits, request_timeout: Duration) -> Harness {
        let blocks = Blocks::default();
        let esplora = mock_esplora(Arc::clone(&blocks)).await;
        let electrum = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let vars: HashMap<String, String> = [
            ("REGTEST_RPC_HOST", "127.0.0.1".to_owned()),
            ("REGTEST_RPC_PORT", "1".to_owned()),
            ("REGTEST_RPC_USER", "u".to_owned()),
            ("REGTEST_RPC_PASSWORD", "p".to_owned()),
            ("REGTEST_ESPLORA_URL", esplora),
            (
                "REGTEST_ELECTRUM_ADDR",
                electrum.local_addr().unwrap().to_string(),
            ),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        let mut config = Config::from_vars(|k| vars.get(k).cloned()).unwrap();
        config.ws = ws;
        config.limits.request_timeout = request_timeout;
        config.limits.client_burst = 10_000;
        config.limits.client_rate_per_sec = 10_000;

        let h = chain(BlockHash::all_zeros(), 3, 0);
        for (height, header) in h.iter().enumerate() {
            add_block(&blocks, height as u32, header);
        }
        capture();
        let app = App::new(&config);
        let sockets = Arc::clone(app.sockets());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let (stop, stopped) = oneshot::channel::<()>();
        let served = tokio::spawn(app.serve(listener, async {
            let _ = stopped.await;
        }));
        let mut peer = Peer::accept(&electrum).await;
        peer.handshake(2, &h[2]).await;
        // Wait until the first tip's block is fetched and is the snapshot a subscriber gets.
        let deadline = Instant::now() + WAIT;
        loop {
            let mut probe = Ws::connect(&addr, "/regtest/app/ws").await;
            probe.json().await;
            probe
                .send(json!({"op": "subscribe", "topics": ["blocks"]}))
                .await;
            if timeout(Duration::from_millis(100), probe.json())
                .await
                .is_ok()
            {
                break;
            }
            assert!(Instant::now() < deadline, "no snapshot");
        }
        while sockets.open() > 0 {
            assert!(Instant::now() < deadline, "probe never closed");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Harness {
            addr,
            electrum,
            peer,
            blocks,
            stop: Some(stop),
            served,
            sockets,
            h,
        }
    }

    async fn connect(&self) -> Ws {
        Ws::connect(&self.addr, "/regtest/app/ws").await
    }

    /// Pushes a new tip whose block the mock Esplora knows.
    async fn mine(&mut self, height: u32, header: &Header) {
        add_block(&self.blocks, height, header);
        self.peer.push_tip(height, header).await;
    }
}

fn add_block(blocks: &Blocks, height: u32, header: &Header) {
    let id = header.block_hash().to_string();
    let block = json!({"id": id, "height": height,
        "previousblockhash": header.prev_blockhash.to_string(), "tx_count": 1});
    blocks.lock().unwrap().insert(id, block);
}

// The log. It is global, so every test here shares one capture and looks for lines only it
// can produce.

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

/// Waits until a logged event matches `f`, and returns it.
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

// The browser side.

/// A minimal WebSocket client that sends exactly what a test tells it to and never answers
/// pings on its own.
struct Ws {
    stream: TcpStream,
}

#[derive(Debug, PartialEq)]
enum Msg {
    Text(Vec<u8>),
    Ping,
    Close(u16, String),
    Other(u8),
}

const KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";

impl Ws {
    async fn connect(addr: &str, path: &str) -> Ws {
        Self::connect_stream(TcpStream::connect(addr).await.unwrap(), path)
            .await
            .unwrap()
    }

    /// The upgrade request; `Err` carries the status line of a refusal.
    async fn connect_stream(mut stream: TcpStream, path: &str) -> Result<Ws, String> {
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: {KEY}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        stream.write_all(req.as_bytes()).await.unwrap();
        // Byte by byte, so nothing after the head is consumed.
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            let mut b = [0u8];
            if timeout(WAIT, stream.read_exact(&mut b))
                .await
                .unwrap()
                .is_err()
            {
                break;
            }
            head.push(b[0]);
        }
        let head = String::from_utf8(head).unwrap();
        if !head.starts_with("HTTP/1.1 101") {
            return Err(head.lines().next().unwrap_or_default().to_owned());
        }
        assert!(
            head.contains("sec-websocket-accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),
            "{head}"
        );
        Ok(Ws { stream })
    }

    async fn send_raw(&mut self, bytes: &[u8]) {
        let _ = self.stream.write_all(bytes).await;
    }

    async fn send_frame(&mut self, b0: u8, payload: &[u8]) {
        self.send_raw(&masked(b0, payload)).await;
    }

    async fn send(&mut self, msg: Value) {
        self.send_frame(0x81, msg.to_string().as_bytes()).await;
    }

    /// The next frame, with its raw bytes.
    async fn recv_raw(&mut self) -> (Msg, Vec<u8>) {
        let mut head = [0u8; 2];
        timeout(WAIT, self.stream.read_exact(&mut head))
            .await
            .expect("timed out waiting for a frame")
            .expect("connection closed");
        assert_eq!(head[1] & 0x80, 0, "server frames are not masked");
        let len = match head[1] & 0x7F {
            126 => u64::from(self.stream.read_u16().await.unwrap()),
            127 => self.stream.read_u64().await.unwrap(),
            n => u64::from(n),
        };
        let mut payload = vec![0u8; len as usize];
        self.stream.read_exact(&mut payload).await.unwrap();
        let msg = match head[0] {
            0x81 => Msg::Text(payload.clone()),
            0x89 => Msg::Ping,
            0x88 => Msg::Close(
                u16::from_be_bytes([payload[0], payload[1]]),
                String::from_utf8(payload[2..].to_vec()).unwrap(),
            ),
            b => Msg::Other(b),
        };
        (msg, payload)
    }

    /// The next text frame as JSON and its exact payload bytes, skipping pings.
    async fn json_raw(&mut self) -> (Value, Vec<u8>) {
        loop {
            match self.recv_raw().await.0 {
                Msg::Text(t) => return (serde_json::from_slice(&t).unwrap(), t),
                Msg::Ping => {}
                other => panic!("expected text, got {other:?}"),
            }
        }
    }

    async fn json(&mut self) -> Value {
        self.json_raw().await.0
    }

    /// Reads until the close frame and returns its code and reason.
    async fn close_code(&mut self) -> (u16, String) {
        loop {
            if let Msg::Close(code, reason) = self.recv_raw().await.0 {
                return (code, reason);
            }
        }
    }

    async fn subscribe_blocks(&mut self) -> Value {
        self.send(json!({"op": "subscribe", "topics": ["blocks"]}))
            .await;
        self.json().await
    }
}

fn masked(b0: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![b0];
    match payload.len() {
        n if n < 126 => out.push(0x80 | n as u8),
        n if n <= 0xFFFF => {
            out.push(0x80 | 126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            out.push(0x80 | 127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    let key = [1, 2, 3, 4];
    out.extend_from_slice(&key);
    out.extend(payload.iter().zip(key.iter().cycle()).map(|(b, k)| b ^ k));
    out
}

fn limits() -> WsLimits {
    WsLimits::default()
}

// The tests.

#[tokio::test]
async fn greeting_then_snapshot_then_each_block_once_with_identical_bytes() {
    let mut s = Harness::start(limits()).await;
    let mut a = s.connect().await;
    let hello = a.json().await;
    assert_eq!(
        hello,
        json!({"type": "hello", "v": 1, "network": "regtest",
            "tip": {"height": 2, "hash": s.h[2].block_hash().to_string()}})
    );
    // Subscribing first sends the current state: the tip's block.
    let snapshot = a.subscribe_blocks().await;
    assert_eq!(snapshot["topic"], "blocks");
    assert_eq!(snapshot["type"], "block");
    assert_eq!(snapshot["block"]["height"], 2);

    let mut b = s.connect().await;
    b.json().await;
    b.subscribe_blocks().await;

    let next = chain(s.h[2].block_hash(), 1, 1)[0];
    s.mine(3, &next).await;
    let (msg, a_bytes) = a.json_raw().await;
    let (_, b_bytes) = b.json_raw().await;
    assert_eq!(a_bytes, b_bytes);
    assert_eq!(msg["block"]["id"], next.block_hash().to_string());
    assert_eq!(msg["block"]["height"], 3);
    // One fan-out of one frame; its payload is over 125 bytes, so the header is 4 bytes.
    let bytes = a_bytes.len() + 4;
    let fanout =
        logged(|v| v["event"] == "ws_fanout" && v["bytes"] == bytes && v["subscribers"] == 2).await;
    assert_eq!(fanout["network"], "regtest");
    assert_eq!(fanout["kind"], "block");
    assert_eq!(fanout["dropped_slow"], 0);

    // Nothing else was queued: the next message is the answer to a ping.
    for ws in [&mut a, &mut b] {
        ws.send(json!({"op": "ping"})).await;
        assert_eq!(ws.json().await, json!({"type": "pong"}));
    }
    // After unsubscribing, blocks stop.
    a.send(json!({"op": "unsubscribe", "topics": ["blocks"]}))
        .await;
    let after = chain(next.block_hash(), 1, 2)[0];
    s.mine(4, &after).await;
    assert_eq!(b.json().await["block"]["height"], 4);
    a.send(json!({"op": "ping"})).await;
    assert_eq!(a.json().await, json!({"type": "pong"}));
}

#[tokio::test]
async fn reorg_resync_and_missing_blocks_are_announced() {
    let mut s = Harness::start(limits()).await;
    let mut ws = s.connect().await;
    ws.json().await;
    ws.subscribe_blocks().await;

    // A competing block at height 2 replaces h[2].
    let fork = chain(s.h[1].block_hash(), 1, 7)[0];
    s.mine(2, &fork).await;
    let req = s.peer.expect("blockchain.block.headers").await;
    let hexes: Vec<String> = [s.h[0], s.h[1], fork].iter().map(serialize_hex).collect();
    s.peer
        .reply(&req, json!({"count": 3, "headers": hexes, "max": 2016}))
        .await;
    assert_eq!(
        ws.json().await,
        json!({"topic": "blocks", "type": "reorg", "fork_height": 1})
    );
    let msg = ws.json().await;
    assert_eq!(msg["type"], "block");
    assert_eq!(msg["block"]["id"], fork.block_hash().to_string());

    // A block Esplora cannot serve: clients are told to resync instead.
    let unknown = chain(fork.block_hash(), 1, 8)[0];
    s.peer.push_tip(3, &unknown).await;
    let msg = ws.json().await;
    assert_eq!(msg["type"], "resync");
    assert_eq!(msg["tip"]["height"], 3);
    // A later subscriber gets that resync as the snapshot, not the block before it.
    let mut late = s.connect().await;
    late.json().await;
    assert_eq!(late.subscribe_blocks().await, msg);

    // The node connection drops and comes back: the client reconnects and clients resync.
    s.peer.wr.shutdown().await.unwrap();
    s.peer = Peer::accept(&s.electrum).await;
    s.peer.handshake(3, &unknown).await;
    let msg = ws.json().await;
    assert_eq!(
        msg,
        json!({"topic": "blocks", "type": "resync",
            "tip": {"height": 3, "hash": unknown.block_hash().to_string()}})
    );
}

#[tokio::test]
async fn each_message_limit_closes_with_its_code() {
    let ws_limits = WsLimits {
        max_message_bytes: 64,
        messages_per_sec: 5,
        ..limits()
    };
    let s = Harness::start(ws_limits).await;
    let cases: Vec<(&[u8], u16)> = vec![
        (&[0x81], 1009),       // a text frame over 64 bytes, refused from its header
        (&[0x82], 1003),       // binary
        (&[0x81, 0xFF], 1007), // invalid UTF-8
        (&[0x81, b'{'], 4000), // not JSON
        (&[0x01, b'x'], 1002), // a new message inside a fragmented one
    ];
    for (bytes, code) in cases {
        let mut ws = s.connect().await;
        ws.json().await;
        match (code, bytes) {
            (1009, _) => ws.send_frame(0x81, &[b' '; 65]).await,
            (1002, _) => {
                ws.send_frame(0x01, b"{").await;
                ws.send_frame(0x81, b"}").await;
            }
            (_, [b0, rest @ ..]) => ws.send_frame(*b0, rest).await,
            _ => unreachable!(),
        }
        assert_eq!(ws.close_code().await.0, code, "{bytes:?}");
    }

    // Unknown topic, unmasked frame.
    let mut ws = s.connect().await;
    ws.json().await;
    ws.send(json!({"op": "subscribe", "topics": ["mempool-everything"]}))
        .await;
    assert_eq!(ws.close_code().await, (4000, "bad message".to_owned()));
    let mut ws = s.connect().await;
    ws.json().await;
    ws.send_raw(&[0x81, 0x01, b'x']).await;
    assert_eq!(ws.close_code().await.0, 1002);

    // A fragmented message is reassembled, and its total size is capped too.
    let mut ws = s.connect().await;
    ws.json().await;
    ws.send_frame(0x01, br#"{"op":"#).await;
    ws.send_frame(0x80, br#""ping"}"#).await;
    assert_eq!(ws.json().await, json!({"type": "pong"}));
    ws.send_frame(0x01, &[b' '; 40]).await;
    ws.send_frame(0x80, &[b' '; 40]).await;
    assert_eq!(ws.close_code().await.0, 1009);

    // Five messages a second are fine; a burst past that is not.
    let mut ws = s.connect().await;
    ws.json().await;
    for _ in 0..5 {
        ws.send(json!({"op": "ping"})).await;
    }
    for _ in 0..5 {
        assert_eq!(ws.json().await, json!({"type": "pong"}));
    }
    ws.send(json!({"op": "ping"})).await;
    assert_eq!(
        ws.close_code().await,
        (4002, "too many messages".to_owned())
    );

    // A client close is answered with a close.
    let mut ws = s.connect().await;
    ws.json().await;
    ws.send_frame(0x88, &1000u16.to_be_bytes()).await;
    assert_eq!(ws.close_code().await.0, 1000);
}

#[tokio::test]
async fn socket_caps_refuse_the_upgrade() {
    let s = Harness::start(WsLimits {
        max_sockets: 3,
        client_max_sockets: 2,
        ..limits()
    })
    .await;
    let connect = |addr: String| async move {
        let stream = TcpStream::connect(addr).await.unwrap();
        Ws::connect_stream(stream, "/regtest/app/ws").await
    };
    let _a = connect(s.addr.clone()).await.unwrap();
    let b = connect(s.addr.clone()).await.unwrap();
    assert_eq!(
        connect(s.addr.clone()).await.err().unwrap(),
        "HTTP/1.1 429 Too Many Requests"
    );
    // Another address (127.0.0.2 is loopback on Linux; macOS needs an alias) has its own cap,
    // but the total of 3 still applies.
    let other = TcpSocket::new_v4().unwrap();
    if other.bind("127.0.0.2:0".parse().unwrap()).is_ok() {
        let other_addr = s.addr.clone();
        let stream = other.connect(other_addr.parse().unwrap()).await.unwrap();
        let _c = Ws::connect_stream(stream, "/regtest/app/ws").await.unwrap();
        let other = TcpSocket::new_v4().unwrap();
        other.bind("127.0.0.2:0".parse().unwrap()).unwrap();
        let stream = other.connect(s.addr.parse().unwrap()).await.unwrap();
        assert_eq!(
            Ws::connect_stream(stream, "/regtest/app/ws")
                .await
                .err()
                .unwrap(),
            "HTTP/1.1 503 Service Unavailable"
        );
    }
    // A closed socket frees its slot.
    drop(b);
    let deadline = Instant::now() + WAIT;
    while connect(s.addr.clone()).await.is_err() {
        assert!(Instant::now() < deadline, "slot never freed");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Not an upgrade, or a bad key: 400 before any slot is taken.
    let mut stream = TcpStream::connect(&s.addr).await.unwrap();
    stream
        .write_all(b"GET /regtest/app/ws HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).await.unwrap();
    assert!(reply.starts_with("HTTP/1.1 400"), "{reply}");
    assert!(reply.contains("websocket upgrade required"), "{reply}");
}

#[tokio::test]
async fn a_slow_reader_is_dropped_while_others_keep_receiving() {
    let mut s = Harness::start(WsLimits {
        send_queue: 2,
        ..limits()
    })
    .await;
    // Big blocks, and a slow client with a tiny receive window, so its buffers fill fast.
    let pad = "x".repeat(256 * 1024);
    let slow_socket = TcpSocket::new_v4().unwrap();
    slow_socket.set_recv_buffer_size(4096).unwrap();
    let stream = slow_socket.connect(s.addr.parse().unwrap()).await.unwrap();
    let mut slow = Ws::connect_stream(stream, "/regtest/app/ws").await.unwrap();
    slow.json().await;
    slow.subscribe_blocks().await;
    let mut fast = s.connect().await;
    fast.json().await;
    fast.subscribe_blocks().await;

    let blocks = chain(s.h[2].block_hash(), 40, 3);
    for (i, header) in blocks.iter().enumerate() {
        let height = 3 + i as u32;
        let id = header.block_hash().to_string();
        s.blocks
            .lock()
            .unwrap()
            .insert(id.clone(), json!({"id": id, "height": height, "pad": pad}));
        s.peer.push_tip(height, header).await;
        // The fast client gets every block, in order, while the slow one falls behind.
        assert_eq!(fast.json().await["block"]["height"], height);
    }
    let dropped = logged(|v| v["event"] == "ws_clients_dropped" && v["reason"] == "too_slow").await;
    assert_eq!(dropped["total"], 1, "the slow client is dropped once");
    logged(|v| v["event"] == "ws_closed" && v["code"] == 4003).await;
    drop(slow);
}

#[tokio::test]
async fn quiet_clients_are_pinged_then_closed() {
    let s = Harness::start(WsLimits {
        idle_timeout: Duration::from_secs(1),
        ..limits()
    })
    .await;
    let mut silent = s.connect().await;
    let mut polite = s.connect().await;
    silent.json().await;
    polite.json().await;
    let started = Instant::now();
    // Answering every ping keeps a socket open across several idle periods.
    let polite = tokio::spawn(async move {
        let mut pings = 0;
        while started.elapsed() < Duration::from_millis(3500) {
            if let Ok((Msg::Ping, _)) = timeout(Duration::from_millis(100), polite.recv_raw()).await
            {
                pings += 1;
                polite.send_frame(0x8A, b"").await;
            }
        }
        polite.send(json!({"op": "ping"})).await;
        assert_eq!(polite.json().await, json!({"type": "pong"}));
        pings
    });
    assert_eq!(silent.recv_raw().await.0, Msg::Ping);
    assert_eq!(silent.close_code().await, (4004, "idle timeout".to_owned()));
    let closed_after = started.elapsed();
    assert!(
        closed_after >= Duration::from_secs(1) && closed_after < Duration::from_millis(1600),
        "{closed_after:?}"
    );
    assert!(polite.await.unwrap() >= 3);
}

// Several workers, so connection tasks and `serve` race as they do in the binary.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_closes_sockets_within_the_deadline() {
    let mut s = Harness::start_with(limits(), Duration::from_secs(1)).await;
    let mut reading = s.connect().await;
    reading.json().await;
    // Never reads and never answers the close: shutdown must not wait on it past the deadline.
    let mut stubborn = s.connect().await;
    stubborn.json().await;

    let started = Instant::now();
    s.stop.take().unwrap().send(()).unwrap();
    assert_eq!(
        reading.close_code().await,
        (1001, "server shutting down".to_owned())
    );
    timeout(Duration::from_secs(3), &mut s.served)
        .await
        .expect("shutdown overran its deadline")
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    // Every socket's task has ended and released its slot, not only the wait.
    assert_eq!(s.sockets.open(), 0);
    assert_eq!(stubborn.close_code().await.0, 1001);
}
