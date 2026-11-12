//! The close-watcher against a fake chain: blocks, positions and spends all read from one list
//! of blocks, so reorgs are real.

use std::sync::atomic::AtomicU32;

use lightning::bitcoin::absolute::LockTime;
use lightning::bitcoin::block::{Header as BlockHeader, Version as BlockVersion};
use lightning::bitcoin::transaction::Version;
use lightning::bitcoin::{
    Amount, Block, CompactTarget, OutPoint, Sequence, Transaction, TxIn, TxMerkleNode, TxOut, Txid,
    Witness,
};

use super::*;
use crate::closes::{self, MAX_SWEEPS, funding_program, revoked};

static SALT: AtomicU32 = AtomicU32::new(1);

/// Mines a block of `txs` on top of the chain; returns its height.
fn mine(node: &FakeChain, txs: Vec<Transaction>) -> u32 {
    let mut blocks = node.0.blocks.lock().unwrap();
    let height = blocks.len() as u32;
    let salt = SALT.fetch_add(1, Ordering::Relaxed);
    let coinbase = Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            script_sig: ScriptBuf::from_bytes([height.to_le_bytes(), salt.to_le_bytes()].concat()),
            sequence: Sequence::MAX,
            witness: Witness::new(),
        }],
        output: vec![],
    };
    let header = BlockHeader {
        version: BlockVersion::TWO,
        prev_blockhash: blocks
            .last()
            .map_or(BlockHash::all_zeros(), |b| b.block_hash()),
        merkle_root: TxMerkleNode::all_zeros(),
        time: 1_700_000_000 + height,
        bits: CompactTarget::from_consensus(0x207f_ffff),
        nonce: salt,
    };
    let txdata = std::iter::once(coinbase).chain(txs).collect();
    blocks.push(Block { header, txdata });
    node.0.tip.store(height, Ordering::Relaxed);
    height
}

fn mine_empty(node: &FakeChain, n: u32) {
    for _ in 0..n {
        mine(node, vec![]);
    }
}

/// Drops the blocks from `height` up: the next ones mined replace them.
fn unmine(node: &FakeChain, height: u32) {
    node.0.blocks.lock().unwrap().truncate(height as usize);
}

fn tx(input: Vec<TxIn>, output: Vec<TxOut>, lock_time: u32) -> Transaction {
    Transaction {
        version: Version::TWO,
        lock_time: LockTime::from_consensus(lock_time),
        input,
        output,
    }
}

fn out(script: ScriptBuf, sats: u64) -> TxOut {
    TxOut {
        value: Amount::from_sat(sats),
        script_pubkey: script,
    }
}

fn input(prev: OutPoint, sequence: u32, witness: Vec<Vec<u8>>) -> TxIn {
    TxIn {
        previous_output: prev,
        script_sig: ScriptBuf::new(),
        sequence: Sequence(sequence),
        witness: Witness::from_slice(&witness),
    }
}

/// A transaction paying a channel between `a` and `b` its funding output, from a made-up input.
fn fund(a: u8, b: u8, sats: u64) -> Transaction {
    let n = SALT.fetch_add(1, Ordering::Relaxed);
    let prev = OutPoint::new(Txid::from_byte_array([n as u8; 32]), n);
    tx(
        vec![input(prev, u32::MAX, vec![vec![1; 64]])],
        vec![out(funding(a, b), sats)],
        0,
    )
}

/// The witness that spends a channel's funding output.
fn funding_witness(a: u8, b: u8) -> Vec<Vec<u8>> {
    let s = Secp256k1::new();
    let script =
        make_funding_redeemscript(&key(a + 100).public_key(&s), &key(b + 100).public_key(&s));
    vec![vec![], vec![0x30; 72], vec![0x30; 72], script.to_bytes()]
}

fn wpkh() -> ScriptBuf {
    ScriptBuf::from_bytes([&[0x00, 0x14][..], &[7; 20]].concat())
}

fn coop(prev: OutPoint, a: u8, b: u8, outs: Vec<TxOut>) -> Transaction {
    tx(vec![input(prev, u32::MAX, funding_witness(a, b))], outs, 0)
}

/// BOLT 3's `to_local` script with a 144-block delay.
fn to_local() -> ScriptBuf {
    let k = [2u8; 33];
    let mut s = vec![0x63, 0x21];
    s.extend(k);
    s.extend([0x67, 0x02, 0x90, 0x00, 0xb2, 0x75, 0x21]);
    s.extend(k);
    s.extend([0x68, 0xac]);
    ScriptBuf::from_bytes(s)
}

/// A commitment with a `to_local` output (0), an anchor (1) and a `to_remote` (2).
fn commitment(prev: OutPoint, a: u8, b: u8) -> Transaction {
    tx(
        vec![input(prev, 0x8000_0042, funding_witness(a, b))],
        vec![
            out(to_local().to_p2wsh(), 600_000),
            out(ScriptBuf::new_op_return([]).to_p2wsh(), 330),
            out(wpkh(), 300_000),
        ],
        0x2000_0007,
    )
}

/// Sweeps `to_local` through its revocation path (`revoked`) or after its delay.
fn sweep(commitment: &Transaction, revoked: bool) -> Transaction {
    let arg = if revoked { vec![1] } else { vec![] };
    let prev = OutPoint::new(commitment.compute_txid(), 0);
    tx(
        vec![input(
            prev,
            144,
            vec![vec![0x30; 72], arg, to_local().to_bytes()],
        )],
        vec![out(wpkh(), 590_000)],
        0,
    )
}

struct Net {
    node: FakeChain,
    g: Arc<Gossip>,
}

fn config() -> Config {
    quick(|c| c.tip_every = Duration::from_millis(20))
}

/// A handler with lookups and the close-watcher running against `node`.
fn net(node: &FakeChain, config: &Config, replay: Replay) -> Net {
    let (g, _, _) = gossip(config, replay);
    let pace = pace(config);
    tokio::spawn(chain::run(Arc::clone(&g), node.clone(), Arc::clone(&pace)));
    tokio::spawn(closes::run(
        Arc::clone(&g),
        node.clone(),
        pace,
        config.tip_every,
    ));
    Net {
        node: node.clone(),
        g,
    }
}

impl Net {
    async fn at(&self, height: u32) {
        let w = &self.g.watch;
        for _ in 0..500 {
            let s = w.status();
            if s.height == Some(height) && s.caught_up {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("watcher not at {height}: {:?}", w.status());
    }

    /// Announces the channel funded at `scid` and waits for it to be verified.
    async fn open(&self, scid: u64, a: u8, b: u8) {
        let _ = self
            .g
            .handle_channel_announcement(None, &channel(scid, a, b));
        wait_for("verified", || self.in_graph(scid)).await;
    }

    fn in_graph(&self, scid: u64) -> bool {
        self.g.graph().read_only().channel(scid).is_some()
    }

    fn history(&self) -> Vec<History> {
        self.g
            .watch
            .recent(0)
            .into_iter()
            .map(|r| r.change)
            .collect()
    }

    fn closed(&self) -> Vec<Closed> {
        self.history()
            .into_iter()
            .filter_map(|h| match h {
                History::Closed(c) => Some(c),
                _ => None,
            })
            .collect()
    }

    fn asked(&self) -> Vec<&'static str> {
        std::mem::take(&mut *self.node.0.asked.lock().unwrap())
    }
}

/// A chain with a channel between 1 and 2 funded at height 1 (scid 1x1x0) and five blocks on
/// top, and the watcher following it with the channel verified.
async fn one_channel(config: &Config) -> (Net, u64, Transaction) {
    let node = FakeChain::default();
    mine_empty(&node, 1);
    let f = fund(1, 2, 1_000_000);
    let h = mine(&node, vec![f.clone()]);
    mine_empty(&node, 5);
    let n = net(&node, config, Replay::default());
    n.at(6).await;
    let id = scid(h, 1, 0);
    n.open(id, 1, 2).await;
    (n, id, f)
}

#[tokio::test]
async fn a_close_in_a_block_is_recorded_once_and_removes_the_channel() {
    let (n, id, f) = one_channel(&config()).await;
    let opened = n.history();
    let block1 = n.node.block_at(1).unwrap();
    assert_eq!(
        opened,
        vec![History::Opened(Opened {
            scid: id,
            node_one: channel(id, 1, 2).contents.node_id_1,
            node_two: channel(id, 1, 2).contents.node_id_2,
            capacity_sats: 1_000_000,
            block: BlockRef {
                height: 1,
                hash: block1.block_hash(),
                time: u64::from(block1.header.time),
            },
        })]
    );
    assert_eq!(n.g.watch.status().watched, 1);

    n.asked();
    let close = coop(
        OutPoint::new(f.compute_txid(), 0),
        1,
        2,
        vec![out(wpkh(), 999_500)],
    );
    let h = mine(&n.node, vec![close.clone()]);
    n.at(h).await;
    assert!(!n.in_graph(id));
    let c = n.closed();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].scid, id);
    assert_eq!(c[0].kind, CloseKind::Cooperative);
    assert_eq!(c[0].txid, close.compute_txid());
    assert_eq!(c[0].fee_sats, Some(500));
    assert_eq!(c[0].capacity_sats, 1_000_000);
    assert_eq!(c[0].opened.height, 1);
    assert_eq!(c[0].closed.height, h);
    assert_eq!(c[0].closed.hash, n.node.block_at(h).unwrap().block_hash());
    // The node's cost for the block: the tip, one page of headers, the block, and one position
    // check for the close. Tip polls repeat; nothing else is asked.
    let asked = n.asked();
    let count = |r| asked.iter().filter(|a| **a == r).count();
    assert_eq!(
        (count("headers"), count("block"), count("tx_position")),
        (1, 1, 1)
    );
    assert!(
        asked
            .iter()
            .all(|a| ["tip_hash", "headers", "block", "tx_position"].contains(a))
    );
    let s = n.g.watch.status();
    assert_eq!((s.cooperative, s.force, s.watched, s.opened), (1, 0, 0, 1));

    // Gossip for it keeps arriving: it stays out.
    let before = n.node.batches().len();
    assert!(
        n.g.handle_channel_announcement(None, &channel(id, 1, 2))
            .is_err()
    );
    assert!(n.g.handle_channel_update(None, &update(id, 1, 0)).is_err());
    mine_empty(&n.node, 1);
    n.at(h + 1).await;
    assert!(!n.in_graph(id));
    assert_eq!(n.node.batches().len(), before, "answered without the node");
    assert_eq!(n.closed().len(), 1, "recorded once");
}

#[tokio::test]
async fn several_closes_in_one_block_are_each_classified() {
    let node = FakeChain::default();
    mine_empty(&node, 1);
    let (fa, fb, fc) = (
        fund(1, 2, 1_000_000),
        fund(3, 4, 2_000_000),
        fund(5, 6, 3_000_000),
    );
    let h = mine(&node, vec![fa.clone(), fb.clone(), fc.clone()]);
    mine_empty(&node, 5);
    let n = net(&node, &config(), Replay::default());
    n.at(6).await;
    let (a, b, c) = (scid(h, 1, 0), scid(h, 2, 0), scid(h, 3, 0));
    n.open(a, 1, 2).await;
    n.open(b, 3, 4).await;
    n.open(c, 5, 6).await;

    let force = commitment(OutPoint::new(fb.compute_txid(), 0), 3, 4);
    // A splice-in: the funding output and a wallet input.
    let wallet = fund(9, 10, 50_000);
    let wh = mine(&node, vec![wallet.clone()]);
    n.at(wh).await;
    let splice = tx(
        vec![
            input(
                OutPoint::new(fc.compute_txid(), 0),
                0xffff_fffd,
                funding_witness(5, 6),
            ),
            input(
                OutPoint::new(wallet.compute_txid(), 0),
                0xffff_fffd,
                vec![vec![1; 64]],
            ),
        ],
        vec![out(funding(5, 6), 3_049_000)],
        wh,
    );
    let close = coop(
        OutPoint::new(fa.compute_txid(), 0),
        1,
        2,
        vec![out(wpkh(), 600_000), out(wpkh(), 399_000)],
    );
    let ch = mine(&node, vec![close, force.clone(), splice]);
    n.at(ch).await;
    let mut kinds: Vec<(u64, CloseKind, Option<u64>)> = n
        .closed()
        .iter()
        .map(|c| (c.scid, c.kind, c.fee_sats))
        .collect();
    kinds.sort_by_key(|k| k.0);
    assert_eq!(
        kinds,
        vec![
            (a, CloseKind::Cooperative, Some(1_000)),
            (b, CloseKind::Force, Some(2_000_000 - 900_330)),
            (c, CloseKind::Other, Some(1_000)),
        ]
    );
    assert!(!n.in_graph(a) && !n.in_graph(b) && !n.in_graph(c));
    // The commitment's `to_local` is watched for a penalty; the anchor and `to_remote` are not.
    assert_eq!(n.g.watch.status().sweeps, 1);
}

#[tokio::test]
async fn a_revoked_commitment_swept_by_its_victim_becomes_a_penalty_and_a_reorg_undoes_it() {
    let (n, id, f) = one_channel(&config()).await;
    let commit = commitment(OutPoint::new(f.compute_txid(), 0), 1, 2);
    let ch = mine(&n.node, vec![commit.clone()]);
    n.at(ch).await;
    assert_eq!(n.closed()[0].kind, CloseKind::Force);
    // A spend of the anchor says nothing; the sweep through the revocation path does.
    let ph = mine(&n.node, vec![sweep(&commit, true)]);
    n.at(ph).await;
    let s = n.g.watch.status();
    assert_eq!((s.force, s.penalty, s.sweeps), (0, 1, 0));
    assert!(matches!(
        n.history().last(),
        Some(History::Penalty { scid, .. }) if *scid == id
    ));
    // The sweep's block leaves the chain; an honest sweep replaces it.
    unmine(&n.node, ph);
    let honest = mine(&n.node, vec![sweep(&commit, false)]);
    mine_empty(&n.node, 1);
    n.at(honest + 1).await;
    let s = n.g.watch.status();
    assert_eq!(
        (s.force, s.penalty, s.sweeps, s.undone, s.reorgs),
        (1, 0, 1, 1, 1)
    );
    assert!(n.history().contains(&History::PenaltyUndone { scid: id }));
}

#[test]
fn revocation_and_funding_witnesses_are_told_apart() {
    let f = fund(1, 2, 1_000);
    let commit = commitment(OutPoint::new(f.compute_txid(), 0), 1, 2);
    assert!(revoked(&sweep(&commit, true).input[0]));
    assert!(!revoked(&sweep(&commit, false).input[0]));
    assert_eq!(
        funding_program(&commit.input[0]),
        crate::closes::program_of(&funding(1, 2))
    );
    assert_eq!(funding_program(&sweep(&commit, true).input[0]), None);
    // An HTLC output spent with the revocation key that its script names.
    let s = Secp256k1::new();
    let rk = key(7).public_key(&s).serialize();
    let h = lightning::bitcoin::hashes::hash160::Hash::hash(&rk);
    let mut script = vec![0x76, 0xa9, 0x14];
    script.extend(h.to_byte_array());
    script.extend([0x87, 0x63, 0xac, 0x67]);
    let spend = |key: Vec<u8>| {
        input(
            OutPoint::null(),
            0,
            vec![vec![0x30; 72], key, script.clone()],
        )
    };
    assert!(revoked(&spend(rk.to_vec())));
    assert!(!revoked(&spend(key(8).public_key(&s).serialize().to_vec())));
}

#[tokio::test]
async fn a_channel_verified_at_an_older_tip_is_checked_again() {
    // The lookup answered "unspent" at tip 6, but the watcher had already read block 7, where it
    // closed: the close is found through the node's spend records.
    let node = FakeChain::default();
    mine_empty(&node, 1);
    let f = fund(1, 2, 1_000_000);
    let h = mine(&node, vec![f.clone()]);
    mine_empty(&node, 5);
    let id = scid(h, 1, 0);
    node.add(id, &funding(1, 2), 1_000_000, false);
    node.0.stale_once.store(true, Ordering::Relaxed);
    let close = coop(
        OutPoint::new(f.compute_txid(), 0),
        1,
        2,
        vec![out(wpkh(), 999_000)],
    );
    let ch = mine(&node, vec![close.clone()]);
    let n = net(&node, &config(), Replay::default());
    n.at(ch).await;
    node.0.tip.store(6, Ordering::Relaxed);
    n.open(id, 1, 2).await;
    wait_for("closed", || !n.in_graph(id)).await;
    let c = n.closed();
    assert_eq!(
        (c.len(), c[0].closed.height, c[0].fee_sats),
        (1, ch, Some(1_000))
    );
    assert_eq!(c[0].txid, close.compute_txid());
}

#[tokio::test]
async fn closes_while_down_are_found_from_the_graph_file() {
    // The graph file holds two channels; one closed while we were down.
    let node = FakeChain::default();
    mine_empty(&node, 1);
    let (fa, fb) = (fund(1, 2, 1_000_000), fund(3, 4, 2_000_000));
    let h = mine(&node, vec![fa.clone(), fb]);
    mine_empty(&node, 5);
    let close = commitment(OutPoint::new(fa.compute_txid(), 0), 1, 2);
    let ch = mine(&node, vec![close.clone()]);
    mine_empty(&node, 3);
    let (a, b) = (scid(h, 1, 0), scid(h, 2, 0));
    let graph = Graph::new(NET, sink());
    for (id, x, y) in [(a, 1, 2), (b, 3, 4)] {
        graph
            .update_channel_from_announcement_no_lookup(&channel(id, x, y))
            .unwrap();
    }
    let n = net(&node, &config(), Replay::from_graph(&graph));
    wait_for("replayed", || {
        n.g.replay_tick(5000);
        !n.g.replaying()
    })
    .await;
    assert!(n.in_graph(b));
    n.at(ch + 3).await;
    let c = n.closed();
    assert_eq!(c.len(), 1);
    assert_eq!(
        (c[0].scid, c[0].kind, c[0].closed.height),
        (a, CloseKind::Force, ch)
    );
    assert_eq!(c[0].opened.height, h);
    assert_eq!(c[0].fee_sats, Some(1_000_000 - 900_330));
    assert_eq!(c[0].node_one, channel(a, 1, 2).contents.node_id_1);
    // Found without reading a block: the funding txid, its spender and the spender.
    let asked = n.asked();
    assert!(!asked.contains(&"block"), "{asked:?}");
    for r in ["block_hash", "block_txid", "outspend", "tx"] {
        assert_eq!(asked.iter().filter(|a| **a == r).count(), 1, "{r}");
    }
    assert!(!n.in_graph(a) && n.in_graph(b));
}

#[tokio::test]
async fn a_long_gap_is_caught_up_without_reading_its_blocks() {
    let (n, id, f) = one_channel(&config()).await;
    n.asked();
    // The watcher stops reading for a while (node unreachable); twelve blocks pass.
    n.node.0.down.store(true, Ordering::Relaxed);
    mine_empty(&n.node, 4);
    let close = coop(
        OutPoint::new(f.compute_txid(), 0),
        1,
        2,
        vec![out(wpkh(), 999_000)],
    );
    let ch = mine(&n.node, vec![close]);
    mine_empty(&n.node, 7);
    n.node.0.down.store(false, Ordering::Relaxed);
    n.at(ch + 7).await;
    wait_for("closed", || !n.in_graph(id)).await;
    assert_eq!(n.closed()[0].closed.height, ch);
    assert!(!n.asked().contains(&"block"));
}

#[tokio::test]
async fn a_reorg_undoes_a_close_and_the_new_chain_closes_it_again() {
    let (n, id, f) = one_channel(&config()).await;
    let nodes_before = n.g.graph().read_only().nodes().len();
    let fid = OutPoint::new(f.compute_txid(), 0);
    let ch = mine(&n.node, vec![coop(fid, 1, 2, vec![out(wpkh(), 999_000)])]);
    n.at(ch).await;
    assert!(!n.in_graph(id));
    // The close's block is replaced by one without it: the channel is back, with its capacity.
    unmine(&n.node, ch);
    mine_empty(&n.node, 2);
    n.at(ch + 1).await;
    assert!(n.history().contains(&History::CloseUndone { scid: id }));
    let cap =
        n.g.graph()
            .read_only()
            .channel(id)
            .and_then(|c| c.capacity_sats);
    assert_eq!(cap, Some(1_000_000));
    assert_eq!(n.g.graph().read_only().nodes().len(), nodes_before);
    let s = n.g.watch.status();
    assert_eq!((s.cooperative, s.undone, s.watched, s.reorgs), (0, 1, 1, 1));
    // The new chain confirms a force close instead.
    let fh = mine(&n.node, vec![commitment(fid, 1, 2)]);
    n.at(fh).await;
    assert!(!n.in_graph(id));
    let kinds: Vec<CloseKind> = n.closed().iter().map(|c| c.kind).collect();
    assert_eq!(kinds, [CloseKind::Cooperative, CloseKind::Force]);
    assert_eq!(n.g.watch.status().force, 1);
}

#[tokio::test]
async fn a_reorg_of_the_funding_block_undoes_the_open() {
    let node = FakeChain::default();
    mine_empty(&node, 1);
    let (fa, fb) = (fund(1, 2, 1_000_000), fund(3, 4, 2_000_000));
    let h = mine(&node, vec![fa.clone(), fb.clone()]);
    mine_empty(&node, 5);
    let n = net(&node, &config(), Replay::default());
    n.at(6).await;
    let (a, b) = (scid(h, 1, 0), scid(h, 2, 0));
    n.open(a, 1, 2).await;
    n.open(b, 3, 4).await;
    // Seven blocks replaced: the new block at the funding height holds only `fa`'s funding, at
    // the same position. `b`'s output no longer exists.
    unmine(&node, h);
    let nh = mine(&node, vec![fa]);
    mine_empty(&node, 6);
    n.at(nh + 6).await;
    let hist = n.history();
    assert!(hist.contains(&History::OpenUndone { scid: a }), "{hist:?}");
    assert!(hist.contains(&History::OpenUndone { scid: b }));
    let reopened = hist.iter().rev().find_map(|r| match r {
        History::Opened(o) if o.scid == a => Some(o.block),
        _ => None,
    });
    assert_eq!(
        reopened.map(|b| b.hash),
        Some(node.block_at(nh).unwrap().block_hash())
    );
    assert!(n.in_graph(a));
    assert!(!n.in_graph(b));
    assert_eq!(n.g.watch.status().watched, 1);
}

#[tokio::test]
async fn history_and_sweeps_are_bounded() {
    let mut config = config();
    // The stream's room is rounded up to a power of two.
    config.history = 4;
    let node = FakeChain::default();
    mine_empty(&node, 1);
    let funds: Vec<Transaction> = (0..5u8)
        .map(|i| fund(10 + 2 * i, 11 + 2 * i, 1_000_000))
        .collect();
    let h = mine(&node, funds.clone());
    mine_empty(&node, 5);
    let n = net(&node, &config, Replay::default());
    n.at(6).await;
    let mut rx = n.g.watch.subscribe();
    for i in 0..5u8 {
        n.open(scid(h, u32::from(i) + 1, 0), 10 + 2 * i, 11 + 2 * i)
            .await;
    }
    // A consumer past the stream's room rebuilds from the open set, current at the last record.
    let (seq, open) = n.g.watch.open();
    assert_eq!((seq, open.len()), (5, 5));
    let closes = funds
        .iter()
        .enumerate()
        .map(|(i, f)| {
            commitment(
                OutPoint::new(f.compute_txid(), 0),
                10 + 2 * i as u8,
                11 + 2 * i as u8,
            )
        })
        .collect();
    let ch = mine(&node, closes);
    n.at(ch).await;
    let kept = n.g.watch.recent(0);
    assert_eq!(
        kept.iter().map(|r| r.seq).collect::<Vec<_>>(),
        [7, 8, 9, 10]
    );
    assert_eq!(n.g.watch.recent(9).len(), 1);
    assert_eq!(n.g.watch.open(), (10, Vec::new()));
    // The stream had room for four, so this receiver lagged.
    assert!(matches!(
        rx.try_recv(),
        Err(broadcast::error::TryRecvError::Lagged(6))
    ));
    let s = n.g.watch.status();
    assert_eq!((s.force, s.sweeps), (5, MAX_SWEEPS));
}

#[tokio::test]
async fn channels_sharing_a_funding_script_are_told_apart_by_position() {
    // Two channels between the same nodes with the same funding keys: one script, two outputs.
    let node = FakeChain::default();
    mine_empty(&node, 1);
    let (fa, fb) = (fund(1, 2, 1_000_000), fund(1, 2, 2_000_000));
    let h = mine(&node, vec![fa.clone(), fb]);
    mine_empty(&node, 5);
    let n = net(&node, &config(), Replay::default());
    n.at(6).await;
    let (a, b) = (scid(h, 1, 0), scid(h, 2, 0));
    n.open(a, 1, 2).await;
    n.open(b, 1, 2).await;
    // A third output paying the same script, not a channel, spent first.
    let stray = fund(1, 2, 5_000);
    let sh = mine(&node, vec![stray.clone()]);
    let spend = coop(
        OutPoint::new(stray.compute_txid(), 0),
        1,
        2,
        vec![out(wpkh(), 4_000)],
    );
    let close = coop(
        OutPoint::new(fa.compute_txid(), 0),
        1,
        2,
        vec![out(wpkh(), 999_000)],
    );
    let ch = mine(&node, vec![spend, close]);
    n.at(ch).await;
    let c = n.closed();
    assert_eq!(c.iter().map(|c| c.scid).collect::<Vec<_>>(), [a]);
    assert!(!n.in_graph(a) && n.in_graph(b));
    assert_eq!(n.g.watch.status().watched, 1);
    assert!(sh < ch);
}

#[tokio::test]
async fn a_close_found_while_down_and_undone_by_a_reorg_is_watched_again() {
    let node = FakeChain::default();
    mine_empty(&node, 1);
    let fa = fund(1, 2, 1_000_000);
    let h = mine(&node, vec![fa.clone()]);
    mine_empty(&node, 5);
    let fid = OutPoint::new(fa.compute_txid(), 0);
    let ch = mine(&node, vec![coop(fid, 1, 2, vec![out(wpkh(), 999_000)])]);
    let a = scid(h, 1, 0);
    let graph = Graph::new(NET, sink());
    graph
        .update_channel_from_announcement_no_lookup(&channel(a, 1, 2))
        .unwrap();
    let n = net(&node, &config(), Replay::from_graph(&graph));
    wait_for("replayed", || {
        n.g.replay_tick(5000);
        !n.g.replaying()
    })
    .await;
    n.at(ch).await;
    assert_eq!(n.closed().len(), 1);
    // The closing block, which the watcher started from, is replaced without the close.
    unmine(&node, ch);
    mine_empty(&node, 2);
    n.at(ch + 1).await;
    assert!(n.history().contains(&History::CloseUndone { scid: a }));
    assert_eq!(n.g.watch.status().watched, 1);
    // The new chain force-closes it: recorded, though it never went back in the graph.
    let fh = mine(&node, vec![commitment(fid, 1, 2)]);
    n.at(fh).await;
    let kinds: Vec<CloseKind> = n.closed().iter().map(|c| c.kind).collect();
    assert_eq!(kinds, [CloseKind::Cooperative, CloseKind::Force]);
    assert_eq!(n.g.watch.status().watched, 0);
}

#[tokio::test]
async fn a_close_found_ahead_of_the_watcher_is_left_to_the_block_read() {
    // The watcher has read up to 7 and looks again only after a second. The replay finds the
    // channel spent in block 8 meanwhile: it is watched, not recorded, and reading block 8
    // records the close with what undoes it.
    let node = FakeChain::default();
    mine_empty(&node, 1);
    let fa = fund(1, 2, 1_000_000);
    let h = mine(&node, vec![fa.clone()]);
    mine_empty(&node, 6);
    let a = scid(h, 1, 0);
    let graph = Graph::new(NET, sink());
    graph
        .update_channel_from_announcement_no_lookup(&channel(a, 1, 2))
        .unwrap();
    let config = quick(|c| c.tip_every = Duration::from_secs(1));
    let n = net(&node, &config, Replay::from_graph(&graph));
    // Not caught up: the graph file is not replayed yet.
    wait_for("first look", || n.g.watch.status().height == Some(7)).await;
    let fid = OutPoint::new(fa.compute_txid(), 0);
    let ch = mine(&node, vec![coop(fid, 1, 2, vec![out(wpkh(), 999_000)])]);
    wait_for("replayed", || {
        n.g.replay_tick(5000);
        !n.g.replaying()
    })
    .await;
    n.at(ch).await;
    assert_eq!(n.closed().len(), 1);
    assert_eq!(
        n.g.watch.status().blocks,
        1,
        "recorded by reading the block"
    );
    // That block leaves the chain: the close is undone.
    unmine(&node, ch);
    mine_empty(&node, 2);
    n.at(ch + 1).await;
    assert!(n.history().contains(&History::CloseUndone { scid: a }));
    assert_eq!(n.g.watch.status().watched, 1);
}

#[test]
fn real_mainnet_closes_are_classified() {
    let fixtures = include_str!("../../../../fixtures/lightning/mainnet/closes.txt");
    let mut seen = Vec::new();
    for line in fixtures.lines().filter(|l| !l.starts_with('#')) {
        let mut f = line.split(' ');
        let (kind, txid, hex) = (f.next().unwrap(), f.next().unwrap(), f.next().unwrap());
        let tx: Transaction = lightning::bitcoin::consensus::encode::deserialize_hex(hex).unwrap();
        assert_eq!(tx.compute_txid().to_string(), txid);
        let funding = tx.input.iter().position(|i| funding_program(i).is_some());
        let got = funding.map(|i| format!("{:?}", crate::classify(&tx, i)));
        assert_eq!(got.as_deref().unwrap_or("None"), kind, "{txid}");
        assert!(!tx.input.iter().any(revoked), "{txid}");
        seen.push(kind);
    }
    assert_eq!(
        seen,
        [
            "Cooperative",
            "Cooperative",
            "Force",
            "Force",
            "None",
            "Other"
        ]
    );
}

// Load on the node (docs/LIGHTNING.md): one gate for every Lightning request.

/// Announces `n` channels funded at height 1 of a chain six blocks deep, with validation, the
/// close-watcher and a third loop standing in for the funding fill all asking the node through
/// one gate; returns when all are verified.
async fn three_loops(node: &FakeChain, config: &Config, n: u32) {
    mine_empty(node, 1);
    let funds: Vec<Transaction> = (0..n).map(|_| fund(1, 2, 1_000_000)).collect();
    let h = mine(node, funds);
    mine_empty(node, 5);
    let (g, _, _) = gossip(config, Replay::default());
    let pace = pace(config);
    tokio::spawn(chain::run(Arc::clone(&g), node.clone(), Arc::clone(&pace)));
    tokio::spawn(closes::run(
        Arc::clone(&g),
        node.clone(),
        Arc::clone(&pace),
        config.tip_every,
    ));
    let filler = node.clone();
    let fill = tokio::spawn(async move {
        loop {
            let _ = pace.run(crate::Chain::tip_height(&filler)).await;
        }
    });
    for i in 0..n {
        let _ = g.handle_channel_announcement(None, &channel(scid(h, i + 1, 0), 1, 2));
    }
    wait_for("all verified", || {
        g.graph().read_only().channels().len() == n as usize
    })
    .await;
    fill.abort();
}

/// Most requests started within any one second.
fn per_second(starts: &[std::time::Instant]) -> usize {
    starts
        .iter()
        .map(|a| {
            starts
                .iter()
                .filter(|b| **b >= *a && **b - *a < Duration::from_secs(1))
                .count()
        })
        .max()
        .unwrap_or(0)
}

#[tokio::test]
async fn a_fast_node_gets_one_request_at_a_time_at_most_four_a_second() {
    let node = FakeChain::default();
    let config = quick(|c| {
        c.lookup_interval = Config::new(NET, PathBuf::new()).lookup_interval;
        c.tip_every = Duration::from_millis(20);
    });
    three_loops(&node, &config, 300).await;
    let starts = node.0.starts.lock().unwrap().clone();
    assert_eq!(node.0.max_in_flight.load(Ordering::SeqCst), 1);
    for w in starts.windows(2) {
        assert!(w[1] - w[0] >= config.lookup_interval, "{:?}", w[1] - w[0]);
    }
    assert!(per_second(&starts) <= 4, "{}", per_second(&starts));
    // 100 positions a request: three requests for 300 channels.
    let sizes: Vec<usize> = node.batches().iter().map(|b| b.1).collect();
    assert_eq!(sizes, [100, 100, 100]);
}

#[tokio::test]
async fn a_slow_node_is_asked_less() {
    let node = FakeChain::default();
    let took = Duration::from_millis(150);
    *node.0.delay.lock().unwrap() = took;
    let config = quick(|c| {
        c.lookup_interval = Duration::from_millis(250);
        c.tip_every = Duration::from_millis(20);
    });
    three_loops(&node, &config, 200).await;
    let starts = node.0.starts.lock().unwrap().clone();
    assert_eq!(node.0.max_in_flight.load(Ordering::SeqCst), 1);
    // 150 ms answering, then 450 ms left alone: busy a quarter of the time, not 60 %.
    for w in starts.windows(2) {
        assert!(w[1] - w[0] >= took * 4, "{:?}", w[1] - w[0]);
    }
    assert!(per_second(&starts) <= 2, "{}", per_second(&starts));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_takes_stored_answers_and_reads_on_from_the_saved_block() {
    let dir = TempDir::new();
    let node = FakeChain::default();
    mine_empty(&node, 1);
    let (fa, fb) = (fund(1, 2, 1_000_000), fund(3, 4, 2_000_000));
    let h = mine(&node, vec![fa.clone(), fb]);
    mine_empty(&node, 5);
    let (a, b) = (scid(h, 1, 0), scid(h, 2, 0));
    // A graph file holding both, as gossip left it.
    let graph = Graph::new(NET, sink());
    for (id, x, y) in [(a, 1, 2), (b, 3, 4)] {
        graph
            .update_channel_from_announcement_no_lookup(&channel(id, x, y))
            .unwrap();
    }
    let bytes = graph.encode();
    store::write_atomic(&dir.0.join(store::GRAPH_FILE), |f| {
        std::io::Write::write_all(f, &bytes)
    })
    .unwrap();
    let mut config = config();
    config.data_dir = dir.0.clone();
    config.peers.clear();

    // First life: both checked with the node, then saved at shutdown.
    let (peer, _) = Peer::start(config.clone(), node.clone()).unwrap();
    let status = peer.status();
    wait_for("checked", || {
        let s = status.borrow();
        s.synced && s.validation.verified == 2 && s.closes.caught_up
    })
    .await;
    peer.shutdown().await.unwrap();
    let looked_up = |n: &FakeChain| n.batches().iter().map(|b| b.1).sum::<usize>();
    assert_eq!(looked_up(&node), 2);
    assert!(dir.0.join(store::VERIFIED_FILE).exists());

    // Down for three blocks; `a` closes in the first.
    let close = coop(
        OutPoint::new(fa.compute_txid(), 0),
        1,
        2,
        vec![out(wpkh(), 999_000)],
    );
    let ch = mine(&node, vec![close]);
    mine_empty(&node, 2);
    node.0.asked.lock().unwrap().clear();

    // Second life: no channel asked of the node again, the close found by reading on.
    let (peer, _) = Peer::start(config.clone(), node.clone()).unwrap();
    let status = peer.status();
    wait_for("caught up", || {
        let s = status.borrow();
        s.synced && s.closes.height == Some(ch + 2) && s.closes.caught_up
    })
    .await;
    assert_eq!(looked_up(&node), 2, "a stored answer was asked again");
    peer.read(|g| {
        assert!(g.channel(a).is_none());
        assert_eq!(g.channel(b).unwrap().capacity_sats, Some(2_000_000));
    });
    let closed: Vec<(u64, u32)> = peer
        .recent(0)
        .into_iter()
        .filter_map(|r| match r.change {
            History::Closed(c) => Some((c.scid, c.closed.height)),
            _ => None,
        })
        .collect();
    assert_eq!(closed, [(a, ch)]);
    // The three blocks were read: no catch-up of every channel by position.
    let asked = node.0.asked.lock().unwrap().clone();
    assert_eq!(
        asked.iter().filter(|r| **r == "block").count(),
        3,
        "{asked:?}"
    );
    peer.shutdown().await.unwrap();
}

#[test]
fn saved_answers_round_trip_and_a_bad_file_is_ignored() {
    let dir = TempDir::new();
    let v = store::Verified {
        height: 900,
        hash: BlockHash::from_byte_array([9; 32]),
        channels: (0..3)
            .map(|i| store::Saved {
                scid: scid(800, i, 1),
                capacity: 1_000 + u64::from(i),
                program: [i as u8; 32],
                block: closes::BlockRef {
                    height: 800,
                    hash: BlockHash::from_byte_array([8; 32]),
                    time: 1_700_000_000,
                },
            })
            .collect(),
    };
    store::write_verified(&dir.0, NET, &v).unwrap();
    assert_eq!(store::load_verified(&dir.0, NET, 10), Some(v));
    // Another network's, or more channels than the ceiling: as if there were none.
    assert_eq!(store::load_verified(&dir.0, Network::Bitcoin, 10), None);
    assert_eq!(store::load_verified(&dir.0, NET, 2), None);
    // Cut short.
    let path = dir.0.join(store::VERIFIED_FILE);
    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(&path, &bytes[..bytes.len() - 1]).unwrap();
    assert_eq!(store::load_verified(&dir.0, NET, 10), None);
    std::fs::write(&path, b"").unwrap();
    assert_eq!(store::load_verified(&dir.0, NET, 10), None);
}

#[tokio::test]
async fn saved_answers_are_forgotten_once_the_watcher_reads_on() {
    // Saved at height 8; while down, the node moved to a shorter branch whose block 7 closes
    // the channel.
    let node = FakeChain::default();
    mine_empty(&node, 1);
    let f = fund(1, 2, 1_000_000);
    let h = mine(&node, vec![f.clone()]);
    mine_empty(&node, 7);
    let id = scid(h, 1, 0);
    let funded = node.block_at(h).unwrap();
    let saved = node.block_at(8).unwrap().block_hash();
    let v = store::Verified {
        height: 8,
        hash: saved,
        channels: vec![store::Saved {
            scid: id,
            capacity: 1_000_000,
            program: closes::program_of(&funding(1, 2)).unwrap(),
            block: closes::BlockRef {
                height: h,
                hash: funded.block_hash(),
                time: u64::from(funded.header.time),
            },
        }],
    };
    unmine(&node, 7);
    let close = coop(
        OutPoint::new(f.compute_txid(), 0),
        1,
        2,
        vec![out(wpkh(), 999_000)],
    );
    mine(&node, vec![close]);
    let graph = Graph::new(NET, sink());
    graph
        .update_channel_from_announcement_no_lookup(&channel(id, 1, 2))
        .unwrap();
    let config = config();
    let (g, _, _) = gossip(&config, Replay::from_graph(&graph));
    g.lookup.restore(&v);
    g.watch.resume(v.height, v.hash);
    let pace = pace(&config);
    tokio::spawn(chain::run(Arc::clone(&g), node.clone(), Arc::clone(&pace)));
    tokio::spawn(closes::run(
        Arc::clone(&g),
        node.clone(),
        pace,
        config.tip_every,
    ));
    // The replay hands the channel back only after the watcher stopped waiting for it.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    wait_for("replayed", || {
        g.replay_tick(5000);
        !g.replaying()
    })
    .await;
    // Asked of the node, which says spent: never shown as open.
    assert!(g.graph().read_only().channel(id).is_none());
    assert_eq!(rejected(&g).spent, 1);
}
