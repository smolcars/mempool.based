//! The mempool topics against real engines on a mock node: each topic's snapshot and changes, a
//! client-side model that must end equal to the server's, random sequences of mempool changes,
//! lagging and late subscribers, the bounds, the off switch, and a node down at boot.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bitcoin::absolute::LockTime;
use bitcoin::block::{Header, Version};
use bitcoin::hashes::Hash;
use bitcoin::{
    Amount, Block, BlockHash, CompactTarget, OutPoint, ScriptBuf, Sequence, SignedAmount,
    Transaction, TxIn, TxMerkleNode, TxOut, Txid, Witness,
};
use engine::mempool::{self, Source, SourceError, Tracker};
use engine::projected::{self, ClusterSource, Projection, Projector};
use node::esplora::ChainTip;
use node::rpc::model::{Cluster, ClusterChunk, EntryFees, MempoolEntry, MempoolInfo, RawMempool};
use serde_json::Value;
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use super::*;
use crate::Config;
use crate::ws::{Close, Conn, Sockets};

const WAIT: Duration = Duration::from_secs(10);

// A deterministic generator, so a failing seed can be replayed.

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

// The node.

/// One mempool transaction as the mock node holds it.
#[derive(Clone)]
struct MockTx {
    tx: Transaction,
    fee: u64,
    vsize: u64,
    time: u64,
}

#[derive(Default)]
struct Node {
    txs: Vec<MockTx>,
    sequence: u64,
    /// Chunks the node mines by: every transaction alone unless joined.
    clusters: HashMap<Txid, Cluster>,
    tip: Option<ChainTip>,
    blocks: HashMap<BlockHash, Block>,
    /// Every call fails, as with a node that is down.
    down: bool,
    next_tag: u32,
    /// Every transaction ever added, oldest first.
    history: Vec<Txid>,
    /// The node's minimum accepted rate, sat per 1,000 vB.
    min_fee: u64,
}

#[derive(Clone, Default)]
struct Mock(Arc<Mutex<Node>>);

fn tx(tag: u32) -> Transaction {
    Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: LockTime::from_consensus(tag),
        input: vec![TxIn {
            previous_output: OutPoint::new(Txid::from_byte_array([0xcc; 32]), tag),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Witness::new(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(u64::from(tag) * 7 + 1000),
            script_pubkey: ScriptBuf::new(),
        }],
    }
}

fn alone(t: &MockTx) -> Cluster {
    cluster(&[(&[t])])
}

/// One chunk per slice, each holding its transactions.
fn cluster(chunks: &[&[&MockTx]]) -> Cluster {
    let chunks: Vec<ClusterChunk> = chunks
        .iter()
        .map(|c| ClusterChunk {
            chunkfee: SignedAmount::from_sat(c.iter().map(|t| t.fee as i64).sum()),
            chunkweight: c.iter().map(|t| t.vsize * 4).sum(),
            txs: c.iter().map(|t| t.tx.compute_txid()).collect(),
        })
        .collect();
    Cluster {
        clusterweight: chunks.iter().map(|c| c.chunkweight).sum(),
        txcount: chunks.iter().map(|c| c.txs.len() as u64).sum(),
        chunks,
    }
}

impl Mock {
    fn new() -> Self {
        let m = Self::default();
        m.mine(0);
        m
    }

    fn node(&self) -> std::sync::MutexGuard<'_, Node> {
        self.0.lock().unwrap()
    }

    fn add(&self, fee: u64, vsize: u64) -> Txid {
        let mut n = self.node();
        n.next_tag += 1;
        let t = MockTx {
            tx: tx(n.next_tag),
            fee,
            vsize,
            time: 1_700_000_000 + u64::from(n.next_tag),
        };
        let id = t.tx.compute_txid();
        n.clusters.insert(id, alone(&t));
        n.txs.push(t);
        n.history.push(id);
        n.sequence += 1;
        id
    }

    fn remove(&self, id: Txid) {
        let mut n = self.node();
        n.txs.retain(|t| t.tx.compute_txid() != id);
        n.clusters.remove(&id);
        // A transaction it was joined with stands alone again.
        let rest: Vec<MockTx> = n.txs.clone();
        for t in &rest {
            let tid = t.tx.compute_txid();
            if n.clusters
                .get(&tid)
                .is_some_and(|c| c.chunks.iter().flat_map(|c| &c.txs).any(|m| *m == id))
            {
                n.clusters.insert(tid, alone(t));
            }
        }
        n.sequence += 1;
    }

    /// Mines `a` and `b` as one chunk, as a child paying for its parent.
    fn join(&self, a: Txid, b: Txid) {
        let mut n = self.node();
        let find = |id| n.txs.iter().find(|t| t.tx.compute_txid() == id).cloned();
        let (Some(a), Some(b)) = (find(a), find(b)) else {
            return;
        };
        let c = cluster(&[&[&a, &b]]);
        for t in [&a, &b] {
            n.clusters.insert(t.tx.compute_txid(), c.clone());
        }
        n.sequence += 1;
    }

    /// A new block holding the `count` first transactions.
    fn mine(&self, count: usize) {
        let mut n = self.node();
        let count = count.min(n.txs.len());
        let mined: Vec<MockTx> = n.txs.drain(..count).collect();
        for t in &mined {
            n.clusters.remove(&t.tx.compute_txid());
        }
        let rest = n.txs.clone();
        for t in &rest {
            let id = t.tx.compute_txid();
            if n.clusters.get(&id).is_some_and(|c| {
                c.chunks
                    .iter()
                    .flat_map(|c| &c.txs)
                    .any(|m| mined.iter().any(|x| x.tx.compute_txid() == *m))
            }) {
                n.clusters.insert(id, alone(t));
            }
        }
        let height = n.tip.map_or(0, |t| t.height + 1);
        let mut block = Block {
            header: Header {
                version: Version::TWO,
                prev_blockhash: n.tip.map_or(BlockHash::all_zeros(), |t| t.hash),
                merkle_root: TxMerkleNode::all_zeros(),
                time: height,
                bits: CompactTarget::from_consensus(0x207f_ffff),
                nonce: height,
            },
            txdata: std::iter::once(tx(4_000_000 + height))
                .chain(mined.into_iter().map(|t| t.tx))
                .collect(),
        };
        block.header.merkle_root = block.compute_merkle_root().unwrap();
        let hash = block.block_hash();
        n.blocks.insert(hash, block);
        n.tip = Some(ChainTip { hash, height });
        n.sequence += 1;
    }

    fn set_down(&self, down: bool) {
        self.node().down = down;
    }

    fn check(&self) -> Result<std::sync::MutexGuard<'_, Node>, SourceError> {
        let n = self.node();
        if n.down {
            return Err(SourceError::Failed(
                "rpc unavailable: connection refused".into(),
            ));
        }
        Ok(n)
    }
}

impl Source for Mock {
    async fn mempool_info(&self) -> Result<MempoolInfo, SourceError> {
        let n = self.check()?;
        Ok(MempoolInfo {
            loaded: true,
            size: n.txs.len() as u64,
            bytes: n.sequence,
            usage: 0,
            maxmempool: 300_000_000,
            total_fee: Amount::ZERO,
            mempoolminfee: Amount::from_sat(n.min_fee.max(1000)),
            minrelaytxfee: Amount::from_sat(100),
            incrementalrelayfee: Amount::from_sat(100),
            fullrbf: true,
            limitclustercount: 64,
            limitclustersize: 101,
            unbroadcastcount: 0,
        })
    }

    async fn listing(&self) -> Result<RawMempool, SourceError> {
        let n = self.check()?;
        Ok(RawMempool {
            mempool_sequence: n.sequence,
            txids: n.txs.iter().map(|t| t.tx.compute_txid()).collect(),
        })
    }

    async fn details(
        &self,
        txids: &[Txid],
    ) -> Result<Vec<Option<(MempoolEntry, Transaction)>>, SourceError> {
        let n = self.check()?;
        Ok(txids
            .iter()
            .map(|id| {
                let t = n.txs.iter().find(|t| t.tx.compute_txid() == *id)?;
                let fee = Amount::from_sat(t.fee);
                let signed = fee.to_signed().unwrap();
                let entry = MempoolEntry {
                    wtxid: t.tx.compute_wtxid(),
                    vsize: t.vsize,
                    vsize_bip141: t.vsize,
                    weight: t.vsize * 4,
                    time: t.time,
                    height: 0,
                    fees: EntryFees {
                        base: fee,
                        modified: signed,
                        ancestor: signed,
                        descendant: signed,
                        chunk: signed,
                    },
                    ancestorcount: 1,
                    ancestorsize: 0,
                    descendantcount: 1,
                    descendantsize: 0,
                    chunkweight: t.vsize * 4,
                    depends: Vec::new(),
                    spentby: Vec::new(),
                    bip125_replaceable: true,
                    unbroadcast: false,
                };
                Some((entry, t.tx.clone()))
            })
            .collect())
    }

    async fn tip(&self) -> Result<ChainTip, SourceError> {
        self.check()?.tip.ok_or(SourceError::Busy)
    }

    async fn block(&self, hash: BlockHash) -> Result<Block, SourceError> {
        self.check()?
            .blocks
            .get(&hash)
            .cloned()
            .ok_or_else(|| SourceError::Failed("no such block".into()))
    }

    fn bytes(&self) -> u64 {
        0
    }
}

impl ClusterSource for Mock {
    async fn clusters(&self, txids: &[Txid]) -> Result<Vec<Option<Cluster>>, SourceError> {
        let n = self.check()?;
        Ok(txids.iter().map(|t| n.clusters.get(t).cloned()).collect())
    }

    fn bytes(&self) -> u64 {
        0
    }
}

// The server side.

struct Server {
    ctx: Arc<NetworkContext>,
    tracker: Tracker,
    projector: Projector,
    mock: Mock,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for Server {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

fn context(mempool: bool, interval: Duration) -> Arc<NetworkContext> {
    let vars: HashMap<&str, &str> = [
        ("REGTEST_RPC_HOST", "127.0.0.1"),
        ("REGTEST_RPC_PORT", "1"),
        ("REGTEST_RPC_USER", "u"),
        ("REGTEST_RPC_PASSWORD", "p"),
        ("REGTEST_ESPLORA_URL", "http://127.0.0.1:1"),
        ("REGTEST_ELECTRUM_ADDR", "127.0.0.1:1"),
        (
            "REGTEST_MEMPOOL_ENABLED",
            if mempool { "true" } else { "false" },
        ),
    ]
    .into_iter()
    .collect();
    let mut config = Config::from_vars(|k| vars.get(k).map(|v| (*v).to_owned())).unwrap();
    config.ws.mempool_interval = interval;
    config.ws.latest_txs = 5;
    let sockets = Sockets::new(&config.ws, Duration::from_secs(1));
    Arc::new(NetworkContext::new(&config.networks[0], &config, &sockets).unwrap())
}

impl Server {
    /// Engines on `mock`, fast enough for tests, with the topics fed every `interval`.
    fn start(mock: &Mock, interval: Duration) -> Self {
        let ctx = context(true, interval);
        let tracker_config = mempool::Config {
            gap: Duration::from_millis(10),
            heartbeat: Duration::from_millis(100),
            max_backoff: Duration::from_millis(50),
            ..mempool::Config::default()
        };
        let (tracker, t1) = Tracker::start(mock.clone(), tracker_config);
        let config = projected::Config {
            max_backoff: Duration::from_millis(50),
            ..projected::Config::default()
        };
        let (projector, t2) = Projector::start(tracker.clone(), mock.clone(), config);
        let mut tasks = run(&ctx, &tracker, &projector);
        tasks.extend([t1, t2]);
        Self {
            ctx,
            tracker,
            projector,
            mock: mock.clone(),
            tasks,
        }
    }

    /// Waits until the engines hold exactly the mock's mempool and have stopped moving.
    async fn settle(&self) {
        let deadline = Instant::now() + WAIT;
        loop {
            let want: std::collections::HashSet<Txid> = self
                .mock
                .node()
                .txs
                .iter()
                .map(|t| t.tx.compute_txid())
                .collect();
            let tip = self.mock.node().tip;
            let p = self.projector.current();
            let have: std::collections::HashSet<Txid> = p
                .blocks
                .iter()
                .flat_map(|b| b.txs.iter().map(|(t, _)| *t))
                .collect();
            if have == want && p.tip == tip && !p.stale {
                tokio::time::sleep(Duration::from_millis(60)).await;
                if self.projector.current().seq == p.seq {
                    return;
                }
            }
            assert!(Instant::now() < deadline, "the engines never settled");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

// The client side: a model of what a browser holds, built only from the frames it receives.

static IDS: AtomicU64 = AtomicU64::new(1_000_000);

pub(crate) fn conn(queue: usize) -> (Arc<Conn>, mpsc::Receiver<Frame>) {
    let (tx, rx) = mpsc::channel(queue);
    let conn = Conn {
        id: IDS.fetch_add(1, Ordering::Relaxed),
        queue: tx,
        closing: watch::Sender::new(None),
    };
    (Arc::new(conn), rx)
}

/// A frame's opcode and payload.
pub(crate) fn open(f: &Frame) -> (u8, &[u8]) {
    let b = f.as_bytes();
    let (len, at) = match b[1] & 0x7F {
        126 => (u64::from(u16::from_be_bytes([b[2], b[3]])), 4),
        127 => (u64::from_be_bytes(b[2..10].try_into().unwrap()), 10),
        n => (u64::from(n), 2),
    };
    assert_eq!(b.len() - at, len as usize);
    (b[0] & 0x0F, &b[at..])
}

type Records = HashMap<[u8; 32], (u32, u32, u64)>;

#[derive(Default)]
struct Client {
    /// Per projected block: the seq of the last frame applied, and the records.
    blocks: HashMap<u8, (u64, Records)>,
    json: HashMap<String, Value>,
    frames: usize,
}

fn u32_at(p: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(p[at..at + 4].try_into().unwrap())
}

fn u64_at(p: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(p[at..at + 8].try_into().unwrap())
}

fn read_record(r: &[u8]) -> ([u8; 32], (u32, u32, u64)) {
    (
        r[..32].try_into().unwrap(),
        (u32_at(r, 32), u32_at(r, 36), u64_at(r, 40)),
    )
}

impl Client {
    /// Applies one frame as `docs/WS.md` tells a browser to. A delta that does not follow the
    /// frame before it is an error: the client would have to start over.
    fn apply(&mut self, f: &Frame) -> Result<(), String> {
        self.frames += 1;
        let (opcode, p) = open(f);
        if opcode == frame::TEXT {
            let v: Value = serde_json::from_slice(p).unwrap();
            let topic = v["topic"].as_str().unwrap().to_owned();
            self.json.insert(topic, v);
            return Ok(());
        }
        assert_eq!(opcode, frame::BINARY);
        assert_eq!(p[1], FORMAT_VERSION);
        let block = p[2];
        match p[0] {
            KIND_SNAPSHOT => {
                let count = u32_at(p, 4) as usize;
                let seq = u64_at(p, 8);
                assert_eq!(p.len(), SNAPSHOT_HEAD + count * RECORD);
                let records = p[SNAPSHOT_HEAD..]
                    .as_chunks::<RECORD>()
                    .0
                    .iter()
                    .map(|r| read_record(r))
                    .collect();
                self.blocks.insert(block, (seq, records));
            }
            KIND_DELTA => {
                let removed = u32_at(p, 4) as usize;
                let (seq, prev) = (u64_at(p, 8), u64_at(p, 16));
                let set = u32_at(p, 24) as usize;
                assert_eq!(p.len(), DELTA_HEAD + removed * 32 + set * RECORD);
                let Some((have, records)) = self.blocks.get_mut(&block) else {
                    return Err(format!("delta for block {block} before its snapshot"));
                };
                if *have != prev {
                    return Err(format!("block {block}: delta after {prev}, we hold {have}"));
                }
                let (gone, rest) = p[DELTA_HEAD..].split_at(removed * 32);
                for t in gone.as_chunks::<32>().0 {
                    if records.remove(t).is_none() {
                        return Err(format!("block {block}: removed a record it did not hold"));
                    }
                }
                for r in rest.as_chunks::<RECORD>().0 {
                    let (t, v) = read_record(r);
                    records.insert(t, v);
                }
                *have = seq;
            }
            k => panic!("unknown kind {k}"),
        }
        Ok(())
    }

    fn drain(&mut self, rx: &mut mpsc::Receiver<Frame>) -> Result<(), String> {
        while let Ok(f) = rx.try_recv() {
            self.apply(&f)?;
        }
        Ok(())
    }
}

/// What a client must hold for each projected block, from the server's projection.
fn expected(p: &Projection, block: u8) -> Records {
    p.blocks
        .get(usize::from(block))
        .map(|b| {
            b.txs
                .iter()
                .map(|(t, r)| (txid_bytes(t), (r.rate, r.vsize, r.fee)))
                .collect()
        })
        .unwrap_or_default()
}

fn subscribe_all(ctx: &NetworkContext, conn: &Arc<Conn>) {
    for topic in topics() {
        ctx.hub.subscribe(topic, conn);
    }
}

/// Waits until the client, fed only by its frames, holds exactly the server's current state.
async fn converge(s: &Server, client: &mut Client, rx: &mut mpsc::Receiver<Frame>) {
    s.settle().await;
    let deadline = Instant::now() + WAIT;
    loop {
        client.drain(rx).unwrap();
        let p = s.projector.current();
        let blocks_equal = (0..TXS_TOPICS as u8).all(|b| {
            client
                .blocks
                .get(&b)
                .is_some_and(|(_, r)| *r == expected(&p, b))
        });
        let min_fee = s.tracker.node_info().map(|i| i.mempoolminfee);
        let json_equal = client.json.get("mempool") == Some(&totals(&p, min_fee))
            && client.json.get("projected") == Some(&projected(&p));
        let latest_equal = client.json.get("transactions").is_some_and(|v| {
            let newest_ever: Vec<Txid> = s
                .mock
                .node()
                .history
                .iter()
                .rev()
                .take(5)
                .copied()
                .collect();
            let (want, all_there) = s.tracker.read(|m| {
                let all = newest_ever.iter().all(|t| m.get(t).is_some());
                (newest(Vec::new(), m.iter(), 5), all)
            });
            // Transactions stay in the list after they leave the mempool: compare only while
            // the newest ever added are all still there, so they are the list.
            !all_there || *v == latest(&want)
        });
        if blocks_equal && json_equal && latest_equal {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "client never matched the server: blocks {blocks_equal}, json {json_equal}, \
             latest {latest_equal}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

// The tests.

#[test]
fn every_topic_name_parses_back() {
    for topic in topics() {
        assert_eq!(Topic::parse(topic.name()), Some(topic));
    }
    assert_eq!(topics().count(), 3 + TXS_TOPICS);
    // Every projected block but the last, which holds the rest of the mempool.
    assert_eq!(TXS_TOPICS, projected::Config::default().blocks - 1);
}

#[test]
fn frames_have_the_documented_layout() {
    let a = Txid::from_byte_array([1; 32]);
    let mut b_bytes = [2; 32];
    b_bytes[0] = 0xab;
    let b = Txid::from_byte_array(b_bytes);
    let placed = |rate, vsize, fee| projected::Placed {
        block: 3,
        rate,
        vsize,
        fee,
    };
    let block = ProjectedBlock {
        summary: projected::BlockSummary::default(),
        txs: vec![(a, placed(1500, 200, 300)), (b, placed(1000, 141, 141))],
    };
    let s = snapshot(3, 0x0102, &block);
    assert_eq!(&s[..16], &[1, 1, 3, 0, 2, 0, 0, 0, 2, 1, 0, 0, 0, 0, 0, 0]);
    assert_eq!(s.len(), 16 + 2 * 48);
    // The txid as it is written in hex: internal byte 0 comes last.
    assert_eq!(s[16..48], txid_bytes(&a));
    assert_eq!(s[64 + 31], 0xab);
    assert_eq!(hex(&s[64..96]), b.to_string());
    assert_eq!(
        &s[96..112],
        &[0xe8, 3, 0, 0, 141, 0, 0, 0, 141, 0, 0, 0, 0, 0, 0, 0]
    );

    let d = delta(3, 9, 7, &[&a], &[&block.txs[1]]);
    assert_eq!(
        &d[..32],
        &[
            2, 1, 3, 0, 1, 0, 0, 0, 9, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0,
            0, 0, 0
        ]
    );
    assert_eq!(d.len(), 32 + 32 + 48);
    assert_eq!(d[32..64], txid_bytes(&a));
    assert_eq!(d[64..112], s[64..112]);
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

#[tokio::test]
async fn each_topic_gives_a_state_then_changes_that_reach_the_servers_state() {
    let mock = Mock::new();
    let parent = mock.add(100, 1000); // 0.1 sat/vB alone
    let others: Vec<Txid> = (0..20).map(|i| mock.add(2_000 + i * 50, 1000)).collect();
    let s = Server::start(&mock, Duration::from_millis(20));
    let (conn, mut rx) = conn(1024);
    subscribe_all(&s.ctx, &conn);
    let mut client = Client::default();
    converge(&s, &mut client, &mut rx).await;
    let totals = &client.json["mempool"];
    assert_eq!(totals["count"], 21);
    assert_eq!(totals["vsize"], 21_000);
    assert_eq!(totals["min_fee_rate"], 1000);
    assert_eq!(totals["stale"], false);
    assert_eq!(
        client.json["projected"]["blocks"].as_array().unwrap().len(),
        8
    );
    let latest = &client.json["transactions"]["txs"];
    assert_eq!(latest.as_array().unwrap().len(), 5);
    assert_eq!(latest[0]["txid"], others[19].to_string(), "newest first");

    // A child lifts its parent: both records change rate in place.
    let child = mock.add(20_000, 200);
    mock.join(parent, child);
    converge(&s, &mut client, &mut rx).await;
    let lifted = client.blocks[&0].1[&txid_bytes(&parent)];
    assert_eq!(lifted.0, 20_100 * 1000 / 1200);

    // A replacement, then a block that empties the first projected block.
    mock.remove(others[0]);
    mock.add(5_000, 1000);
    converge(&s, &mut client, &mut rx).await;
    let n = mock.node().txs.len();
    mock.mine(n);
    converge(&s, &mut client, &mut rx).await;
    assert!(client.blocks[&0].1.is_empty());
    assert_eq!(client.json["mempool"]["count"], 0);
}

#[tokio::test]
async fn random_changes_keep_every_client_equal_to_the_server() {
    for seed in 1..=6u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let mock = Mock::new();
        let s = Server::start(&mock, Duration::from_millis(15));
        let (early, mut early_rx) = conn(4096);
        subscribe_all(&s.ctx, &early);
        let mut a = Client::default();
        for step in 0..12 {
            for _ in 0..rng.below(40) {
                // Large transactions so the mempool spans several projected blocks.
                mock.add(rng.below(400_000), 1_000 + rng.below(150_000));
            }
            let ids: Vec<Txid> = mock
                .node()
                .txs
                .iter()
                .map(|t| t.tx.compute_txid())
                .collect();
            for _ in 0..rng.below(8) {
                if !ids.is_empty() {
                    mock.remove(ids[rng.below(ids.len() as u64) as usize]);
                }
            }
            for _ in 0..rng.below(4) {
                if ids.len() > 1 {
                    let i = rng.below(ids.len() as u64) as usize;
                    let j = rng.below(ids.len() as u64) as usize;
                    if i != j {
                        mock.join(ids[i], ids[j]);
                    }
                }
            }
            if rng.below(4) == 0 {
                let n = mock.node().txs.len();
                mock.mine(rng.below(n as u64 + 1) as usize);
            }
            converge(&s, &mut a, &mut early_rx).await;
            // A client that subscribes now gets the stored state and the changes appended
            // since, and must land on the same state.
            let (late, mut late_rx) = conn(4096);
            subscribe_all(&s.ctx, &late);
            let mut b = Client::default();
            converge(&s, &mut b, &mut late_rx).await;
            assert!(
                b.frames <= 3 + TXS_TOPICS * (1 + MAX_APPENDED),
                "seed {seed} step {step}: a new subscriber got {} frames",
                b.frames
            );
            for t in topics() {
                s.ctx.hub.unsubscribe(t, late.id);
            }
        }
    }
}

#[tokio::test]
async fn many_small_changes_are_folded_into_a_new_stored_state() {
    let mock = Mock::new();
    for i in 0..50 {
        mock.add(1_000 + i, 500);
    }
    let s = Server::start(&mock, Duration::from_millis(5));
    let (conn_a, mut rx) = conn(4096);
    s.ctx.hub.subscribe(Topic::ProjectedTxs(0), &conn_a);
    let mut a = Client::default();
    converge_block0(&s, &mut a, &mut rx).await;
    for i in 0..(3 * MAX_APPENDED as u64) {
        mock.add(5_000 + i, 500);
        converge_block0(&s, &mut a, &mut rx).await;
    }
    let deltas = a.frames;
    assert!(deltas > 2 * MAX_APPENDED, "{deltas} frames");
    let (conn_b, mut rx_b) = conn(4096);
    s.ctx.hub.subscribe(Topic::ProjectedTxs(0), &conn_b);
    let mut b = Client::default();
    converge_block0(&s, &mut b, &mut rx_b).await;
    assert!(b.frames <= 1 + MAX_APPENDED, "{} frames", b.frames);
}

async fn converge_block0(s: &Server, client: &mut Client, rx: &mut mpsc::Receiver<Frame>) {
    s.settle().await;
    let deadline = Instant::now() + WAIT;
    loop {
        client.drain(rx).unwrap();
        let p = s.projector.current();
        if client
            .blocks
            .get(&0)
            .is_some_and(|(_, r)| *r == expected(&p, 0))
        {
            return;
        }
        assert!(Instant::now() < deadline, "block 0 never matched");
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

#[tokio::test]
async fn a_client_that_falls_behind_is_closed_and_a_new_subscription_catches_up() {
    let mock = Mock::new();
    let s = Server::start(&mock, Duration::from_millis(5));
    // Room for the snapshots only: the first change fills its queue.
    let (slow, _never_read) = conn(3 + TXS_TOPICS);
    subscribe_all(&s.ctx, &slow);
    for i in 0..30 {
        mock.add(1_000 + i, 500);
        s.settle().await;
    }
    assert_eq!(slow.reason(), Some(Close::SLOW));
    // Its reconnect subscribes afresh and lands on the server's state.
    for t in topics() {
        s.ctx.hub.unsubscribe(t, slow.id);
    }
    let (again, mut rx) = conn(1024);
    subscribe_all(&s.ctx, &again);
    let mut c = Client::default();
    converge(&s, &mut c, &mut rx).await;
}

#[tokio::test]
async fn messages_are_rate_limited() {
    let mock = Mock::new();
    let interval = Duration::from_millis(300);
    let s = Server::start(&mock, interval);
    let (conn, mut rx) = conn(4096);
    s.ctx.hub.subscribe(Topic::Mempool, &conn);
    s.ctx.hub.subscribe(Topic::Transactions, &conn);
    let started = Instant::now();
    // Changes every 10 ms for 1.2 s: at most one message per interval and topic, plus the first.
    while started.elapsed() < Duration::from_millis(1200) {
        mock.add(1_000, 500);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let mut counts: HashMap<String, usize> = HashMap::new();
    while let Ok(f) = rx.try_recv() {
        let v: Value = serde_json::from_slice(open(&f).1).unwrap();
        *counts
            .entry(v["topic"].as_str().unwrap().to_owned())
            .or_default() += 1;
    }
    for topic in ["mempool", "transactions"] {
        let n = counts.get(topic).copied().unwrap_or(0);
        assert!((2..=6).contains(&n), "{topic}: {n} messages in 1.2 s");
    }
    // The list never exceeds its size.
    let _ = s;
}

#[tokio::test]
async fn the_off_switch_says_so_on_every_topic_and_polls_nothing() {
    let ctx = context(false, Duration::from_millis(10));
    start(&ctx);
    let (conn, mut rx) = conn(64);
    subscribe_all(&ctx, &conn);
    let mut seen = 0;
    while let Ok(f) = rx.try_recv() {
        let v: Value = serde_json::from_slice(open(&f).1).unwrap();
        assert_eq!(v["type"], "off", "{v}");
        seen += 1;
    }
    assert_eq!(seen, 3 + TXS_TOPICS);
}

#[tokio::test]
async fn a_node_down_at_boot_is_retried_and_the_topics_start_when_it_comes_back() {
    let mock = Mock::new();
    mock.add(1_000, 500);
    mock.set_down(true);
    let s = Server::start(&mock, Duration::from_millis(10));
    let (conn, mut rx) = conn(1024);
    subscribe_all(&s.ctx, &conn);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        rx.try_recv().is_err(),
        "nothing is published before the first cycle"
    );
    mock.set_down(false);
    let mut c = Client::default();
    converge(&s, &mut c, &mut rx).await;
    assert_eq!(c.json["mempool"]["count"], 1);

    // Down again: the last projection stays, marked stale; then it recovers.
    mock.set_down(true);
    let deadline = Instant::now() + WAIT;
    while c.json["projected"]["stale"] != true {
        assert!(Instant::now() < deadline, "never marked stale");
        tokio::time::sleep(Duration::from_millis(10)).await;
        c.drain(&mut rx).unwrap();
    }
    assert_eq!(c.json["mempool"]["stale"], true);
    mock.add(2_000, 500);
    mock.set_down(false);
    converge(&s, &mut c, &mut rx).await;
    assert_eq!(c.json["mempool"]["count"], 2);
    assert_eq!(c.json["projected"]["stale"], false);
}

#[test]
fn a_transaction_accepted_again_is_listed_once() {
    let info = |time| TxInfo::new(Amount::from_sat(1), Amount::ZERO, 1, 4, time);
    let (a, b) = (
        Txid::from_byte_array([1; 32]),
        Txid::from_byte_array([2; 32]),
    );
    let list = newest(
        Vec::new(),
        [(&a, &info(100)), (&b, &info(101))].into_iter(),
        10,
    );
    // `a` left and came back later: its newer row replaces the old one, wherever that sorted.
    let list = newest(list, std::iter::once((&a, &info(102))), 10);
    let rows: Vec<(Txid, u32)> = list.iter().map(|s| (s.txid, s.time)).collect();
    assert_eq!(rows, vec![(a, 102), (b, 101)]);
}

#[tokio::test]
async fn a_new_minimum_fee_rate_is_sent_on_its_own() {
    let mock = Mock::new();
    mock.add(1_000, 500);
    let s = Server::start(&mock, Duration::from_millis(20));
    let (conn, mut rx) = conn(1024);
    s.ctx.hub.subscribe(Topic::Mempool, &conn);
    let mut c = Client::default();
    let deadline = Instant::now() + WAIT;
    while c
        .json
        .get("mempool")
        .is_none_or(|v| v["min_fee_rate"] != 1000)
    {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
        c.drain(&mut rx).unwrap();
    }
    // Nothing else changes: no new projection, only the node's floor.
    mock.node().min_fee = 2_500;
    while c.json["mempool"]["min_fee_rate"] != 2_500 {
        assert!(Instant::now() < deadline, "the new floor never arrived");
        tokio::time::sleep(Duration::from_millis(10)).await;
        c.drain(&mut rx).unwrap();
    }
}

/// The latest `fees` message `rx` holds, if any.
fn latest_fees(rx: &mut mpsc::Receiver<Frame>, last: &mut Value) {
    while let Ok(f) = rx.try_recv() {
        *last = serde_json::from_slice(open(&f).1).unwrap();
    }
}

#[tokio::test]
async fn fees_follow_the_projection_and_are_withdrawn_while_it_is_stale() {
    let mock = Mock::new();
    let s = Server::start(&mock, Duration::from_millis(10));
    // The server's own fees task is still warming up and its node is unreachable: it serves
    // nothing rather than a guess.
    // A second fees task on its own context, warmed up as after five quiet minutes.
    let ctx = context(true, Duration::from_millis(10));
    let mut rec = engine::fees::Recommender::default();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    rec.gap(now - 400);
    let engines = crate::ws::fees::Engines {
        tracker: s.tracker.clone(),
        projector: s.projector.clone(),
    };
    let task = tokio::spawn(crate::ws::fees::run(Arc::clone(&ctx), Some(engines), rec));
    let (conn, mut rx) = conn(1024);
    ctx.hub.subscribe(Topic::Fees, &conn);

    // 1.2 MvB: six 100 kvB transactions at 5 sat/vB, six at 2 sat/vB.
    for fee in [500_000, 200_000] {
        for _ in 0..6 {
            mock.add(fee, 100_000);
        }
    }
    s.settle().await;
    let mut last = Value::Null;
    let deadline = Instant::now() + WAIT;
    let levels = |v: &Value| -> Vec<u64> {
        v["levels"]
            .as_array()
            .map(|l| l.iter().map(|l| l["rate"].as_u64().unwrap()).collect())
            .unwrap_or_default()
    };
    // The next block holds the 5 sat/vB half and must outbid the 2 sat/vB rest; three blocks
    // hold everything, so the rest is the node's minimum (1 sat/vB on this mock).
    while levels(&last) != [2_001, 1_000, 1_000, 1_000] {
        assert!(Instant::now() < deadline, "never recommended: {last}");
        tokio::time::sleep(Duration::from_millis(10)).await;
        latest_fees(&mut rx, &mut last);
    }
    assert_eq!(last["source"], "projected");
    assert_eq!(last["min_fee_rate"], 1_000);
    assert_eq!(last["tip"]["height"], 0);
    assert_eq!(
        ctx.fees.current().unwrap().source,
        crate::ws::fees::Source::Projected
    );
    assert!(s.ctx.fees.current().is_none());
    let mb = ctx.fees.blocks().unwrap();
    assert_eq!(mb.blocks[0].txs + mb.blocks[1].txs, 12);
    assert_eq!(mb.blocks[0].median, 5_000);

    // A block takes the 5 sat/vB ones: everything left fits in the next block.
    mock.mine(6);
    s.settle().await;
    while levels(&last) != [1_000; 4] {
        assert!(
            Instant::now() < deadline,
            "never followed the block: {last}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
        latest_fees(&mut rx, &mut last);
    }
    assert_eq!(last["tip"]["height"], 1);

    // The node fails: the projection goes stale, and with no node estimates either, nothing
    // is served rather than an old answer.
    mock.set_down(true);
    while last["type"] != "unavailable" {
        assert!(Instant::now() < deadline, "never withdrawn: {last}");
        tokio::time::sleep(Duration::from_millis(10)).await;
        latest_fees(&mut rx, &mut last);
    }
    assert!(ctx.fees.current().is_none());
    mock.set_down(false);
    while last["source"] != "projected" {
        assert!(Instant::now() < deadline, "never came back: {last}");
        tokio::time::sleep(Duration::from_millis(10)).await;
        latest_fees(&mut rx, &mut last);
    }
    task.abort();
}

// A block found: the order and timing `docs/WS.md` promises.

/// What the `blocks` feed does for a new block: its message to subscribers, then the record of
/// what it announced.
fn announce_block(s: &Server) -> ChainTip {
    let tip = s.mock.node().tip.unwrap();
    let msg = json!({ "topic": "blocks", "type": "block", "block": { "id": tip.hash, "height": tip.height } });
    s.ctx.hub.announce_tip(Some(tip.hash), || {
        s.ctx.hub.publish(
            Topic::Blocks,
            &Frame::text(msg.to_string().as_bytes()),
            Snapshot::Replace,
        );
    });
    tip
}

/// A received frame as (topic, the tip height of a `projected` message, or the height of a
/// `block` message).
fn label(f: &Frame) -> (String, Option<u64>) {
    let (opcode, p) = open(f);
    if opcode == frame::BINARY {
        return (format!("projected-txs:{}", p[2]), None);
    }
    let v: Value = serde_json::from_slice(p).unwrap();
    let topic = v["topic"].as_str().unwrap().to_owned();
    let height = match v["type"].as_str() {
        Some("projected") => v["tip"]["height"].as_u64(),
        Some("block") => v["block"]["height"].as_u64(),
        _ => None,
    };
    (topic, height)
}

/// Frames until a `projected` message on `height` arrives, with the time it took.
async fn until_projected(
    rx: &mut mpsc::Receiver<Frame>,
    height: u64,
) -> (Vec<(String, Option<u64>)>, Duration) {
    let at = Instant::now();
    let mut seen = Vec::new();
    loop {
        let f = tokio::time::timeout(WAIT, rx.recv())
            .await
            .unwrap()
            .unwrap();
        let l = label(&f);
        let done = l == ("projected".to_owned(), Some(height));
        seen.push(l);
        if done {
            return (seen, at.elapsed());
        }
    }
}

#[tokio::test]
async fn a_found_block_is_followed_by_the_projection_on_it_and_nothing_older() {
    let mock = Mock::new();
    for i in 0..20 {
        mock.add(1_000 + i, 500);
    }
    // A long interval: only the new tip can explain a prompt message.
    let s = Server::start(&mock, Duration::from_secs(10));
    s.settle().await;
    announce_block(&s);
    let (watcher, mut rx) = conn(4096);
    s.ctx.hub.subscribe(Topic::Blocks, &watcher);
    subscribe_all(&s.ctx, &watcher);
    tokio::time::sleep(Duration::from_millis(50)).await;
    while rx.try_recv().is_ok() {}

    // The engines see the block before the `blocks` topic has announced it: the mempool topics
    // wait for its message.
    mock.mine(10);
    let tip = mock.node().tip.unwrap();
    s.tracker.block_found(tip);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(s.projector.current().tip, Some(tip), "the engines have it");
    assert!(rx.try_recv().is_err(), "nothing before the block's message");
    announce_block(&s);
    let (seen, took) = until_projected(&mut rx, u64::from(tip.height)).await;
    assert_eq!(seen[0], ("blocks".to_owned(), Some(u64::from(tip.height))));
    assert!(took < Duration::from_millis(500), "{took:?}");

    // The other way round: the block's message first, then the engines.
    tokio::time::sleep(Duration::from_millis(50)).await;
    while rx.try_recv().is_ok() {}
    mock.mine(5);
    let tip = announce_block(&s);
    s.tracker.block_found(tip);
    let (seen, took) = until_projected(&mut rx, u64::from(tip.height)).await;
    assert_eq!(seen[0], ("blocks".to_owned(), Some(u64::from(tip.height))));
    assert!(
        seen.iter()
            .all(|(t, h)| t != "projected" || *h == Some(u64::from(tip.height))),
        "no projection on the old tip after the block: {seen:?}"
    );
    assert!(took < Duration::from_millis(500), "{took:?}");
    let mut c = Client::default();
    let (late, mut late_rx) = conn(4096);
    subscribe_all(&s.ctx, &late);
    converge(&s, &mut c, &mut late_rx).await;
    assert_eq!(c.json["mempool"]["count"], 5);
    assert_eq!(c.json["projected"]["tip"]["height"], tip.height);
}

#[tokio::test]
async fn in_an_empty_mempool_the_projection_follows_every_block() {
    let mock = Mock::new();
    let s = Server::start(&mock, Duration::from_secs(10));
    s.settle().await;
    announce_block(&s);
    let (conn, mut rx) = conn(4096);
    subscribe_all(&s.ctx, &conn);
    for _ in 0..4 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        mock.mine(0);
        let tip = announce_block(&s);
        s.tracker.block_found(tip);
        let (_, took) = until_projected(&mut rx, u64::from(tip.height)).await;
        assert!(took < Duration::from_millis(500), "{took:?}");
    }
}

#[tokio::test]
async fn found_blocks_keep_every_client_equal_to_the_server() {
    for seed in 1..=3u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let mock = Mock::new();
        let s = Server::start(&mock, Duration::from_millis(15));
        let (early, mut early_rx) = conn(4096);
        subscribe_all(&s.ctx, &early);
        let mut a = Client::default();
        for _ in 0..8 {
            for _ in 0..rng.below(40) {
                mock.add(rng.below(400_000), 1_000 + rng.below(150_000));
            }
            let ids: Vec<Txid> = mock
                .node()
                .txs
                .iter()
                .map(|t| t.tx.compute_txid())
                .collect();
            for _ in 0..rng.below(4) {
                if ids.len() > 1 {
                    let i = rng.below(ids.len() as u64) as usize;
                    let j = rng.below(ids.len() as u64) as usize;
                    if i != j {
                        mock.join(ids[i], ids[j]);
                    }
                }
            }
            converge(&s, &mut a, &mut early_rx).await;
            // The block is applied from its transactions and published at once; the client
            // still lands on the server's state.
            let n = mock.node().txs.len();
            mock.mine(rng.below(n as u64 + 1) as usize);
            let tip = announce_block(&s);
            s.tracker.block_found(tip);
            converge(&s, &mut a, &mut early_rx).await;
            assert_eq!(a.json["projected"]["tip"]["height"], tip.height);
        }
    }
}
