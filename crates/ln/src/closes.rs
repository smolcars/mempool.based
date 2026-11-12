//! Channel opens and closes (`docs/LIGHTNING.md`, Opens and closes).
//!
//! Every verified channel's funding output is watched. A funding output pays the P2WSH hash of
//! the two-of-two script of the announced keys, and spending it reveals that script as the last
//! witness item. So the watcher reads each new block once and, for every input whose witness ends
//! with a two-of-two script, looks up the script's hash in the set of watched outputs: one block
//! fetch per block and no work per channel. A match is confirmed by asking where the spent
//! transaction is (its height and index must be the channel's), since a funding script can be
//! paid twice. The spending transaction's shape says how the channel closed ([`classify`]).
//!
//! Blocks are followed one by one from the first tip seen, and a window of the newest is kept
//! with what each changed, so a reorg undoes exactly what the blocks it removes did. Channels that
//! may have closed in blocks never read (downtime, a long gap, the first start) are found by
//! [`Watcher::check`] through the node's spend records instead, rate-limited like validation.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lightning::bitcoin::hashes::{Hash, hash160, sha256};
use lightning::bitcoin::{BlockHash, OutPoint, Script, ScriptBuf, Transaction, TxIn, Txid};
use lightning::ln::msgs::{ChannelUpdate, UnsignedNodeAnnouncement};
use lightning::types::features::ChannelFeatures;
use node::esplora::model::Block as Header;
use node::esplora::{Error, OutPositions};
use tokio::sync::broadcast;

use crate::chain::{Chain, LOOKUP_BATCH, position};
use crate::gossip::Gossip;
use crate::pace::Pace;
use crate::store::{Saved, Verified};
use crate::{Change, Event, NodeId};

/// New blocks read one by one, at most. Past this many the catch-up checks every watched output
/// instead: about 18 MB for 36,000 channels, the size of ten blocks.
const MAX_GAP: usize = 10;
/// Newest blocks kept with what they changed, so a reorg up to this deep is undone exactly.
const WINDOW: usize = 100;
/// A force close's outputs are watched this long for a penalty: the longest `to_self_delay`
/// implementations accept.
const SWEEP_BLOCKS: u32 = 2016;
/// Force closes watched for a penalty at once; the oldest is dropped beyond.
pub(crate) const MAX_SWEEPS: usize = if cfg!(test) { 3 } else { 10_000 };
/// Anchor outputs (BOLT 3, `option_anchors`): anyone may sweep them, so their spends say nothing.
const ANCHOR_SATS: u64 = 330;
const RETRY_MIN: Duration = Duration::from_secs(1);
const RETRY_MAX: Duration = Duration::from_secs(60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// After a restart, how long the watcher waits for the replay before it reads blocks.
const RESUME_WAIT: Duration = Duration::from_secs(120);

/// A block, as a history record names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockRef {
    pub height: u32,
    pub hash: BlockHash,
    /// The header's time, unix seconds.
    pub time: u64,
}

/// How a channel closed, as far as the chain tells (`docs/LIGHTNING.md` says how sure each is).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CloseKind {
    /// Not a commitment transaction, and its only input is the funding output: both sides
    /// signed a closing transaction. A splice that only takes funds out looks the same.
    Cooperative,
    /// A commitment transaction (BOLT 3 marks every one in its locktime and input sequence): one
    /// side closed alone.
    Force,
    /// A force close whose commitment had been revoked: one of its outputs was swept through the
    /// revocation path. Seen only once that sweep confirms; until then the close reads `Force`.
    Penalty,
    /// Neither: more inputs than the funding output, which no closing protocol builds. Most
    /// likely a splice.
    Other,
}

/// A channel known to be open: its funding output was found unspent and six deep.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    pub scid: u64,
    pub node_one: NodeId,
    pub node_two: NodeId,
    pub capacity_sats: u64,
    /// The funding transaction's block.
    pub block: BlockRef,
}

/// A channel whose funding output was spent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Closed {
    pub scid: u64,
    pub node_one: NodeId,
    pub node_two: NodeId,
    pub capacity_sats: u64,
    pub opened: BlockRef,
    pub closed: BlockRef,
    /// The spending transaction.
    pub txid: Txid,
    pub kind: CloseKind,
    /// The spending transaction's fee. `None` when it has other inputs and the node did not say.
    pub fee_sats: Option<u64>,
}

/// One change to the channel history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum History {
    Opened(Opened),
    Closed(Closed),
    /// A force close was a breach: `txid` swept one of its commitment's outputs through the
    /// revocation path in `block`. The close's kind is now [`CloseKind::Penalty`].
    Penalty {
        scid: u64,
        txid: Txid,
        block: BlockRef,
    },
    /// The funding block left the best chain. An [`History::Opened`] with the new block follows
    /// if the funding output is still at the same position; otherwise the channel is gone.
    OpenUndone {
        scid: u64,
    },
    /// The closing block left the best chain: the channel is open again (and back in the graph
    /// if it was there), until a close is recorded anew.
    CloseUndone {
        scid: u64,
    },
    /// The sweep's block left the best chain: the close reads `Force` again.
    PenaltyUndone {
        scid: u64,
    },
}

/// A history change and its sequence number, which grows by one per record from 1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub seq: u64,
    pub change: History,
}

/// Where the close-watcher stands, and its counts since start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Closes {
    /// The newest block read.
    pub height: Option<u32>,
    /// The node had no newer block at the last look, and nothing waits for the catch-up.
    pub caught_up: bool,
    /// Funding outputs watched: the verified channels.
    pub watched: usize,
    /// Force closes whose outputs are watched for a penalty.
    pub sweeps: usize,
    /// Channels waiting for the catch-up: to check whether their output is spent, or to find where.
    pub catch_up: usize,
    pub opened: u64,
    pub cooperative: u64,
    pub force: u64,
    pub penalty: u64,
    pub other: u64,
    /// Records undone because their block left the best chain.
    pub undone: u64,
    pub reorgs: u64,
    /// Blocks read in full.
    pub blocks: u64,
    /// Requests to the node, and failed ones in a row.
    pub requests: u64,
    pub node_failures: u32,
}

/// How a transaction spending a funding output closed the channel. `input` spends it.
pub fn classify(tx: &Transaction, input: usize) -> CloseKind {
    let commitment = tx.lock_time.to_consensus_u32() >> 24 == 0x20
        && tx
            .input
            .get(input)
            .is_some_and(|i| i.sequence.0 >> 24 == 0x80);
    if commitment {
        CloseKind::Force
    } else if tx.input.len() == 1 {
        CloseKind::Cooperative
    } else {
        CloseKind::Other
    }
}

/// The P2WSH program an input spends, when its witness ends with a two-of-two of compressed keys
/// (`2 <key> <key> 2 OP_CHECKMULTISIG`, BOLT 3's funding script): a spend of what may be a
/// Lightning funding output.
pub fn funding_program(input: &TxIn) -> Option<[u8; 32]> {
    let s = input.witness.last()?;
    let two_of_two = s.len() == 71
        && s.starts_with(&[0x52, 0x21])
        && s.get(35) == Some(&0x21)
        && s.ends_with(&[0x52, 0xae]);
    two_of_two.then(|| sha256::Hash::hash(s).to_byte_array())
}

/// The short channel id of an output position, if it fits one.
fn scid_of(height: u32, index: u32, vout: u32) -> Option<u64> {
    (height < 1 << 24 && index < 1 << 24 && vout < 1 << 16)
        .then(|| u64::from(height) << 40 | u64::from(index) << 16 | u64::from(vout))
}

/// The program of a P2WSH output script.
pub(crate) fn program_of(script: &Script) -> Option<[u8; 32]> {
    if !script.is_p2wsh() {
        return None;
    }
    script.as_bytes().get(2..34)?.try_into().ok()
}

/// Whether an input spends a commitment output through its revocation path (BOLT 3): `to_local`
/// with `<sig> 1 <script>`, or an HTLC output with `<sig> <revocationpubkey> <script>`, the key
/// hashing to the one the script names.
pub fn revoked(input: &TxIn) -> bool {
    let w = &input.witness;
    let (Some(script), Some(arg), 3) = (w.last(), w.nth(1), w.len()) else {
        return false;
    };
    // OP_IF <revocationpubkey> OP_ELSE … OP_ENDIF OP_CHECKSIG
    let to_local = script.starts_with(&[0x63, 0x21]) && script.ends_with(&[0x68, 0xac]);
    // OP_DUP OP_HASH160 <20> OP_EQUAL OP_IF OP_CHECKSIG
    let htlc = script.starts_with(&[0x76, 0xa9, 0x14])
        && script.get(23..26) == Some(&[0x87, 0x63, 0xac][..]);
    (to_local && arg == [1])
        || (htlc
            && arg.len() == 33
            && script.get(3..23) == Some(&hash160::Hash::hash(arg).to_byte_array()[..]))
}

/// A watched funding output: the channel's nodes, value and block.
#[derive(Clone, Debug)]
pub(crate) struct Funding {
    pub(crate) nodes: (NodeId, NodeId),
    pub(crate) capacity: u64,
    pub(crate) program: [u8; 32],
    pub(crate) block: BlockRef,
    /// The tip its lookup was answered at: unspent up to there.
    pub(crate) verified_at: u32,
}

/// What puts a closed channel back in the graph if its close leaves the best chain.
struct Snapshot {
    features: ChannelFeatures,
    updates: Vec<ChannelUpdate>,
    /// Announcements of its nodes, applied again to any that left the graph with it.
    nodes: Vec<UnsignedNodeAnnouncement>,
}

/// A close, as a block's undo record keeps it.
struct Undo {
    scid: u64,
    funding: Funding,
    /// `None` for a channel that was not in the graph (found by the catch-up).
    snapshot: Option<Snapshot>,
    kind: CloseKind,
}

/// A force close's commitment outputs watched for a penalty, until a height.
#[derive(Clone)]
struct Sweeps {
    until: u32,
    outs: Vec<OutPoint>,
}

/// A block read, with what it changed.
struct Seen {
    height: u32,
    hash: BlockHash,
    closes: Vec<Undo>,
    penalties: Vec<(u64, Sweeps)>,
}

/// A spent funding output to find the spend of. `in_graph`: still watched, rather than a stored
/// channel the replay found spent.
#[derive(Clone)]
struct Find {
    scid: u64,
    funding: Funding,
    in_graph: bool,
}

#[derive(Default)]
struct State {
    funding: HashMap<u64, Funding>,
    /// (program, scid) for every watched output: a script can fund more than one channel.
    programs: BTreeSet<([u8; 32], u64)>,
    /// Channels from the graph file not yet answered, with their nodes.
    stored: HashMap<u64, (NodeId, NodeId)>,
    /// Watched channels to check: is the output spent?
    recheck: BTreeSet<u64>,
    find: VecDeque<Find>,
    sweeps: HashMap<u64, Sweeps>,
    swept: HashMap<OutPoint, u64>,
    seen: VecDeque<Seen>,
    /// The block being read, once its inputs were matched: a channel verified below it was not
    /// among them, and is checked again.
    reading: Option<(u32, BlockHash)>,
    /// Whether the last look at the node found no newer block.
    at_tip: bool,
    history: VecDeque<Record>,
    seq: u64,
    counts: Closes,
}

impl State {
    fn hash_at(&self, height: u32) -> Option<BlockHash> {
        let first = self.seen.front()?.height;
        let i = usize::try_from(height.checked_sub(first)?).ok()?;
        self.seen.get(i).map(|b| b.hash)
    }

    fn watch(&mut self, scid: u64, f: Funding) {
        self.programs.insert((f.program, scid));
        self.funding.insert(scid, f);
    }

    fn unwatch(&mut self, scid: u64) -> Option<Funding> {
        let f = self.funding.remove(&scid)?;
        self.programs.remove(&(f.program, scid));
        Some(f)
    }

    fn add_sweeps(&mut self, scid: u64, sweeps: Sweeps) {
        if self.sweeps.len() >= MAX_SWEEPS
            && let Some(oldest) = self
                .sweeps
                .iter()
                .min_by_key(|(_, s)| s.until)
                .map(|(k, _)| *k)
        {
            self.drop_sweeps(oldest);
        }
        for o in &sweeps.outs {
            self.swept.insert(*o, scid);
        }
        self.sweeps.insert(scid, sweeps);
    }

    fn drop_sweeps(&mut self, scid: u64) -> Option<Sweeps> {
        let s = self.sweeps.remove(&scid)?;
        for o in &s.outs {
            self.swept.remove(o);
        }
        Some(s)
    }

    fn count(&mut self, kind: CloseKind, by: i64) {
        let c = &mut self.counts;
        let n = match kind {
            CloseKind::Cooperative => &mut c.cooperative,
            CloseKind::Force => &mut c.force,
            CloseKind::Penalty => &mut c.penalty,
            CloseKind::Other => &mut c.other,
        };
        *n = n.saturating_add_signed(by);
    }
}

/// The watched outputs, the catch-up queue, the block window and the history. Shared by the
/// gossip handler (which registers verified channels) and [`run`].
pub(crate) struct Watch {
    state: Mutex<State>,
    history: broadcast::Sender<Record>,
    cap: usize,
}

impl Watch {
    pub(crate) fn new(cap: usize, stored: HashMap<u64, (NodeId, NodeId)>) -> Self {
        Self {
            state: Mutex::new(State {
                stored,
                ..State::default()
            }),
            history: broadcast::channel(cap.max(1)).0,
            cap,
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn record(&self, s: &mut State, change: History) {
        s.seq += 1;
        let r = Record { seq: s.seq, change };
        if self.cap > 0 {
            if s.history.len() == self.cap {
                s.history.pop_front();
            }
            s.history.push_back(r.clone());
        }
        let _ = self.history.send(r);
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<Record> {
        self.history.subscribe()
    }

    /// Records kept, after sequence number `after`.
    pub(crate) fn recent(&self, after: u64) -> Vec<Record> {
        let s = self.lock();
        s.history
            .iter()
            .filter(|r| r.seq > after)
            .cloned()
            .collect()
    }

    /// Every watched channel as an [`Opened`] record, and the sequence number of the last record
    /// sent: the open channels as of that record, for a consumer that fell too far behind.
    pub(crate) fn open(&self) -> (u64, Vec<Opened>) {
        let s = self.lock();
        let open = s
            .funding
            .iter()
            .map(|(scid, f)| Opened {
                scid: *scid,
                node_one: f.nodes.0,
                node_two: f.nodes.1,
                capacity_sats: f.capacity,
                block: f.block,
            })
            .collect();
        (s.seq, open)
    }

    pub(crate) fn status(&self) -> Closes {
        let s = self.lock();
        let catch_up = s.recheck.len() + s.find.len();
        Closes {
            height: s.seen.back().map(|b| b.height),
            caught_up: s.at_tip && catch_up == 0 && s.stored.is_empty(),
            watched: s.funding.len(),
            sweeps: s.sweeps.len(),
            catch_up,
            ..s.counts
        }
    }

    /// A channel entered the graph verified: watch its output, and record the open once. If
    /// blocks after its lookup's tip were already read without it, or its lookup's tip
    /// (`tip_hash` at `verified_at`) is not the block read at that height, check it again.
    pub(crate) fn opened(&self, scid: u64, f: Funding, tip_hash: BlockHash) {
        let mut s = self.lock();
        if s.funding.contains_key(&scid) {
            return;
        }
        let reading = s.reading;
        let read = s.seen.back().map(|b| b.height).max(reading.map(|r| r.0));
        let here = s
            .hash_at(f.verified_at)
            .or(reading.filter(|r| r.0 == f.verified_at).map(|r| r.1));
        if read.is_some_and(|h| h > f.verified_at) || here.is_some_and(|h| h != tip_hash) {
            s.recheck.insert(scid);
        }
        let open = Opened {
            scid,
            node_one: f.nodes.0,
            node_two: f.nodes.1,
            capacity_sats: f.capacity,
            block: f.block,
        };
        s.watch(scid, f);
        s.counts.opened += 1;
        self.record(&mut s, History::Opened(open));
    }

    /// The node answered for a channel. One from the graph file whose output is now spent closed
    /// while we were down: find where. `spent` carries the output's value, script and block.
    pub(crate) fn answered(
        &self,
        scid: u64,
        early: bool,
        spent: Option<(u64, [u8; 32], BlockRef)>,
    ) {
        let mut s = self.lock();
        if early || s.stored.is_empty() {
            return;
        }
        let Some(nodes) = s.stored.remove(&scid) else {
            return;
        };
        if let Some((capacity, program, block)) = spent {
            let funding = Funding {
                nodes,
                capacity,
                program,
                block,
                verified_at: 0,
            };
            s.find.push_back(Find {
                scid,
                funding,
                in_graph: false,
            });
        }
    }

    /// What a restart may take as answered: the newest block read, and every watched channel
    /// not waiting to be checked again, each unspent up to that block. `None` before the first
    /// block was read.
    pub(crate) fn saved(&self) -> Option<Verified> {
        let s = self.lock();
        let last = s.seen.back()?;
        let finding: HashSet<u64> = s.find.iter().map(|f| f.scid).collect();
        let channels = s
            .funding
            .iter()
            .filter(|(scid, _)| !s.recheck.contains(scid) && !finding.contains(scid))
            .map(|(scid, f)| Saved {
                scid: *scid,
                capacity: f.capacity,
                program: f.program,
                block: f.block,
            })
            .collect();
        Some(Verified {
            height: last.height,
            hash: last.hash,
            channels,
        })
    }

    /// Starts from the block a saved file names, as if it had just been read: the watcher reads
    /// on from there, and the channels answered from the file are not checked again.
    pub(crate) fn resume(&self, height: u32, hash: BlockHash) {
        self.lock().seen.push_back(Seen {
            height,
            hash,
            closes: Vec::new(),
            penalties: Vec::new(),
        });
    }

    /// The graph file's channels were all answered: none is still to come.
    pub(crate) fn replay_done(&self) {
        let mut s = self.lock();
        if !s.stored.is_empty() {
            s.stored = HashMap::new();
        }
    }

    /// The channel left the graph (pruned): stop watching it.
    pub(crate) fn forget(&self, scid: u64) {
        self.lock().unwatch(scid);
    }

    fn has_checks(&self) -> bool {
        let s = self.lock();
        !s.recheck.is_empty() || !s.find.is_empty()
    }
}

/// Follows the chain and runs the catch-up until the peer stops. Requests go through the gate
/// every Lightning request shares; the tip is read every `tip_every`.
pub(crate) async fn run<C: Chain>(
    gossip: Arc<Gossip>,
    chain: C,
    pace: Arc<Pace>,
    tip_every: Duration,
) {
    let w = Watcher {
        gossip,
        chain,
        pace,
    };
    // Resumed from the block it had read before a restart: the replay first hands back the
    // channels the saved file answers, or the blocks read meanwhile would send them all to be
    // checked again. Bounded, since a channel handed back later is only checked again.
    let resumed = !w.watch().lock().seen.is_empty();
    let until = Instant::now() + RESUME_WAIT;
    while resumed && w.gossip.replay_left() > 0 && Instant::now() < until {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let mut next_tip = Instant::now();
    loop {
        if !w.gossip.replaying() {
            w.gossip.watch.replay_done();
        }
        // A look at the tip when due, then one catch-up step: neither starves the other.
        let mut r = None;
        if Instant::now() >= next_tip {
            next_tip = Instant::now() + tip_every;
            r = Some(w.follow().await);
        }
        if r.as_ref().is_none_or(Result::is_ok) && w.gossip.watch.has_checks() {
            r = Some(w.check().await);
        }
        let Some(r) = r else {
            tokio::time::sleep_until(next_tip.into()).await;
            continue;
        };
        let failures = {
            let mut s = w.gossip.watch.lock();
            let c = &mut s.counts;
            c.node_failures = if r.is_ok() {
                0
            } else {
                c.node_failures.saturating_add(1)
            };
            c.node_failures
        };
        if let Err(error) = r {
            let _ = w
                .gossip
                .events
                .send(Event::CloseWatchFailed { failures, error });
            let backoff = RETRY_MIN
                .saturating_mul(1 << failures.saturating_sub(1).min(16))
                .min(RETRY_MAX);
            tokio::time::sleep(backoff).await;
            next_tip = Instant::now();
        }
    }
}

struct Watcher<C> {
    gossip: Arc<Gossip>,
    chain: C,
    pace: Arc<Pace>,
}

impl<C: Chain> Watcher<C> {
    /// One request to the node: paced, timed out and counted.
    async fn ask<T>(
        &self,
        call: impl Future<Output = Result<T, Error>>,
    ) -> Result<T, &'static str> {
        self.ask_found(call).await?.ok_or(Error::NotFound.label())
    }

    /// [`Self::ask`], with what the node does not know as `None` rather than an error, for the
    /// lookups that must not be retried forever.
    async fn ask_found<T>(
        &self,
        call: impl Future<Output = Result<T, Error>>,
    ) -> Result<Option<T>, &'static str> {
        self.gossip.watch.lock().counts.requests += 1;
        match self
            .pace
            .run(tokio::time::timeout(REQUEST_TIMEOUT, call))
            .await
        {
            Ok(Ok(v)) => Ok(Some(v)),
            Ok(Err(Error::NotFound)) => Ok(None),
            Ok(Err(e)) => Err(e.label()),
            Err(_) => Err("timeout"),
        }
    }

    fn watch(&self) -> &Watch {
        &self.gossip.watch
    }

    /// Brings the block window in line with the node's best chain: undoes blocks that left it,
    /// reads new ones, or hands a gap too long to read to the catch-up.
    async fn follow(&self) -> Result<(), &'static str> {
        let tip = self.ask(self.chain.tip_hash()).await?;
        let (last, low) = {
            let s = self.watch().lock();
            (
                s.seen.back().map(|b| (b.height, b.hash)),
                s.seen.front().map(|b| b.height),
            )
        };
        if last.map(|l| l.1) == Some(tip) {
            self.watch().lock().at_tip = true;
            return Ok(());
        }
        // Headers from the tip down to one we read (the fork), newest first.
        let mut new: Vec<Header> = Vec::new();
        let mut first: Option<Header> = None;
        let mut fork = None;
        let mut top = None;
        'walk: loop {
            let page = self.ask(self.chain.headers(top)).await?;
            if page.is_empty() {
                return Err("malformed");
            }
            for h in page {
                if new
                    .last()
                    .is_some_and(|n| n.previousblockhash != Some(h.id))
                {
                    return Err("chain_moved");
                }
                first.get_or_insert_with(|| h.clone());
                let (Some((last_height, _)), Some(low)) = (last, low) else {
                    break 'walk;
                };
                if self.watch().lock().hash_at(h.height) == Some(h.id) {
                    fork = Some(h.height);
                    break 'walk;
                }
                if h.height < low {
                    break 'walk;
                }
                // A long gap: skip down to the window, which is all that can hold the fork.
                if new.is_empty() && h.height > last_height + MAX_GAP as u32 {
                    top = Some(last_height);
                    continue 'walk;
                }
                new.push(h);
            }
            top = new.last().and_then(|h| h.height.checked_sub(1));
            if top.is_none() {
                break;
            }
        }
        let Some(first) = first else {
            return Err("malformed");
        };
        let Some((last_height, _)) = last else {
            // First look: start from here. Channels verified below it are checked again.
            self.reset(&first, None);
            return Ok(());
        };
        // Undo what the blocks above the fork did, newest first.
        let mut disconnected = 0;
        loop {
            let above = self
                .watch()
                .lock()
                .seen
                .back()
                .filter(|b| fork.is_none_or(|f| b.height > f))
                .map(|b| (b.height, b.hash));
            if above.is_none() {
                break;
            }
            self.disconnect();
            disconnected += 1;
        }
        if disconnected > 0 {
            let mut s = self.watch().lock();
            s.counts.reorgs += 1;
            // Channels funded above the fork: is their output still where it was?
            let f = fork.unwrap_or(0);
            let again: Vec<u64> = s
                .funding
                .iter()
                .filter(|(_, x)| x.block.height > f)
                .map(|(scid, _)| *scid)
                .collect();
            s.recheck.extend(again);
            drop(s);
            let _ = self.gossip.events.send(Event::Reorg {
                fork: f,
                disconnected,
            });
        }
        if fork == Some(first.height) {
            // The node went back to the fork: nothing new to read.
            return Ok(());
        }
        let joined = new
            .last()
            .is_some_and(|h| fork.is_some_and(|f| h.height == f + 1));
        if !joined || new.len() > MAX_GAP {
            self.reset(&first, Some(last_height));
            return Ok(());
        }
        for h in new.into_iter().rev() {
            self.connect(h).await?;
        }
        Ok(())
    }

    /// Starts following from `tip` without reading the blocks before it. Watched channels that
    /// may have closed in those blocks go to the catch-up: all of them after a gap (`from` the
    /// newest block read), at the first look those verified at or below `tip` (the tip they were
    /// verified at may be another branch's).
    fn reset(&self, tip: &Header, from: Option<u32>) {
        let gap = from.is_some();
        let mut s = self.watch().lock();
        s.seen.clear();
        s.reading = None;
        s.seen.push_back(Seen {
            height: tip.height,
            hash: tip.id,
            closes: Vec::new(),
            penalties: Vec::new(),
        });
        let again: Vec<u64> = s
            .funding
            .iter()
            .filter(|(_, f)| gap || f.verified_at <= tip.height)
            .map(|(scid, _)| *scid)
            .collect();
        let rechecked = again.len();
        if gap {
            s.recheck.clear();
        }
        s.recheck.extend(again);
        s.at_tip = true;
        drop(s);
        if let Some(from) = from {
            let _ = self.gossip.events.send(Event::CloseWatchReset {
                from,
                to: tip.height,
                rechecked,
            });
        }
    }

    /// Reads one block on top of the window and applies the closes and penalties it holds.
    async fn connect(&self, h: Header) -> Result<(), &'static str> {
        let block = self.ask(self.chain.block(h.id)).await?;
        let last = self.watch().lock().seen.back().map(|b| b.hash);
        if block.block_hash() != h.id || Some(block.header.prev_blockhash) != last {
            return Err("chain_moved");
        }
        let at = BlockRef {
            height: h.height,
            hash: h.id,
            time: u64::from(block.header.time),
        };
        // Inputs that may spend a watched funding output.
        let mut candidates = Vec::new();
        {
            let mut s = self.watch().lock();
            for (i, tx) in block.txdata.iter().enumerate().skip(1) {
                for (n, input) in tx.input.iter().enumerate() {
                    let Some(p) = funding_program(input) else {
                        continue;
                    };
                    if s.programs.range((p, 0)..=(p, u64::MAX)).next().is_some() {
                        candidates.push((i, n, input.previous_output, p));
                    }
                }
            }
            // Channels verified from now on at a lower tip were not matched above.
            s.reading = Some((at.height, at.hash));
        }
        // Each candidate's spent output must be a channel's own: the channel id its position
        // makes, watched, with this script.
        let mut closes = Vec::new();
        for (i, n, prev, p) in candidates {
            // Unknown to the node: not a channel's, whose output it found.
            let Some((height, index)) = self.ask_found(self.chain.tx_position(prev.txid)).await?
            else {
                continue;
            };
            let Some(scid) = scid_of(height, index, prev.vout) else {
                continue;
            };
            let own = self
                .watch()
                .lock()
                .funding
                .get(&scid)
                .is_some_and(|f| f.program == p);
            if !own {
                continue;
            }
            let Some(tx) = block.txdata.get(i) else {
                continue;
            };
            let fee = self.fee(tx, scid).await?;
            closes.push((scid, i, n, fee));
        }
        self.watch().lock().counts.blocks += 1;
        let mut undo = Vec::new();
        for (scid, i, n, fee) in closes {
            if let Some(tx) = block.txdata.get(i)
                && let Some(u) = self.close(scid, None, tx, n, at, fee)
            {
                undo.push(u);
            }
        }
        // After the closes: a penalty can sweep a commitment in the block that confirms it.
        let mut s = self.watch().lock();
        let mut penalties = Vec::new();
        for tx in block.txdata.iter().skip(1) {
            for input in &tx.input {
                if let Some(scid) = s.swept.get(&input.previous_output)
                    && revoked(input)
                {
                    penalties.push((*scid, tx.compute_txid()));
                }
            }
        }
        let mut done = Vec::new();
        for (scid, txid) in penalties {
            let Some(sweeps) = s.drop_sweeps(scid) else {
                continue;
            };
            s.count(CloseKind::Force, -1);
            s.count(CloseKind::Penalty, 1);
            self.watch().record(
                &mut s,
                History::Penalty {
                    scid,
                    txid,
                    block: at,
                },
            );
            done.push((scid, sweeps));
        }
        s.reading = None;
        s.seen.push_back(Seen {
            height: at.height,
            hash: at.hash,
            closes: undo,
            penalties: done,
        });
        while s.seen.len() > WINDOW {
            s.seen.pop_front();
        }
        let expired: Vec<u64> = s
            .sweeps
            .iter()
            .filter(|(_, w)| w.until < at.height)
            .map(|(k, _)| *k)
            .collect();
        for scid in expired {
            s.drop_sweeps(scid);
        }
        Ok(())
    }

    /// The closing transaction's fee: its outputs against the funding output when that is its
    /// only input, otherwise the node's word.
    async fn fee(&self, tx: &Transaction, scid: u64) -> Result<Option<u64>, &'static str> {
        if tx.input.len() == 1 {
            let capacity = self.watch().lock().funding.get(&scid).map(|f| f.capacity);
            let out: u64 = tx.output.iter().map(|o| o.value.to_sat()).sum();
            return Ok(capacity.and_then(|c| c.checked_sub(out)));
        }
        self.ask(self.chain.tx_fee(tx.compute_txid())).await
    }

    /// Records a close, once: the channel leaves the watched set and the graph, and a later
    /// announcement of it is refused. `found` is a channel not watched (found spent by the
    /// replay). Returns what undoes it.
    fn close(
        &self,
        scid: u64,
        found: Option<Funding>,
        tx: &Transaction,
        input: usize,
        at: BlockRef,
        fee: Option<u64>,
    ) -> Option<Undo> {
        let in_graph = found.is_none();
        let funding = match found {
            Some(f) => f,
            None => self.watch().lock().unwatch(scid)?,
        };
        let graph = self.gossip.graph();
        let snapshot = if in_graph {
            let ro = graph.read_only();
            ro.channel(scid).map(|c| Snapshot {
                features: c.features.clone(),
                updates: [&c.one_to_two, &c.two_to_one]
                    .into_iter()
                    .flatten()
                    .filter_map(|u| u.last_update_message.clone())
                    .collect(),
                nodes: [c.node_one, c.node_two]
                    .iter()
                    .filter_map(|n| {
                        let info = ro.node(n)?.announcement_info.as_ref()?;
                        Some(UnsignedNodeAnnouncement {
                            features: info.features().clone(),
                            timestamp: info.last_update(),
                            node_id: *n,
                            rgb: info.rgb(),
                            alias: *info.alias(),
                            addresses: info.addresses().to_vec(),
                            excess_address_data: Vec::new(),
                            excess_data: Vec::new(),
                        })
                    })
                    .collect(),
            })
        } else {
            None
        };
        // Spent in the cache before it leaves the graph, so no announcement can slip back in.
        self.gossip.lookup.closed(scid);
        if snapshot.is_some() {
            graph.channel_failed_permanent(scid);
            self.gossip.changed(Change::Channel(scid));
            self.gossip.changed(Change::Node(funding.nodes.0));
            self.gossip.changed(Change::Node(funding.nodes.1));
        }
        let txid = tx.compute_txid();
        let kind = classify(tx, input);
        let mut s = self.watch().lock();
        if kind == CloseKind::Force {
            let outs = tx
                .output
                .iter()
                .enumerate()
                .filter(|(_, o)| o.script_pubkey.is_p2wsh() && o.value.to_sat() != ANCHOR_SATS)
                .filter_map(|(v, _)| Some(OutPoint::new(txid, u32::try_from(v).ok()?)))
                .collect();
            let until = at.height.saturating_add(SWEEP_BLOCKS);
            s.add_sweeps(scid, Sweeps { until, outs });
        }
        s.count(kind, 1);
        let closed = Closed {
            scid,
            node_one: funding.nodes.0,
            node_two: funding.nodes.1,
            capacity_sats: funding.capacity,
            opened: funding.block,
            closed: at,
            txid,
            kind,
            fee_sats: fee,
        };
        self.watch().record(&mut s, History::Closed(closed));
        Some(Undo {
            scid,
            funding,
            snapshot,
            kind,
        })
    }

    /// Undoes the newest block of the window, which left the best chain: its penalties and its
    /// closes. Opens it funded are found by the recheck that follows.
    fn disconnect(&self) {
        let Some(b) = self.watch().lock().seen.pop_back() else {
            return;
        };
        {
            let mut s = self.watch().lock();
            for (scid, sweeps) in b.penalties.into_iter().rev() {
                s.add_sweeps(scid, sweeps);
                s.count(CloseKind::Penalty, -1);
                s.count(CloseKind::Force, 1);
                s.counts.undone += 1;
                self.watch().record(&mut s, History::PenaltyUndone { scid });
            }
        }
        for u in b.closes.into_iter().rev() {
            self.unclose(u);
        }
    }

    /// Reopens a channel whose close left the best chain: watched again, and back in the graph if
    /// it was there. The next block that spends it closes it again.
    fn unclose(&self, u: Undo) {
        self.gossip.lookup.forget(u.scid);
        let (one, two) = u.funding.nodes;
        if let Some(snap) = &u.snapshot {
            let graph = self.gossip.graph();
            // Not through an announcement: LDK refuses one for a channel it removed this week.
            let _ = graph.add_channel_from_partial_announcement(
                u.scid,
                Some(u.funding.capacity),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs()),
                snap.features.clone(),
                one,
                two,
            );
            for update in &snap.updates {
                let _ = graph.update_channel(update);
            }
            for n in &snap.nodes {
                let bare = graph
                    .read_only()
                    .node(&n.node_id)
                    .is_some_and(|i| i.announcement_info.is_none());
                if bare {
                    let _ = graph.update_node_from_unsigned_announcement(n);
                }
            }
            self.gossip.changed(Change::Channel(u.scid));
            self.gossip.changed(Change::Node(one));
            self.gossip.changed(Change::Node(two));
        }
        let mut s = self.watch().lock();
        s.drop_sweeps(u.scid);
        // Watched again even if it was not in the graph (found closed by the catch-up), so its
        // next close is recorded; gossip puts it back in the graph.
        s.watch(u.scid, u.funding);
        s.count(u.kind, -1);
        s.counts.undone += 1;
        self.watch()
            .record(&mut s, History::CloseUndone { scid: u.scid });
    }

    /// One step of the catch-up: a batch of watched outputs checked for a spend, or one spent
    /// output's spend found.
    async fn check(&self) -> Result<(), &'static str> {
        // Items leave the queues once answered, so the status counts them until then. Only this
        // task takes from the front.
        let batch: Vec<u64> = {
            let s = self.watch().lock();
            s.recheck.iter().take(LOOKUP_BATCH).copied().collect()
        };
        if !batch.is_empty() {
            let positions = OutPositions::new(batch.iter().map(|s| position(*s)).collect())
                .ok_or("too many positions")?;
            let r = self.ask(self.chain.outputs(positions)).await?.value;
            if r.len() != batch.len() {
                return Err("malformed");
            }
            {
                let mut s = self.watch().lock();
                for scid in &batch {
                    s.recheck.remove(scid);
                }
            }
            for (scid, slot) in batch.into_iter().zip(r) {
                self.rechecked(scid, slot);
            }
            return Ok(());
        }
        let Some(item) = self.watch().lock().find.front().cloned() else {
            return Ok(());
        };
        self.find(&item).await?;
        self.watch().lock().find.pop_front();
        Ok(())
    }

    /// What the node says now of a watched output: still at its position in the same block, in
    /// another block at the same position after a reorg, gone, or spent.
    fn rechecked(&self, scid: u64, slot: Option<node::esplora::model::PositionedOutput>) {
        let mut s = self.watch().lock();
        let Some(f) = s.funding.get(&scid).cloned() else {
            return;
        };
        let height = position(scid).height;
        let here = slot.filter(|o| {
            o.status.block_height == Some(height)
                && ScriptBuf::from_hex(&o.out.scriptpubkey)
                    .ok()
                    .and_then(|x| program_of(&x))
                    == Some(f.program)
        });
        let Some(o) = here else {
            // Its funding transaction left the chain, or sits elsewhere: the channel is gone.
            s.unwatch(scid);
            s.counts.undone += 1;
            self.watch().record(&mut s, History::OpenUndone { scid });
            drop(s);
            self.gossip.lookup.closed(scid);
            self.gossip.graph().channel_failed_permanent(scid);
            self.gossip.changed(Change::Channel(scid));
            self.gossip.changed(Change::Node(f.nodes.0));
            self.gossip.changed(Change::Node(f.nodes.1));
            return;
        };
        if let Some(hash) = o.status.block_hash
            && hash != f.block.hash
        {
            // Mined again at the same position in another block.
            let block = BlockRef {
                height,
                hash,
                time: o.status.block_time.unwrap_or(f.block.time),
            };
            if let Some(w) = s.funding.get_mut(&scid) {
                w.block = block;
            }
            s.counts.undone += 1;
            self.watch().record(&mut s, History::OpenUndone { scid });
            let open = Opened {
                scid,
                node_one: f.nodes.0,
                node_two: f.nodes.1,
                capacity_sats: f.capacity,
                block,
            };
            self.watch().record(&mut s, History::Opened(open));
        }
        if o.spent {
            s.find.push_back(Find {
                scid,
                funding: f,
                in_graph: true,
            });
        }
    }

    /// Finds the spend of a spent funding output through the node's spend records: the funding
    /// txid from its position, its spender, and the spending transaction. Four or five requests.
    async fn find(&self, item: &Find) -> Result<(), &'static str> {
        // What the node no longer knows (a reorg since) is dropped, not retried.
        let p = position(item.scid);
        let Some(hash) = self.ask_found(self.chain.block_hash(p.height)).await? else {
            return Ok(());
        };
        let Some(txid) = self.ask_found(self.chain.block_txid(hash, p.index)).await? else {
            return Ok(());
        };
        let Some(spend) = self.ask_found(self.chain.outspend(txid, p.vout)).await? else {
            return Ok(());
        };
        let (Some(spender), Some(status)) = (spend.txid, spend.status) else {
            return Ok(());
        };
        let (Some(height), Some(hash), Some(time)) =
            (status.block_height, status.block_hash, status.block_time)
        else {
            // In the mempool: the block that confirms it closes the channel.
            return Ok(());
        };
        {
            // A block the watcher has not read on its branch (not yet, or the node switched
            // branches since): the channel is watched, and reading the block closes it, with what
            // undoes it. Below the window the block cannot be compared, and is taken as it is.
            let mut s = self.watch().lock();
            let (low, last) = (
                s.seen.front().map_or(0, |b| b.height),
                s.seen.back().map_or(0, |b| b.height),
            );
            let other_branch = height <= last && height >= low && s.hash_at(height) != Some(hash);
            if height > last || other_branch {
                if !item.in_graph && !s.funding.contains_key(&item.scid) {
                    s.watch(item.scid, item.funding.clone());
                }
                // If the watcher's branch wins, its block at that height was read without the
                // channel: check it again once the watcher has followed the node.
                if other_branch {
                    s.recheck.insert(item.scid);
                }
                return Ok(());
            }
        }
        let Some(tx) = self.ask_found(self.chain.tx(spender)).await? else {
            return Ok(());
        };
        let Some(input) = tx
            .input
            .iter()
            .position(|i| i.previous_output == OutPoint::new(txid, p.vout))
        else {
            return Err("malformed");
        };
        let fee = if tx.input.len() == 1 {
            let out: u64 = tx.output.iter().map(|o| o.value.to_sat()).sum();
            item.funding.capacity.checked_sub(out)
        } else {
            self.ask(self.chain.tx_fee(spender)).await?
        };
        let at = BlockRef { height, hash, time };
        let found = (!item.in_graph).then(|| item.funding.clone());
        if let Some(u) = self.close(item.scid, found, &tx, input, at, fee) {
            let mut s = self.watch().lock();
            if let Some(b) = s.seen.iter_mut().find(|b| b.hash == hash) {
                b.closes.push(u);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_channel_verified_below_the_block_being_read_is_checked_again() {
        let w = Watch::new(10, HashMap::new());
        w.lock().seen.push_back(Seen {
            height: 100,
            hash: BlockHash::all_zeros(),
            closes: Vec::new(),
            penalties: Vec::new(),
        });
        // Block 101's inputs were matched; a lookup answered at tip 100 registers a channel.
        w.lock().reading = Some((101, BlockHash::all_zeros()));
        let n = NodeId::from_slice(&[2; 33]).unwrap();
        let f = |verified_at| Funding {
            nodes: (n, n),
            capacity: 1,
            program: [1; 32],
            block: BlockRef {
                height: 1,
                hash: BlockHash::all_zeros(),
                time: 0,
            },
            verified_at,
        };
        let zero = BlockHash::all_zeros();
        w.opened(1, f(100), zero);
        w.opened(2, f(101), zero);
        // Answered at the height being read, but on another branch than the block read there.
        w.opened(4, f(101), BlockHash::from_byte_array([1; 32]));
        assert_eq!(w.lock().recheck.iter().copied().collect::<Vec<_>>(), [1, 4]);
    }

    #[test]
    fn scid_of_is_the_inverse_of_position() {
        let id = scid_of(975_000, 1234, 5).unwrap();
        let p = position(id);
        assert_eq!((p.height, p.index, p.vout), (975_000, 1234, 5));
        assert_eq!(scid_of(1 << 24, 0, 0), None);
        assert_eq!(scid_of(1, 0, 1 << 16), None);
    }
}
