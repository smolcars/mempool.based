//! Funding-output checks for channel announcements (`docs/LIGHTNING.md`, Validation).
//!
//! LDK asks [`Lookup`] about each channel announcement it has not validated, while it handles the
//! message. A result already known (cached) is answered at once. Anything else is answered later:
//! LDK keeps the announcement, and the channel's later updates and node announcements, waiting on
//! a [`UtxoFuture`]; the lookup joins a queue; and one task, [`run`], sends the queue to the node in
//! batches of up to [`LOOKUP_BATCH`] positions through the gate every Lightning request shares
//! ([`Pace`]), then resolves the waiting announcements. While the queue holds
//! `Config::max_pending` announcements, `processing_queue_high` makes LDK hold the next gossip
//! message of each peer and stop reading from it, so a burst is paced, not dropped.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use lightning::bitcoin::constants::ChainHash;
use lightning::bitcoin::hashes::Hash;
use lightning::bitcoin::{
    Amount, Block, BlockHash, ScriptBuf, Transaction, TxOut, Txid, WScriptHash,
};
use lightning::routing::utxo::{UtxoFuture, UtxoLookup, UtxoLookupError, UtxoResult};
use node::esplora::model::{Block as Header, Outspend, PositionedOutput};
use node::esplora::{Error, Esplora, OutPosition, OutPositions, Stamped};
use tokio::sync::Notify;

use crate::closes::{BlockRef, program_of};
use crate::gossip::Gossip;
use crate::pace::Pace;
use crate::store::Verified;
use crate::{Event, Rejected};

/// BOLT 7: a channel is announced once its funding output is six blocks deep.
pub(crate) const CONFIRMATIONS: u32 = 6;
/// A missing output in a block at most this far past the node's tip is early, not invented: the
/// node may be behind the peer that announced it.
const EARLY_AHEAD: u32 = 6;
/// Lookups past `max_pending` still taken: LDK reads at most one more gossip message per peer
/// once the queue is high.
const PENDING_SLACK: usize = 64;
/// A first lookup waits this long so a burst of announcements goes out as one batch.
const LINGER: Duration = Duration::from_millis(50);
/// Positions per request (the node takes up to 300): a third of that keeps each request short,
/// so the node's other callers never wait long behind one of ours.
pub(crate) const LOOKUP_BATCH: usize = 100;
/// Retry delays after a failed request: doubling from the first to the second.
const RETRY_MIN: Duration = Duration::from_secs(1);
const RETRY_MAX: Duration = Duration::from_secs(60);
/// A request with no answer by then counts as failed. The client's own timeout is shorter.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// While only early announcements wait, the node's tip is read this often. Tests wait less.
const TIP_EVERY: Duration = Duration::from_secs(if cfg!(test) { 1 } else { 60 });

/// Where funding outputs are looked up (rbitcoin's output-by-position routes), and where the
/// close-watcher reads blocks and spends. Implemented for [`Esplora`]; tests use a fake. Clones
/// share one client.
pub trait Chain: Clone + Send + Sync + 'static {
    /// One slot per position, in order, `None` where no output exists; stamped with the tip.
    fn outputs(
        &self,
        positions: OutPositions,
    ) -> impl Future<Output = Result<Stamped<Vec<Option<PositionedOutput>>>, Error>> + Send;
    fn tip_height(&self) -> impl Future<Output = Result<u32, Error>> + Send;
    fn tip_hash(&self) -> impl Future<Output = Result<BlockHash, Error>> + Send;
    /// Up to ten headers, newest first, from height `top` (or the tip) down.
    fn headers(&self, top: Option<u32>) -> impl Future<Output = Result<Vec<Header>, Error>> + Send;
    fn block(&self, hash: BlockHash) -> impl Future<Output = Result<Block, Error>> + Send;
    fn block_hash(&self, height: u32) -> impl Future<Output = Result<BlockHash, Error>> + Send;
    /// The txid at `index` in the block.
    fn block_txid(
        &self,
        hash: BlockHash,
        index: u32,
    ) -> impl Future<Output = Result<Txid, Error>> + Send;
    /// A confirmed transaction's block height and index in the block.
    fn tx_position(&self, txid: Txid) -> impl Future<Output = Result<(u32, u32), Error>> + Send;
    /// Who spent an output, and in which block.
    fn outspend(
        &self,
        txid: Txid,
        vout: u32,
    ) -> impl Future<Output = Result<Outspend, Error>> + Send;
    fn tx(&self, txid: Txid) -> impl Future<Output = Result<Transaction, Error>> + Send;
    /// A transaction's fee, if the node knows its inputs' values.
    fn tx_fee(&self, txid: Txid) -> impl Future<Output = Result<Option<u64>, Error>> + Send;
}

impl Chain for Esplora {
    async fn outputs(
        &self,
        positions: OutPositions,
    ) -> Result<Stamped<Vec<Option<PositionedOutput>>>, Error> {
        self.outputs_at(positions, None).await
    }

    async fn tip_height(&self) -> Result<u32, Error> {
        Ok(Esplora::tip_height(self).await?.value)
    }

    async fn tip_hash(&self) -> Result<BlockHash, Error> {
        Ok(Esplora::tip_hash(self).await?.value)
    }

    async fn headers(&self, top: Option<u32>) -> Result<Vec<Header>, Error> {
        Ok(self.blocks(top).await?.value)
    }

    async fn block(&self, hash: BlockHash) -> Result<Block, Error> {
        Ok(self.block_raw(hash).await?.value)
    }

    async fn block_hash(&self, height: u32) -> Result<BlockHash, Error> {
        Ok(Esplora::block_hash(self, height).await?.value)
    }

    async fn block_txid(&self, hash: BlockHash, index: u32) -> Result<Txid, Error> {
        Ok(Esplora::block_txid(self, hash, index).await?.value)
    }

    async fn tx_position(&self, txid: Txid) -> Result<(u32, u32), Error> {
        let p = self.tx_merkle_proof(txid).await?.value;
        Ok((p.block_height, p.pos))
    }

    async fn outspend(&self, txid: Txid, vout: u32) -> Result<Outspend, Error> {
        Ok(self.tx_outspend(txid, vout, None).await?.value)
    }

    async fn tx(&self, txid: Txid) -> Result<Transaction, Error> {
        Ok(self.tx_raw(txid).await?.value)
    }

    async fn tx_fee(&self, txid: Txid) -> Result<Option<u64>, Error> {
        Ok(Esplora::tx(self, txid).await?.value.fee)
    }
}

/// A short channel id's funding position: block height, transaction index, output index.
pub(crate) fn position(scid: u64) -> OutPosition {
    OutPosition {
        height: (scid >> 40) as u32,
        index: ((scid >> 16) & 0x00ff_ffff) as u32,
        vout: (scid & 0xffff) as u32,
    }
}

/// Where a funding output is and when it was checked: its block, and the tip of the answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Funded {
    pub(crate) block: BlockRef,
    pub(crate) tip: u32,
    pub(crate) tip_hash: BlockHash,
}

/// What the node said about a funding position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Unspent and deep enough.
    Found(TxOut, Funded),
    /// No such output: an invented channel, or one at the wrong position.
    Missing,
    /// The output exists but was spent: a closed channel still being gossiped.
    Spent,
    /// Not yet `CONFIRMATIONS` deep, or in a block just past the node's tip. Checked again once
    /// the tip reaches `ready_at`.
    Early { ready_at: u32 },
}

pub(crate) fn classify(
    scid: u64,
    (tip, tip_hash): (u32, BlockHash),
    slot: Option<PositionedOutput>,
) -> Outcome {
    let p = position(scid);
    let ready_at = p.height.saturating_add(CONFIRMATIONS - 1);
    match slot {
        None if p.height > tip && p.height <= tip.saturating_add(EARLY_AHEAD) => {
            Outcome::Early { ready_at }
        }
        None => Outcome::Missing,
        Some(o) if o.status.block_height != Some(p.height) => Outcome::Missing,
        Some(o) if o.spent => Outcome::Spent,
        Some(_) if tip < ready_at => Outcome::Early { ready_at },
        Some(o) => match ScriptBuf::from_hex(&o.out.scriptpubkey) {
            Ok(script_pubkey) => Outcome::Found(
                TxOut {
                    value: Amount::from_sat(o.out.value),
                    script_pubkey,
                },
                Funded {
                    block: block_of(p.height, &o),
                    tip,
                    tip_hash,
                },
            ),
            Err(_) => Outcome::Missing,
        },
    }
}

/// The block of a found output. rbitcoin always gives a confirmed output's block hash and time.
fn block_of(height: u32, o: &PositionedOutput) -> BlockRef {
    BlockRef {
        height,
        hash: o.status.block_hash.unwrap_or(BlockHash::all_zeros()),
        time: o.status.block_time.unwrap_or(0),
    }
}

/// A spent funding output's value, program and block, for the catch-up.
pub(crate) fn spent_output(
    scid: u64,
    slot: Option<&PositionedOutput>,
) -> Option<(u64, [u8; 32], BlockRef)> {
    let o = slot.filter(|o| o.spent && o.status.block_height == Some(position(scid).height))?;
    let program = program_of(&ScriptBuf::from_hex(&o.out.scriptpubkey).ok()?)?;
    Some((o.out.value, program, block_of(position(scid).height, o)))
}

/// Lookup results by short channel id, oldest evicted first, each kept for a while.
struct Cache {
    cap: usize,
    ttl: Duration,
    map: HashMap<u64, (Outcome, Instant)>,
    order: VecDeque<(u64, Instant)>,
}

impl Cache {
    fn get(&mut self, scid: u64) -> Option<Outcome> {
        match self.map.get(&scid) {
            Some((o, at)) if at.elapsed() < self.ttl => Some(o.clone()),
            Some(_) => {
                self.map.remove(&scid);
                None
            }
            None => None,
        }
    }

    fn put(&mut self, scid: u64, outcome: Outcome) {
        if self.cap == 0 {
            return;
        }
        let now = Instant::now();
        self.map.insert(scid, (outcome, now));
        self.order.push_back((scid, now));
        // `order` holds every live entry, so bounding it bounds the map.
        while self.order.len() > self.cap {
            if let Some((id, at)) = self.order.pop_front()
                && self.map.get(&id).is_some_and(|(_, t)| *t == at)
            {
                self.map.remove(&id);
            }
        }
    }
}

/// An announcement waiting in LDK, and the script its keys require.
struct Waiter {
    future: UtxoFuture,
    script: ScriptBuf,
}

struct State {
    /// Channel ids awaiting a request, oldest first.
    order: VecDeque<u64>,
    /// Announcements queued or in a request, by channel id. Identical lookups share an entry.
    waiting: HashMap<u64, Vec<Waiter>>,
    waiting_len: usize,
    /// Announcements too shallow for now, with the tip height that makes them deep enough.
    early: HashMap<u64, (u32, Vec<Waiter>)>,
    early_len: usize,
    /// Announcements answered and being handed to LDK, still counted until LDK has added them, so
    /// the ceiling never sees a free slot they are about to take.
    resolving: usize,
    cache: Cache,
    /// The announcement LDK is about to look up: its channel id and the script its keys require.
    expect: Option<(u64, ScriptBuf)>,
    /// What the cache answered for it, if it was found.
    found: Option<(u64, TxOut, Funded)>,
    tip: Option<u32>,
    rejected: Rejected,
    failures: u32,
    requests: u64,
    outputs: u64,
}

/// What [`Lookup::snapshot`] reports.
pub(crate) struct Snapshot {
    pub(crate) pending: usize,
    pub(crate) early: usize,
    pub(crate) rejected: Rejected,
    pub(crate) failures: u32,
    pub(crate) requests: u64,
    pub(crate) outputs: u64,
}

/// LDK's `UtxoLookup`, and the queue it shares with [`run`].
pub(crate) struct Lookup {
    chain: ChainHash,
    max_pending: usize,
    max_early: usize,
    state: Mutex<State>,
    /// New work for [`run`].
    wake: Notify,
    /// Lookups were resolved: the peer manager may read gossip again.
    pub(crate) resolved: Notify,
    /// Tests of other parts answer every lookup at once, as if each funding output existed.
    #[cfg(test)]
    pub(crate) trust: std::sync::atomic::AtomicBool,
}

/// The value tests' trusted lookups give every channel.
#[cfg(test)]
pub(crate) const TRUSTED_SATS: u64 = 10_000_000;

/// How a resolved lookup ended, per waiting announcement.
type Settled = Vec<(UtxoFuture, Result<TxOut, UtxoLookupError>)>;

impl Lookup {
    pub(crate) fn new(config: &crate::Config) -> Self {
        Self {
            chain: ChainHash::using_genesis_block(config.network),
            max_pending: config.max_pending,
            max_early: config.max_early,
            state: Mutex::new(State {
                order: VecDeque::new(),
                waiting: HashMap::new(),
                waiting_len: 0,
                early: HashMap::new(),
                early_len: 0,
                resolving: 0,
                cache: Cache {
                    cap: config.cache_entries,
                    ttl: config.cache_ttl,
                    map: HashMap::new(),
                    order: VecDeque::new(),
                },
                expect: None,
                found: None,
                tip: None,
                rejected: Rejected::default(),
                failures: 0,
                requests: 0,
                outputs: 0,
            }),
            wake: Notify::new(),
            resolved: Notify::new(),
            #[cfg(test)]
            trust: std::sync::atomic::AtomicBool::new(false),
        }
    }

    #[cfg(test)]
    pub(crate) fn cache_len(&self) -> usize {
        self.lock().cache.map.len()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Called just before LDK handles an announcement: the script its bitcoin keys require.
    pub(crate) fn expect(&self, scid: u64, script: Option<ScriptBuf>) {
        self.lock().expect = script.map(|s| (scid, s));
    }

    /// The output an announcement just answered from the cache was found at, if it was.
    pub(crate) fn take_found(&self, scid: u64) -> Option<(TxOut, Funded)> {
        match self.lock().found.take() {
            Some((id, out, f)) if id == scid => Some((out, f)),
            _ => None,
        }
    }

    /// The channel closed: any announcement of it is refused from now on, from the cache and
    /// then by the node.
    pub(crate) fn closed(&self, scid: u64) {
        self.lock().cache.put(scid, Outcome::Spent);
    }

    /// The channel's close left the best chain: what the cache says of it no longer holds.
    pub(crate) fn forget(&self, scid: u64) {
        self.lock().cache.map.remove(&scid);
    }

    /// Answers lookups from a saved file: each channel found, unspent up to the saved block.
    pub(crate) fn restore(&self, v: &Verified) {
        let mut s = self.lock();
        for c in &v.channels {
            let script = ScriptBuf::new_p2wsh(&WScriptHash::from_byte_array(c.program));
            let funded = Funded {
                block: c.block,
                tip: v.height,
                tip_hash: v.hash,
            };
            let out = TxOut {
                value: Amount::from_sat(c.capacity),
                script_pubkey: script,
            };
            s.cache.put(c.scid, Outcome::Found(out, funded));
        }
    }

    /// Forgets the answers [`Self::restore`] gave, by the saved block they were stamped with.
    pub(crate) fn unrestore(&self, saved: BlockHash) {
        self.lock()
            .cache
            .map
            .retain(|_, (o, _)| !matches!(o, Outcome::Found(_, f) if f.tip_hash == saved));
    }

    pub(crate) fn count(&self, f: impl FnOnce(&mut Rejected)) {
        f(&mut self.lock().rejected);
    }

    /// Announcements waiting for a lookup, being resolved, or held as early.
    pub(crate) fn held(&self) -> usize {
        let s = self.lock();
        s.waiting_len + s.resolving + s.early_len
    }

    /// Whether a lookup for this channel is waiting or early.
    pub(crate) fn holds(&self, scid: u64) -> bool {
        let s = self.lock();
        s.waiting.contains_key(&scid) || s.early.contains_key(&scid)
    }

    /// Announcements waiting for a lookup or being resolved.
    pub(crate) fn waiting(&self) -> usize {
        let s = self.lock();
        s.waiting_len + s.resolving
    }

    pub(crate) fn queue_high(&self) -> bool {
        self.lock().waiting_len >= self.max_pending
    }

    pub(crate) fn snapshot(&self) -> Snapshot {
        let s = self.lock();
        Snapshot {
            pending: s.waiting_len + s.resolving,
            early: s.early_len,
            rejected: s.rejected,
            failures: s.failures,
            requests: s.requests,
            outputs: s.outputs,
        }
    }

    /// Moves early announcements the tip has made deep enough back to the front of the queue.
    fn promote(s: &mut State, tip: u32) {
        let ready: Vec<u64> = s
            .early
            .iter()
            .filter(|(_, (at, _))| *at <= tip)
            .map(|(scid, _)| *scid)
            .collect();
        for scid in ready {
            if let Some((_, w)) = s.early.remove(&scid) {
                s.early_len -= w.len();
                s.waiting_len += w.len();
                s.waiting.entry(scid).or_default().extend(w);
                s.order.push_front(scid);
            }
        }
    }

    /// Records the node's answer for one channel and says how each waiting announcement ends.
    /// Announcements for an early output move to the early set instead, while it has room.
    fn settle(
        &self,
        s: &mut State,
        scid: u64,
        outcome: Outcome,
        out: &mut Settled,
        verified: &mut Vec<(u64, TxOut, Funded)>,
    ) {
        let waiters = s.waiting.remove(&scid).unwrap_or_default();
        s.waiting_len -= waiters.len();
        let unknown = || Err(UtxoLookupError::UnknownTx);
        match outcome {
            Outcome::Found(txout, funded) => {
                if waiters.iter().any(|w| w.script == txout.script_pubkey) {
                    verified.push((scid, txout.clone(), funded));
                }
                for w in waiters {
                    if w.script == txout.script_pubkey {
                        out.push((w.future, Ok(txout.clone())));
                    } else {
                        s.rejected.wrong_script += 1;
                        out.push((w.future, unknown()));
                    }
                }
                s.cache.put(scid, Outcome::Found(txout, funded));
            }
            Outcome::Early { ready_at } => {
                if s.early_len + waiters.len() <= self.max_early {
                    s.early_len += waiters.len();
                    s.early
                        .entry(scid)
                        .or_insert((ready_at, Vec::new()))
                        .1
                        .extend(waiters);
                } else {
                    s.rejected.shallow += 1;
                    out.extend(waiters.into_iter().map(|w| (w.future, unknown())));
                }
            }
            o @ (Outcome::Missing | Outcome::Spent) => {
                if o == Outcome::Spent {
                    s.rejected.spent += 1;
                } else {
                    s.rejected.missing += 1;
                }
                out.extend(waiters.into_iter().map(|w| (w.future, unknown())));
                s.cache.put(scid, o);
            }
        }
    }
}

impl UtxoLookup for Lookup {
    fn get_utxo(&self, chain_hash: &ChainHash, scid: u64) -> UtxoResult {
        let unknown = UtxoResult::Sync(Err(UtxoLookupError::UnknownTx));
        let mut s = self.lock();
        let script = match s.expect.take() {
            Some((id, script)) if id == scid => script,
            // Not asked through our handler: nothing to check the script against.
            _ => return unknown,
        };
        if *chain_hash != self.chain {
            return UtxoResult::Sync(Err(UtxoLookupError::UnknownChain));
        }
        match s.cache.get(scid) {
            Some(Outcome::Found(txout, funded)) if txout.script_pubkey == script => {
                s.found = Some((scid, txout.clone(), funded));
                return UtxoResult::Sync(Ok(txout));
            }
            Some(Outcome::Found(..)) => {
                s.rejected.wrong_script += 1;
                return unknown;
            }
            Some(_) => return unknown,
            None => {}
        }
        #[cfg(test)]
        if self.trust.load(std::sync::atomic::Ordering::Relaxed) {
            return UtxoResult::Sync(Ok(TxOut {
                value: Amount::from_sat(TRUSTED_SATS),
                script_pubkey: script,
            }));
        }
        let future = UtxoFuture::new();
        let waiter = Waiter {
            future: future.clone(),
            script,
        };
        if s.early.contains_key(&scid) {
            if s.early_len >= self.max_early {
                s.rejected.shallow += 1;
                return unknown;
            }
            s.early_len += 1;
            if let Some((_, w)) = s.early.get_mut(&scid) {
                w.push(waiter);
            }
            return UtxoResult::Async(future);
        }
        if s.waiting_len >= self.max_pending + PENDING_SLACK {
            s.rejected.queue_full += 1;
            return unknown;
        }
        s.waiting_len += 1;
        let entry = s.waiting.entry(scid).or_default();
        let new = entry.is_empty();
        entry.push(waiter);
        if new {
            s.order.push_back(scid);
            drop(s);
            self.wake.notify_one();
        }
        UtxoResult::Async(future)
    }
}

/// Sends queued lookups to the node and resolves them, until the peer stops. See the module docs.
pub(crate) async fn run<C: Chain>(gossip: Arc<Gossip>, chain: C, pace: Arc<Pace>) {
    let lookup = Arc::clone(&gossip.lookup);
    let mut last: Option<Instant> = None;
    loop {
        // Wait for work: queued lookups, or early ones whose tip is due a check.
        let tip_only = loop {
            let woken = lookup.wake.notified();
            {
                let s = lookup.lock();
                if !s.order.is_empty() {
                    break false;
                }
                if !s.early.is_empty() && last.is_none_or(|t| t.elapsed() >= TIP_EVERY) {
                    break true;
                }
            }
            tokio::select! {
                () = woken => {}
                () = tokio::time::sleep(Duration::from_secs(1)) => {}
            }
        };
        tokio::time::sleep(LINGER).await;
        last = Some(Instant::now());

        if tip_only {
            lookup.lock().requests += 1;
            let tip = pace
                .run(async { tokio::time::timeout(REQUEST_TIMEOUT, chain.tip_height()).await })
                .await;
            match tip {
                Ok(Ok(tip)) => {
                    let mut s = lookup.lock();
                    s.tip = Some(tip);
                    Lookup::promote(&mut s, tip);
                }
                Ok(Err(e)) => failed(&gossip, e.label()).await,
                Err(_) => failed(&gossip, "timeout").await,
            }
            continue;
        }

        // The batch is taken once through the gate, so what arrived while it waited goes too.
        let (batch, answer) = pace
            .run(async {
                let batch: Vec<u64> = {
                    let mut s = lookup.lock();
                    let n = s.order.len().min(LOOKUP_BATCH);
                    let batch: Vec<u64> = s.order.drain(..n).collect();
                    s.requests += 1;
                    s.outputs += batch.len() as u64;
                    batch
                };
                let positions = OutPositions::new(batch.iter().map(|s| position(*s)).collect());
                let answer = match positions {
                    Some(p) => Some(tokio::time::timeout(REQUEST_TIMEOUT, chain.outputs(p)).await),
                    None => None,
                };
                (batch, answer)
            })
            .await;
        let Some(answer) = answer else {
            continue;
        };
        let answer = match answer {
            Ok(Ok(Stamped {
                value,
                tip: Some(tip),
            })) if value.len() == batch.len() => Ok((tip.height, tip.hash, value)),
            // Without a tip stamp, or of the wrong length.
            Ok(Ok(_)) => Err("malformed"),
            Ok(Err(e)) => Err(e.label()),
            Err(_) => Err("timeout"),
        };
        let (tip, tip_hash, slots) = match answer {
            Ok(a) => a,
            Err(e) => {
                {
                    let mut s = lookup.lock();
                    for scid in batch.into_iter().rev() {
                        s.order.push_front(scid);
                    }
                }
                failed(&gossip, e).await;
                continue;
            }
        };
        let (mut settled, mut verified, mut answered) = (Vec::new(), Vec::new(), Vec::new());
        {
            let mut s = lookup.lock();
            s.failures = 0;
            s.tip = Some(s.tip.map_or(tip, |t| t.max(tip)));
            for (scid, slot) in batch.iter().zip(slots) {
                let spent = spent_output(*scid, slot.as_ref());
                let outcome = classify(*scid, (tip, tip_hash), slot);
                answered.push((*scid, matches!(outcome, Outcome::Early { .. }), spent));
                lookup.settle(&mut s, *scid, outcome, &mut settled, &mut verified);
            }
            Lookup::promote(&mut s, tip);
            s.resolving += settled.len();
        }
        // Before the lookups count as done, so the replay is not over before its answers are in.
        for (scid, early, spent) in answered {
            gossip.watch.answered(scid, early, spent);
        }
        // Resolved outside our lock: LDK takes its own graph locks here.
        let (graph, n) = (gossip.graph(), settled.len());
        for (future, result) in settled {
            future.resolve_without_forwarding(graph, result);
        }
        lookup.lock().resolving -= n;
        for (scid, out, funded) in verified {
            gossip.admitted(scid, Some((out, funded)));
        }
        lookup.resolved.notify_one();
    }
}

/// Counts a failed request, reports it with the backlog, and waits out the backoff.
async fn failed(gossip: &Gossip, error: &'static str) {
    let (failures, pending) = {
        let mut s = gossip.lookup.lock();
        s.failures = s.failures.saturating_add(1);
        (s.failures, s.waiting_len)
    };
    let _ = gossip.events.send(Event::LookupFailed {
        failures,
        error,
        pending,
    });
    let backoff = RETRY_MIN
        .saturating_mul(1 << failures.saturating_sub(1).min(16))
        .min(RETRY_MAX);
    tokio::time::sleep(backoff).await;
}
