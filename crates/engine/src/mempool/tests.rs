#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bitcoin::absolute::LockTime;
use bitcoin::block::{Header, Version};
use bitcoin::hashes::Hash;
use bitcoin::{
    Amount, Block, BlockHash, CompactTarget, OutPoint, ScriptBuf, Sequence, Transaction, TxIn,
    TxMerkleNode, TxOut, Txid, Witness,
};
use node::esplora::ChainTip;
use node::rpc::model::{EntryFees, MempoolEntry, MempoolInfo, RawMempool};
use tokio::sync::broadcast::error::RecvError;

use super::model::{Delta, Mempool, Removal, TxInfo};
use super::tracker::{Config, Event, Source, SourceError, Tracker};

// Building blocks.

/// A transaction spending `inputs` with `outputs` outputs; `tag` makes its txid unique.
fn tx(inputs: &[OutPoint], outputs: u32, tag: u32) -> Transaction {
    Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: LockTime::from_consensus(tag),
        input: inputs
            .iter()
            .map(|o| TxIn {
                previous_output: *o,
                script_sig: ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: Witness::new(),
            })
            .collect(),
        output: (0..outputs)
            .map(|i| TxOut {
                value: Amount::from_sat(1000 + u64::from(i)),
                script_pubkey: ScriptBuf::new(),
            })
            .collect(),
    }
}

/// A confirmed outpoint nobody in the test mempool created.
fn coin(n: u32) -> OutPoint {
    OutPoint::new(Txid::from_byte_array([0xcc; 32]), n)
}

fn out(t: &Transaction, vout: u32) -> OutPoint {
    OutPoint::new(t.compute_txid(), vout)
}

fn block(height: u32, prev: BlockHash, txs: &[&Transaction]) -> Block {
    let coinbase = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            script_sig: ScriptBuf::from_bytes(height.to_le_bytes().to_vec()),
            sequence: Sequence::MAX,
            witness: Witness::new(),
        }],
        ..tx(&[], 1, height)
    };
    let mut b = Block {
        header: Header {
            version: Version::TWO,
            prev_blockhash: prev,
            merkle_root: TxMerkleNode::all_zeros(),
            time: height,
            bits: CompactTarget::from_consensus(0x207f_ffff),
            nonce: 0,
        },
        txdata: std::iter::once(coinbase)
            .chain(txs.iter().map(|t| (*t).clone()))
            .collect(),
    };
    b.header.merkle_root = b.compute_merkle_root().unwrap();
    b
}

fn entry(t: &Transaction) -> MempoolEntry {
    let fee = Amount::from_sat(100 + u64::from(t.lock_time.to_consensus_u32()));
    let weight = t.weight().to_wu();
    MempoolEntry {
        wtxid: t.compute_wtxid(),
        vsize: weight.div_ceil(4),
        vsize_bip141: weight.div_ceil(4),
        weight,
        time: 1_700_000_000,
        height: 0,
        fees: EntryFees {
            base: fee,
            modified: fee.to_signed().unwrap(),
            ancestor: fee.to_signed().unwrap(),
            descendant: fee.to_signed().unwrap(),
            chunk: fee.to_signed().unwrap(),
        },
        ancestorcount: 1,
        ancestorsize: 0,
        descendantcount: 1,
        descendantsize: 0,
        chunkweight: weight,
        depends: Vec::new(),
        spentby: Vec::new(),
        bip125_replaceable: true,
        unbroadcast: false,
    }
}

fn tip(b: &Block, height: u32) -> ChainTip {
    ChainTip {
        hash: b.block_hash(),
        height,
    }
}

fn genesis() -> BlockHash {
    BlockHash::all_zeros()
}

/// A mempool holding `txs`, as the first delta would leave it.
fn pool(txs: &[&Transaction]) -> Mempool {
    let mut m = Mempool::default();
    let added = txs
        .iter()
        .map(|t| (t.compute_txid(), m.keys().record(&entry(t), t)))
        .collect();
    let d = Delta {
        seq: 1,
        mempool_sequence: 1,
        tip: tip(&block(0, genesis(), &[]), 0),
        added,
        removed: Vec::new(),
    };
    assert!(m.apply(&d));
    m
}

fn classify(
    m: &Mempool,
    removed: &[&Transaction],
    added: &[&Transaction],
    blocks: &[(u32, Block)],
) -> HashMap<Txid, Removal> {
    let mut removed: Vec<Txid> = removed.iter().map(|t| t.compute_txid()).collect();
    removed.sort_unstable();
    let added: Vec<(Txid, TxInfo)> = added
        .iter()
        .map(|t| (t.compute_txid(), m.keys().record(&entry(t), t)))
        .collect();
    let blocks: Vec<_> = blocks.iter().map(|(h, b)| m.keys().block(*h, b)).collect();
    m.classify(&removed, &added, &blocks).into_iter().collect()
}

fn replaced(by: &Transaction) -> Removal {
    Removal::Replaced {
        by: by.compute_txid(),
    }
}

// The pure core.

#[test]
fn records_carry_fee_size_value_and_time() {
    let t = tx(&[coin(0), coin(1)], 3, 7);
    let m = Mempool::default();
    let r = m.keys().record(&entry(&t), &t);
    assert_eq!(r.fee, Amount::from_sat(107));
    assert_eq!(r.value, Amount::from_sat(1000 + 1001 + 1002));
    assert_eq!(u64::from(r.weight), t.weight().to_wu());
    assert_eq!(r.time, 1_700_000_000);
    assert_eq!((r.outputs, r.spends.len()), (3, 2));
    // The fingerprint is keyed per mempool: another instance hashes differently.
    assert_ne!(
        Mempool::default().keys().key(&coin(0)),
        m.keys().key(&coin(0))
    );
}

#[test]
fn diff_finds_added_and_removed_in_txid_order() {
    let (a, b, c, d) = (
        tx(&[coin(0)], 1, 1),
        tx(&[coin(1)], 1, 2),
        tx(&[coin(2)], 1, 3),
        tx(&[coin(3)], 1, 4),
    );
    let m = pool(&[&a, &b, &c]);
    let diff = m.diff(&[d.compute_txid(), b.compute_txid(), c.compute_txid()]);
    assert_eq!(diff.added, vec![d.compute_txid()]);
    assert_eq!(diff.removed, vec![a.compute_txid()]);
    assert_eq!(m.diff(&[]).removed.len(), 3);
    let mut all = vec![a.compute_txid(), b.compute_txid(), c.compute_txid()];
    all.sort_unstable();
    assert_eq!(m.diff(&[]).removed, all);
    assert!(m.diff(&all).added.is_empty() && m.diff(&all).removed.is_empty());
}

#[test]
fn apply_refuses_out_of_order_deltas_and_keeps_totals() {
    let (a, b) = (tx(&[coin(0)], 1, 1), tx(&[coin(1)], 1, 2));
    let mut m = pool(&[&a, &b]);
    let (va, vb) = (
        m.get(&a.compute_txid()).unwrap().vsize,
        m.get(&b.compute_txid()).unwrap().vsize,
    );
    assert_eq!(m.vsize(), u64::from(va + vb));
    assert_eq!(m.fees(), Amount::from_sat(101 + 102));
    let d = Delta {
        seq: 3,
        mempool_sequence: 9,
        tip: m.tip().unwrap(),
        added: Vec::new(),
        removed: vec![(a.compute_txid(), Removal::Evicted)],
    };
    assert!(!m.apply(&d), "seq 3 after 1 is a gap");
    assert_eq!(m.len(), 2);
    assert!(m.apply(&Delta { seq: 2, ..d }));
    assert_eq!((m.len(), m.seq(), m.mempool_sequence()), (1, 2, 9));
    assert_eq!(m.vsize(), u64::from(vb));
    assert_eq!(m.fees(), Amount::from_sat(102));
}

#[test]
fn confirmed_replaced_and_evicted() {
    let confirmed = tx(&[coin(0)], 1, 1);
    let original = tx(&[coin(1), coin(2)], 1, 2);
    let evicted = tx(&[coin(3)], 1, 3);
    let stays = tx(&[coin(4)], 1, 4);
    // The replacement keeps only the second input, so matching cannot lean on the first.
    let replacement = tx(&[coin(2), coin(9)], 1, 5);
    let m = pool(&[&confirmed, &original, &evicted, &stays]);
    let b = block(1, genesis(), &[&confirmed]);
    let r = classify(
        &m,
        &[&confirmed, &original, &evicted],
        &[&replacement],
        &[(1, b.clone())],
    );
    assert_eq!(
        r[&confirmed.compute_txid()],
        Removal::Confirmed {
            height: 1,
            block: b.block_hash()
        }
    );
    assert_eq!(r[&original.compute_txid()], replaced(&replacement));
    assert_eq!(r[&evicted.compute_txid()], Removal::Evicted);
    assert_eq!(r.len(), 3);
}

#[test]
fn a_block_can_confirm_some_and_double_spend_others() {
    // `a` confirms. `b` loses to `b2`, which the node never saw but a miner did.
    // `c` loses to `c2` arriving in the mempool in the same cycle.
    let a = tx(&[coin(0)], 1, 1);
    let b = tx(&[coin(1)], 1, 2);
    let b2 = tx(&[coin(1)], 1, 20);
    let c = tx(&[coin(2)], 1, 3);
    let c2 = tx(&[coin(2)], 1, 30);
    let m = pool(&[&a, &b, &c]);
    let blk = block(5, genesis(), &[&a, &b2]);
    let r = classify(&m, &[&a, &b, &c], &[&c2], &[(5, blk.clone())]);
    assert_eq!(
        r[&a.compute_txid()],
        Removal::Confirmed {
            height: 5,
            block: blk.block_hash()
        }
    );
    assert_eq!(r[&b.compute_txid()], replaced(&b2));
    assert_eq!(r[&c.compute_txid()], replaced(&c2));
}

#[test]
fn an_older_block_in_the_window_still_counts() {
    let a = tx(&[coin(0)], 1, 1);
    let m = pool(&[&a]);
    let b1 = block(1, genesis(), &[&a]);
    let b2 = block(2, b1.block_hash(), &[]);
    let r = classify(&m, &[&a], &[], &[(2, b2), (1, b1.clone())]);
    assert_eq!(
        r[&a.compute_txid()],
        Removal::Confirmed {
            height: 1,
            block: b1.block_hash()
        }
    );
}

#[test]
fn descendants_leave_with_their_replaced_ancestor() {
    let parent = tx(&[coin(0)], 2, 1);
    let child = tx(&[out(&parent, 1)], 1, 2);
    let grandchild = tx(&[out(&child, 0)], 1, 3);
    let replacement = tx(&[coin(0)], 1, 4);
    let m = pool(&[&parent, &child, &grandchild]);
    // Listed in any order: the result does not depend on it.
    let r = classify(&m, &[&grandchild, &child, &parent], &[&replacement], &[]);
    for t in [&parent, &child, &grandchild] {
        assert_eq!(r[&t.compute_txid()], replaced(&replacement));
    }
}

#[test]
fn a_child_of_a_confirmed_or_evicted_parent_is_not_replaced() {
    let parent = tx(&[coin(0)], 1, 1);
    let child = tx(&[out(&parent, 0)], 1, 2);
    let lone = tx(&[coin(5)], 1, 3);
    let lone_child = tx(&[out(&lone, 0)], 1, 4);
    let m = pool(&[&parent, &child, &lone, &lone_child]);
    let b = block(1, genesis(), &[&parent]);
    let r = classify(&m, &[&parent, &child, &lone, &lone_child], &[], &[(1, b)]);
    assert!(matches!(
        r[&parent.compute_txid()],
        Removal::Confirmed { .. }
    ));
    assert_eq!(r[&child.compute_txid()], Removal::Evicted);
    assert_eq!(r[&lone.compute_txid()], Removal::Evicted);
    assert_eq!(r[&lone_child.compute_txid()], Removal::Evicted);
}

#[test]
fn a_replacement_chain_names_the_live_replacement() {
    // a -> b -> c inside one interval: b was never seen, c shares a's input.
    let a = tx(&[coin(0)], 1, 1);
    let c = tx(&[coin(0), coin(1)], 1, 3);
    let m = pool(&[&a]);
    assert_eq!(
        classify(&m, &[&a], &[&c], &[])[&a.compute_txid()],
        replaced(&c)
    );
    // If c spends none of a's inputs, the link is lost and a reads as evicted (documented).
    let c_only_b_inputs = tx(&[coin(7)], 1, 4);
    assert_eq!(
        classify(&m, &[&a], &[&c_only_b_inputs], &[])[&a.compute_txid()],
        Removal::Evicted
    );
}

#[test]
fn after_a_reorg_only_the_new_chain_counts() {
    // `a` was in a block that a reorg removed; the winning block double-spends it.
    let a = tx(&[coin(0)], 1, 1);
    let rival = tx(&[coin(0)], 1, 2);
    let m = pool(&[&a]);
    let winner = block(1, genesis(), &[&rival]);
    assert_eq!(
        classify(&m, &[&a], &[], &[(1, winner)])[&a.compute_txid()],
        replaced(&rival)
    );
}

// The shell, against a mock node.

#[derive(Default)]
struct Node {
    pool: Vec<Transaction>,
    sequence: u64,
    /// Blocks by hash, and the active tip.
    blocks: HashMap<BlockHash, Block>,
    tip: Option<ChainTip>,
    /// Listed but answering "not in mempool" to detail calls.
    vanish: HashSet<Txid>,
    /// Errors returned by the next calls of any kind, in order.
    failures: VecDeque<SourceError>,
    /// Outcomes of the next detail calls, in order; `None` succeeds.
    detail_failures: VecDeque<Option<SourceError>>,
    /// Detail batches larger than this answer `TooBig`.
    max_batch: Option<usize>,
    /// Same `getmempoolinfo` whatever the pool, to test the heartbeat.
    frozen_info: bool,
    delay: Duration,
}

#[derive(Default)]
struct Counters {
    infos: AtomicU32,
    listings: AtomicU32,
    details: AtomicU32,
    blocks: AtomicU32,
    in_flight: AtomicU32,
    max_in_flight: AtomicU32,
    listings_in_flight: AtomicU32,
    max_listings: AtomicU32,
    bytes: AtomicU64,
}

#[derive(Clone, Default)]
struct Mock {
    node: Arc<Mutex<Node>>,
    n: Arc<Counters>,
}

impl Mock {
    fn new() -> Self {
        let m = Self::default();
        m.mine(&[]);
        m
    }

    fn with<R>(&self, f: impl FnOnce(&mut Node) -> R) -> R {
        f(&mut self.node.lock().unwrap())
    }

    fn send(&self, t: &Transaction) {
        self.with(|n| {
            n.pool.retain(|p| {
                !p.input.iter().any(|i| {
                    t.input
                        .iter()
                        .any(|j| i.previous_output == j.previous_output)
                })
            });
            n.pool.push(t.clone());
            n.sequence += 1;
        });
    }

    /// Mines `txs` onto the tip, dropping them and their conflicts from the pool.
    fn mine(&self, txs: &[&Transaction]) -> Block {
        self.with(|n| {
            let (prev, height) = n.tip.map_or((genesis(), 0), |t| (t.hash, t.height + 1));
            Self::connect(n, block(height, prev, txs), height)
        })
    }

    fn connect(n: &mut Node, b: Block, height: u32) -> Block {
        let spent: HashSet<OutPoint> = b
            .txdata
            .iter()
            .flat_map(|t| t.input.iter().map(|i| i.previous_output))
            .collect();
        let ids: HashSet<Txid> = b.txdata.iter().map(|t| t.compute_txid()).collect();
        n.pool.retain(|p| {
            !ids.contains(&p.compute_txid())
                && !p.input.iter().any(|i| spent.contains(&i.previous_output))
        });
        n.sequence += 1;
        n.tip = Some(tip(&b, height));
        n.blocks.insert(b.block_hash(), b.clone());
        b
    }

    async fn enter(&self) -> Result<(), SourceError> {
        let now = self.n.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.n.max_in_flight.fetch_max(now, Ordering::SeqCst);
        let delay = self.with(|n| n.delay);
        tokio::time::sleep(delay).await;
        self.n.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.n.bytes.fetch_add(100, Ordering::SeqCst);
        match self.with(|n| n.failures.pop_front()) {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

impl Source for Mock {
    async fn mempool_info(&self) -> Result<MempoolInfo, SourceError> {
        self.n.infos.fetch_add(1, Ordering::SeqCst);
        self.enter().await?;
        Ok(self.with(|n| {
            let size = if n.frozen_info {
                0
            } else {
                n.pool.len() as u64
            };
            MempoolInfo {
                loaded: true,
                size,
                bytes: if n.frozen_info { 0 } else { n.sequence },
                usage: 0,
                maxmempool: 300_000_000,
                total_fee: Amount::ZERO,
                mempoolminfee: Amount::ZERO,
                minrelaytxfee: Amount::ZERO,
                incrementalrelayfee: Amount::ZERO,
                fullrbf: true,
                limitclustercount: 64,
                limitclustersize: 101_000,
                unbroadcastcount: 0,
            }
        }))
    }

    async fn listing(&self) -> Result<RawMempool, SourceError> {
        self.n.listings.fetch_add(1, Ordering::SeqCst);
        let now = self.n.listings_in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.n.max_listings.fetch_max(now, Ordering::SeqCst);
        let r = self.enter().await;
        self.n.listings_in_flight.fetch_sub(1, Ordering::SeqCst);
        r?;
        Ok(self.with(|n| RawMempool {
            mempool_sequence: n.sequence,
            txids: n.pool.iter().map(Transaction::compute_txid).collect(),
        }))
    }

    async fn details(
        &self,
        txids: &[Txid],
    ) -> Result<Vec<Option<(MempoolEntry, Transaction)>>, SourceError> {
        self.n.details.fetch_add(1, Ordering::SeqCst);
        self.enter().await?;
        self.with(|n| {
            if let Some(Some(e)) = n.detail_failures.pop_front() {
                return Err(e);
            }
            if n.max_batch.is_some_and(|m| txids.len() > m) {
                return Err(SourceError::TooBig);
            }
            Ok(txids
                .iter()
                .map(|id| {
                    let t = n.pool.iter().find(|t| t.compute_txid() == *id)?;
                    (!n.vanish.contains(id)).then(|| (entry(t), t.clone()))
                })
                .collect())
        })
    }

    async fn tip(&self) -> Result<ChainTip, SourceError> {
        self.enter().await?;
        self.with(|n| n.tip.ok_or(SourceError::Failed("no tip".into())))
    }

    async fn block(&self, hash: BlockHash) -> Result<Block, SourceError> {
        self.n.blocks.fetch_add(1, Ordering::SeqCst);
        self.enter().await?;
        self.with(|n| {
            n.blocks
                .get(&hash)
                .cloned()
                .ok_or(SourceError::Failed("unknown block".into()))
        })
    }

    fn bytes(&self) -> u64 {
        self.n.bytes.load(Ordering::SeqCst)
    }
}

fn config() -> Config {
    Config {
        batch: 2,
        blocks: 3,
        ..Config::default()
    }
}

/// The next `Cycle` or `PollFailed`, skipping other events.
async fn next_cycle(ev: &mut tokio::sync::broadcast::Receiver<Event>) -> Event {
    loop {
        if let e @ (Event::Cycle(_) | Event::PollFailed { .. }) = ev.recv().await.unwrap() {
            return e;
        }
    }
}

fn cycle(e: Event) -> super::CycleStats {
    match e {
        Event::Cycle(s) => s,
        e => panic!("expected a cycle, got {e:?}"),
    }
}

/// The tracker's txids.
fn txids(t: &Tracker) -> HashSet<Txid> {
    t.read(|m| m.iter().map(|(k, _)| *k).collect())
}

fn pool_txids(mock: &Mock) -> HashSet<Txid> {
    mock.with(|n| n.pool.iter().map(Transaction::compute_txid).collect())
}

#[tokio::test(start_paused = true)]
async fn follows_sends_replacements_and_blocks_with_reasons() {
    let mock = Mock::new();
    let (a, b, c) = (
        tx(&[coin(0)], 2, 1),
        tx(&[coin(1)], 1, 2),
        tx(&[coin(2)], 1, 3),
    );
    for t in [&a, &b, &c] {
        mock.send(t);
    }
    let (tracker, task) = Tracker::start(mock.clone(), config());
    let mut ev = tracker.events();
    let first = ev.recv().await.unwrap();
    let Event::Synced { txs: 3, .. } = first else {
        panic!("{first:?}")
    };
    let s = cycle(next_cycle(&mut ev).await);
    assert_eq!((s.seq, s.added, s.size), (Some(1), 3, 3));
    assert_eq!(txids(&tracker), pool_txids(&mock));

    // A subscriber mirrors the state from a snapshot plus deltas.
    let (mut mirror, mut sub) = tracker.subscribe(|m| {
        (
            m.seq(),
            m.iter()
                .map(|(k, v)| (*k, v.clone()))
                .collect::<HashMap<_, _>>(),
        )
    });

    // RBF of `b`, and a child of `a`.
    let b2 = tx(&[coin(1)], 1, 20);
    let child = tx(&[out(&a, 1)], 1, 4);
    mock.send(&b2);
    mock.send(&child);
    let s = cycle(next_cycle(&mut ev).await);
    assert_eq!((s.added, s.replaced, s.size), (2, 1, 4));
    let d = sub.recv().await.unwrap();
    assert_eq!(d.seq, mirror.0 + 1);
    assert_eq!(d.removed, vec![(b.compute_txid(), replaced(&b2))]);

    // A block confirms `a` and `b2`; `child` stays.
    let blk = mock.mine(&[&a, &b2]);
    tracker.tip_changed();
    let s = cycle(next_cycle(&mut ev).await);
    assert_eq!((s.confirmed, s.blocks, s.size), (2, 1, 2));
    let d2 = sub.recv().await.unwrap();
    assert_eq!(d2.seq, d.seq + 1);
    assert_eq!(d2.tip.hash, blk.block_hash());
    assert!(d2.removed.iter().all(|(_, r)| *r
        == Removal::Confirmed {
            height: 1,
            block: blk.block_hash()
        }));
    for delta in [d, d2] {
        mirror.0 = delta.seq;
        for (t, _) in &delta.removed {
            mirror.1.remove(t);
        }
        mirror.1.extend(delta.added.iter().cloned());
    }
    let state = tracker.read(|m| (m.seq(), m.iter().map(|(k, v)| (*k, v.clone())).collect()));
    assert_eq!(mirror, state);
    assert_eq!(txids(&tracker), pool_txids(&mock));
    task.abort();
}

#[tokio::test(start_paused = true)]
async fn probes_skip_unchanged_pools_and_the_heartbeat_lists_anyway() {
    let mock = Mock::new();
    mock.send(&tx(&[coin(0)], 1, 1));
    let (tracker, task) = Tracker::start(mock.clone(), config());
    let mut ev = tracker.events();
    cycle(next_cycle(&mut ev).await);
    tokio::time::sleep(Duration::from_millis(5500)).await;
    // Five probes found nothing; still one listing.
    assert_eq!(mock.n.listings.load(Ordering::SeqCst), 1);
    assert!(mock.n.infos.load(Ordering::SeqCst) >= 6);

    // A change getmempoolinfo cannot see is still found by the heartbeat.
    mock.with(|n| n.frozen_info = true);
    cycle(next_cycle(&mut ev).await); // the probe saw the info change to frozen
    mock.send(&tx(&[coin(1)], 1, 2));
    let s = cycle(next_cycle(&mut ev).await);
    assert_eq!(s.added, 1);
    assert!(s.skipped >= 8, "{s:?}");
    task.abort();
    drop(tracker);
}

#[tokio::test(start_paused = true)]
async fn a_slow_node_is_never_polled_twice_at_once() {
    let mock = Mock::new();
    for i in 0..10 {
        mock.send(&tx(&[coin(i)], 1, i));
    }
    mock.with(|n| n.delay = Duration::from_secs(3));
    let mut cfg = config();
    cfg.in_flight = 2;
    let (tracker, task) = Tracker::start(mock.clone(), cfg);
    let mut ev = tracker.events();
    let mut last_start = None;
    for i in 0..4 {
        mock.send(&tx(&[coin(100 + i)], 1, 100 + i));
        let s = cycle(next_cycle(&mut ev).await);
        assert!(s.duration >= Duration::from_secs(9), "{s:?}");
        let start = tokio::time::Instant::now() - s.duration;
        if let Some(prev) = last_start {
            // The previous cycle took as long; it was followed by at least as long a rest.
            assert!(start - prev >= s.duration * 2, "{:?}", start - prev);
        }
        last_start = Some(start);
    }
    // A hint during a cycle waits for it to end: never two listings at once.
    mock.send(&tx(&[coin(200)], 1, 200));
    tokio::time::sleep(Duration::from_secs(20)).await;
    tracker.tip_changed();
    tokio::time::sleep(Duration::from_secs(1)).await;
    tracker.tip_changed();
    cycle(next_cycle(&mut ev).await);
    cycle(next_cycle(&mut ev).await);
    assert_eq!(mock.n.max_listings.load(Ordering::SeqCst), 1);
    // At most two detail batches at once.
    assert!(mock.n.max_in_flight.load(Ordering::SeqCst) <= 2);
    assert_eq!(txids(&tracker), pool_txids(&mock));
    task.abort();
}

#[tokio::test(start_paused = true)]
async fn failures_back_off_up_to_the_cap() {
    let mock = Mock::new();
    mock.send(&tx(&[coin(0)], 1, 0));
    mock.with(|n| n.failures = VecDeque::from(vec![SourceError::Busy]));
    let (tracker, task) = Tracker::start(mock.clone(), config());
    let mut ev = tracker.events();
    let e = next_cycle(&mut ev).await;
    assert_eq!(
        e,
        Event::PollFailed {
            error: SourceError::Busy,
            consecutive: 1,
            retry_in: Duration::from_secs(2)
        }
    );
    assert_eq!(cycle(next_cycle(&mut ev).await).size, 1);

    mock.with(|n| {
        n.failures = std::iter::repeat_n(SourceError::Failed("down".into()), 6).collect();
    });
    let mut waits = Vec::new();
    for _ in 0..6 {
        let Event::PollFailed { retry_in, .. } = next_cycle(&mut ev).await else {
            panic!()
        };
        waits.push(retry_in.as_secs());
    }
    assert_eq!(waits, vec![2, 4, 8, 16, 30, 30]);
    mock.send(&tx(&[coin(50)], 1, 50));
    let s = cycle(next_cycle(&mut ev).await);
    assert_eq!((s.added, s.size), (1, 2));
    assert_eq!(txids(&tracker), pool_txids(&mock));
    task.abort();
}

#[tokio::test(start_paused = true)]
async fn a_failed_fetch_publishes_nothing_and_resumes_where_it_stopped() {
    let mock = Mock::new();
    for i in 0..6 {
        mock.send(&tx(&[coin(i)], 1, i));
    }
    // Batches of 2, one at a time: the second of three batches fails.
    mock.with(|n| n.detail_failures = VecDeque::from(vec![None, Some(SourceError::Busy)]));
    let mut cfg = config();
    cfg.in_flight = 1;
    let (tracker, task) = Tracker::start(mock.clone(), cfg);
    let mut ev = tracker.events();
    let (_, mut sub) = tracker.subscribe(|_| ());
    let Event::PollFailed { error, .. } = next_cycle(&mut ev).await else {
        panic!()
    };
    assert_eq!(error, SourceError::Busy);
    assert!(tracker.read(|m| m.is_empty()), "nothing applied");
    assert!(
        tokio::time::timeout(Duration::ZERO, sub.recv())
            .await
            .is_err()
    );
    // The retry fetches only the four it does not hold yet.
    let s = cycle(next_cycle(&mut ev).await);
    // Probe, listing, tip, two detail batches; the block is still held.
    assert_eq!((s.added, s.size, s.calls, s.blocks), (6, 6, 5, 0));
    assert_eq!(mock.n.details.load(Ordering::SeqCst), 4);
    let d = sub.recv().await.unwrap();
    assert_eq!((d.seq, d.added.len()), (1, 6));

    // Over the size cap: batches are halved until they fit.
    mock.with(|n| n.max_batch = Some(1));
    mock.send(&tx(&[coin(10)], 1, 10));
    mock.send(&tx(&[coin(11)], 1, 11));
    let s = cycle(next_cycle(&mut ev).await);
    assert_eq!((s.added, s.size), (2, 8));
    // A single transaction that never fits is skipped; the rest of the cycle still lands.
    mock.with(|n| n.max_batch = Some(0));
    mock.send(&tx(&[coin(12)], 1, 12));
    mock.with(|n| {
        n.pool
            .retain(|t| t.compute_txid() != tx(&[coin(10)], 1, 10).compute_txid())
    });
    let s = cycle(next_cycle(&mut ev).await);
    assert_eq!((s.added, s.vanished, s.evicted, s.size), (0, 1, 1, 7));
    task.abort();
}

#[tokio::test(start_paused = true)]
async fn vanished_transactions_are_skipped_then_forgotten() {
    let mock = Mock::new();
    let (a, b) = (tx(&[coin(0)], 1, 1), tx(&[coin(1)], 1, 2));
    mock.send(&a);
    mock.send(&b);
    mock.with(|n| {
        n.vanish.insert(b.compute_txid());
    });
    let (tracker, task) = Tracker::start(mock.clone(), config());
    let mut ev = tracker.events();
    let s = cycle(next_cycle(&mut ev).await);
    assert_eq!((s.added, s.vanished, s.size), (1, 1, 1));
    // Readable again in an unchanged pool: the probe must not skip it until the heartbeat.
    mock.with(|n| n.vanish.clear());
    let s = cycle(next_cycle(&mut ev).await);
    assert_eq!((s.added, s.vanished, s.skipped, s.size), (1, 0, 0, 2));
    // Gone for real; the next cycle classifies its removal.
    mock.with(|n| n.pool.retain(|t| t.compute_txid() != b.compute_txid()));
    mock.send(&tx(&[coin(2)], 1, 3));
    let s = cycle(next_cycle(&mut ev).await);
    assert_eq!((s.added, s.vanished, s.evicted, s.size), (1, 0, 1, 2));
    assert_eq!(txids(&tracker), pool_txids(&mock));
    task.abort();
}

#[tokio::test(start_paused = true)]
async fn restarts_emptied_pools_and_sequence_regressions() {
    let mock = Mock::new();
    for i in 0..4 {
        mock.send(&tx(&[coin(i)], 1, i));
    }
    let (tracker, task) = Tracker::start(mock.clone(), config());
    let mut ev = tracker.events();
    cycle(next_cycle(&mut ev).await);
    // The node restarts with an empty pool and a reset sequence.
    mock.with(|n| {
        n.pool.clear();
        n.sequence = 1;
    });
    let mut regressed = None;
    let s = loop {
        match ev.recv().await.unwrap() {
            Event::SequenceRegressed { from, to } => regressed = Some((from, to)),
            Event::Cycle(s) => break s,
            _ => {}
        }
    };
    assert_eq!(regressed, Some((5, 1)));
    assert_eq!((s.evicted, s.size), (4, 0));
    assert!(tracker.read(|m| m.is_empty()));
    task.abort();
}

#[tokio::test(start_paused = true)]
async fn reorgs_return_transactions_and_confirm_the_new_chain() {
    let mock = Mock::new();
    let (a, b) = (tx(&[coin(0)], 1, 1), tx(&[coin(1)], 1, 2));
    mock.send(&a);
    mock.send(&b);
    let (tracker, task) = Tracker::start(mock.clone(), config());
    let mut ev = tracker.events();
    cycle(next_cycle(&mut ev).await);
    let fork_base = mock.with(|n| n.tip.unwrap());
    let old = mock.mine(&[&a]);
    let s = cycle(next_cycle(&mut ev).await);
    assert_eq!(s.confirmed, 1);
    // A heavier branch from the same parent confirms `b` and a rival of nothing;
    // `a` returns to the mempool.
    let rival_b = tx(&[coin(1)], 1, 99);
    mock.with(|n| {
        n.pool.push(a.clone());
        n.blocks.remove(&old.block_hash());
        let x = Mock::connect(
            n,
            block(fork_base.height + 1, fork_base.hash, &[&rival_b]),
            fork_base.height + 1,
        );
        let y = block(fork_base.height + 2, x.block_hash(), &[]);
        Mock::connect(n, y, fork_base.height + 2);
    });
    let s = cycle(next_cycle(&mut ev).await);
    assert_eq!((s.added, s.replaced, s.size), (1, 1, 1));
    assert_eq!(txids(&tracker), HashSet::from([a.compute_txid()]));
    task.abort();
}

#[tokio::test(start_paused = true)]
async fn a_slow_subscriber_is_told_to_resubscribe() {
    let mock = Mock::new();
    let mut cfg = config();
    cfg.subscriber_queue = 2;
    let (tracker, task) = Tracker::start(mock.clone(), cfg);
    let mut ev = tracker.events();
    cycle(next_cycle(&mut ev).await);
    let (seq, mut sub) = tracker.subscribe(|m| m.seq());
    for i in 0..4 {
        mock.send(&tx(&[coin(i)], 1, i));
        cycle(next_cycle(&mut ev).await);
    }
    assert_eq!(sub.recv().await.unwrap_err(), RecvError::Lagged(2));
    let lag = loop {
        if let Event::SubscriberLagged { missed } = ev.recv().await.unwrap() {
            break missed;
        }
    };
    assert_eq!(lag, 2);
    // After the lag it continues with the newest deltas it still holds.
    assert_eq!(sub.recv().await.unwrap().seq, seq + 3);
    // A fresh subscription starts consistent again.
    let (seq, _sub) = tracker.subscribe(|m| m.seq());
    assert_eq!(seq, tracker.read(|m| m.seq()));
    task.abort();
}
