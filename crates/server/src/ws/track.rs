//! Following one transaction or one address: the `tx:<txid>` and `address:<address>` topics
//! (`docs/WS.md`), and the replacement record they share with `/app`.
//!
//! A transaction's state comes from memory only: the mempool tracker's records and removal
//! reasons, the projected blocks' positions, and the record of recent replacements. Following it
//! never asks the node anything. An address is one shared, reference-counted Electrum
//! subscription per network, whatever the number of sockets following it (SECURITY.md rule 10).
//!
//! Every message of these topics is the topic's whole state, so a client keeps the last one.
//! A tracked item lives while a socket subscribes to it; the last unsubscribe forgets it, its
//! stored message and, for an address, the node's subscription.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::http::StatusCode;
use bitcoin::{BlockHash, Txid};
use engine::mempool::{Delta, Removal, Tracker};
use engine::projected::{Position, Projector};
use logging::events::{TrackingRefused, TrackingSummary};
use node::electrum::{self, ScriptHash, Status};
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;
use tokio::time::Instant;

use super::{Conn, Frame, Hub, Snapshot, Topic};
use crate::config::{Network, WsLimits};
use crate::replacements::{Link, Replacements};
use crate::{ApiError, esplora};

/// The `/app` answer while the network's mempool engine is switched off.
pub const MEMPOOL_OFF: ApiError =
    ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "mempool is switched off");
/// The `/app` answer before the tracker's first complete cycle.
pub const MEMPOOL_SYNCING: ApiError =
    ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "mempool is syncing");
/// How often a network with anything tracked writes a `tracking_summary` line.
const SUMMARY_EVERY: Duration = Duration::from_secs(60);

/// A tracked topic's name as a client writes it, parsed strictly: `tx:` and 64 lowercase hex
/// digits, or `address:` and an address of this network. Returns the topic and its name as the
/// server writes it (bech32 in lower case).
pub fn parse(name: &str, network: Network) -> Option<(Topic, String)> {
    if let Some(hex) = name.strip_prefix("tx:") {
        if !hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return None;
        }
        let txid = esplora::txid(hex).ok()?;
        return Some((Topic::Tx(txid), name.to_owned()));
    }
    let address = esplora::address(name.strip_prefix("address:")?, network).ok()?;
    let sh = ScriptHash::from_script(&address.script_pubkey());
    Some((Topic::Address(sh), format!("address:{address}")))
}

/// Settings that bound tracking, from [`WsLimits`].
#[derive(Debug, Clone, Copy)]
struct Bounds {
    txs_total: usize,
    addresses_total: usize,
    position_interval: Duration,
}

/// One network's tracked transactions and addresses.
pub struct Tracking {
    network: Network,
    hub: Arc<Hub>,
    electrum: Option<electrum::Client>,
    mempool: bool,
    bounds: Bounds,
    engines: OnceLock<(Tracker, Projector)>,
    state: Mutex<State>,
}

impl std::fmt::Debug for Tracking {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tracking")
            .field("network", &self.network)
            .finish_non_exhaustive()
    }
}

struct State {
    txs: HashMap<Txid, TxEntry>,
    addresses: HashMap<ScriptHash, AddrEntry>,
    replacements: Replacements,
    /// Distinguishes an address entry from a later one for the same script.
    next_gen: u64,
    stats: Stats,
}

/// Counted since the last `tracking_summary` line.
#[derive(Default)]
struct Stats {
    subscribed: u64,
    refused: u64,
    messages: u64,
    bytes: u64,
    address_errors: u64,
    /// Refusals for a full server since start, by kind, for [`TrackingRefused`].
    refused_txs: u64,
    refused_addresses: u64,
}

struct TxEntry {
    /// Sockets subscribed.
    refs: usize,
    /// Why it last left the mempool, from the tracker's stream, while it is out of it.
    gone: Option<Removal>,
    /// What the topic last said, and when.
    sent: Option<(Shown, Instant)>,
}

struct AddrEntry {
    refs: usize,
    /// The topic's name as sent.
    name: Arc<str>,
    epoch: u64,
    /// The node's subscription; `None` until it answers or after it refused.
    sub: Option<electrum::Subscription>,
    /// What the topic last said; `None` while waiting for the node.
    state: Option<AddrState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AddrState {
    /// The node watches it. Its Electrum status: a hash of the address's history (`None` for no
    /// history), so it changes whenever a payment enters the mempool or confirms.
    Watching(Option<String>),
    /// The node is unreachable; asked again when the connection is back.
    Unavailable,
    /// The node refused: `too_large`, `no_index`, `server_full` or `refused`.
    Refused(&'static str),
}

/// A transaction's state as the topic says it.
#[derive(Debug, Clone, PartialEq)]
enum TxState {
    /// `position` is `None` for the moment between the tracker seeing it and the projected
    /// blocks placing it.
    Mempool {
        time: u32,
        fee: u64,
        vsize: u32,
        position: Option<Position>,
    },
    /// The chain of replacements from this one, oldest first, and whether it was cut.
    Replaced(Vec<Link>, bool),
    Confirmed {
        height: u32,
        hash: BlockHash,
    },
    /// Evicted, expired, or dropped for a reason the node does not report.
    Evicted,
    /// Not in the mempool and nothing recorded: confirmed before we were asked, never seen, or
    /// gone while we were not looking.
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
struct Shown {
    state: TxState,
    stale: bool,
    /// Projected blocks, so a client knows which one means "beyond".
    blocks: usize,
}

impl Shown {
    /// Equal but for where it waits: how much is ahead of it, or which later block it is in. A
    /// shuffle, sent at most once per `WS_POSITION_INTERVAL_MS`. Entering or leaving the next
    /// block, or a new effective rate, is news and goes out at once. On mainnet a transaction in
    /// the large class of equal chunks at the bottom moves between later blocks every few
    /// seconds; that is the projection's noise, not something a reader needs each time.
    fn shuffled(&self, other: &Self) -> bool {
        let TxState::Mempool {
            position: Some(b), ..
        } = &other.state
        else {
            return false;
        };
        let mut s = self.clone();
        let TxState::Mempool {
            position: Some(a), ..
        } = &mut s.state
        else {
            return false;
        };
        if a.placed.block != b.placed.block && (a.placed.block == 0 || b.placed.block == 0) {
            return false;
        }
        (a.ahead_vsize, a.ahead_txs, a.placed.block) = (b.ahead_vsize, b.ahead_txs, b.placed.block);
        s == *other
    }
}

/// Why a subscription to a tracked topic was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Full;

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Tracking {
    pub fn new(
        network: Network,
        hub: Arc<Hub>,
        electrum: Option<electrum::Client>,
        mempool: bool,
        limits: &WsLimits,
    ) -> Self {
        Self {
            network,
            hub,
            electrum,
            mempool,
            bounds: Bounds {
                txs_total: limits.tracked_txs_total,
                addresses_total: limits.tracked_addresses_total,
                position_interval: limits.position_interval,
            },
            engines: OnceLock::new(),
            state: Mutex::new(State {
                txs: HashMap::new(),
                addresses: HashMap::new(),
                replacements: Replacements::new(limits.replacements_kept),
                next_gen: 0,
                stats: Stats::default(),
            }),
        }
    }

    pub fn hub(&self) -> &Hub {
        &self.hub
    }

    pub fn mempool_enabled(&self) -> bool {
        self.mempool
    }

    /// The running engines, once [`super::mempool::run`] attached them.
    pub fn engines(&self) -> Option<&(Tracker, Projector)> {
        self.engines.get()
    }

    pub(crate) fn attach(&self, tracker: &Tracker, projector: &Projector) {
        let _ = self.engines.set((tracker.clone(), projector.clone()));
    }

    pub fn with_replacements<R>(&self, f: impl FnOnce(&Replacements) -> R) -> R {
        f(&lock(&self.state).replacements)
    }

    /// Transactions and addresses tracked now.
    pub fn tracked(&self) -> (usize, usize) {
        let st = lock(&self.state);
        (st.txs.len(), st.addresses.len())
    }

    /// Adds `conn` to a tracked topic and queues its state to it. Beyond the server's total for
    /// that kind, `conn` is sent a `refused` message instead and nothing is subscribed.
    pub(super) fn subscribe(
        self: &Arc<Self>,
        topic: Topic,
        name: &str,
        conn: &Arc<Conn>,
    ) -> Result<(), Full> {
        // Read before taking the lock: a position lookup can wait for a pack.
        let view = match topic {
            Topic::Tx(txid) if self.mempool => self.view(&[txid]),
            _ => None,
        };
        let mut st = lock(&self.state);
        match topic {
            Topic::Tx(txid) => {
                if !st.txs.contains_key(&txid) && st.txs.len() >= self.bounds.txs_total {
                    st.stats.refused += 1;
                    st.stats.refused_txs += 1;
                    self.refuse(conn, name, "tx", st.stats.refused_txs);
                    return Err(Full);
                }
                let entry = st.txs.entry(txid).or_insert(TxEntry {
                    refs: 0,
                    gone: None,
                    sent: None,
                });
                entry.refs += 1;
                if entry.refs == 1 {
                    st.stats.subscribed += 1;
                    self.first_tx_message(&mut st, txid, view.as_ref());
                }
            }
            Topic::Address(sh) => {
                if !st.addresses.contains_key(&sh)
                    && st.addresses.len() >= self.bounds.addresses_total
                {
                    st.stats.refused += 1;
                    st.stats.refused_addresses += 1;
                    self.refuse(conn, name, "address", st.stats.refused_addresses);
                    return Err(Full);
                }
                if let Some(e) = st.addresses.get_mut(&sh) {
                    e.refs += 1;
                } else {
                    st.next_gen += 1;
                    let epoch = st.next_gen;
                    st.addresses.insert(
                        sh,
                        AddrEntry {
                            refs: 1,
                            name: name.into(),
                            epoch,
                            sub: None,
                            state: None,
                        },
                    );
                    st.stats.subscribed += 1;
                    tokio::spawn(watch(Arc::clone(self), sh, epoch));
                }
            }
            _ => return Err(Full),
        }
        // Under the same lock as every publish of these topics, so the state queued here is
        // always followed by the changes after it.
        self.hub.subscribe(topic, conn);
        Ok(())
    }

    pub(super) fn unsubscribe(&self, topic: Topic, id: u64) {
        let mut st = lock(&self.state);
        self.hub.unsubscribe(topic, id);
        let last = match topic {
            Topic::Tx(t) => st.txs.get_mut(&t).is_some_and(|e| {
                e.refs = e.refs.saturating_sub(1);
                e.refs == 0
            }),
            Topic::Address(sh) => st.addresses.get_mut(&sh).is_some_and(|e| {
                e.refs = e.refs.saturating_sub(1);
                e.refs == 0
            }),
            _ => false,
        };
        if last {
            match topic {
                Topic::Tx(t) => {
                    st.txs.remove(&t);
                }
                // Dropping the entry drops its Electrum handle: the node stops watching.
                Topic::Address(sh) => {
                    st.addresses.remove(&sh);
                }
                _ => {}
            }
            self.hub.remove(topic);
        }
    }

    fn refuse(&self, conn: &Conn, name: &str, kind: &'static str, total: u64) {
        let msg = json!({ "topic": name, "type": "refused", "reason": "server_full" });
        conn.send(Frame::text(msg.to_string().as_bytes()));
        if total.is_power_of_two() {
            logging::emit(&TrackingRefused {
                network: self.network.name(),
                kind,
                reason: "server_full",
                total,
            });
        }
    }

    fn publish(&self, st: &mut State, topic: Topic, msg: &Value) {
        let frame = Frame::text(msg.to_string().as_bytes());
        st.stats.messages += 1;
        st.stats.bytes += frame.len() as u64;
        self.hub.publish(topic, &frame, Snapshot::Replace);
    }

    // Transactions.

    /// What the engines hold for `txids`, read without the tracking lock. `None` before they
    /// are attached or before the tracker's first cycle: nothing honest can be said yet.
    fn view(&self, txids: &[Txid]) -> Option<View> {
        let (tracker, projector) = self.engines.get()?;
        let infos: Option<Vec<(Txid, Record)>> = tracker.read(|m| {
            m.tip()?;
            Some(
                txids
                    .iter()
                    .filter_map(|t| {
                        Some((*t, m.get(t).map(|i| (i.time, i.fee.to_sat(), i.vsize))?))
                    })
                    .collect(),
            )
        });
        let p = projector.current();
        Some(View {
            stale: p.stale,
            blocks: p.blocks.len(),
            txs: infos?
                .into_iter()
                .map(|(t, i)| (t, (i, projector.position(&t))))
                .collect(),
        })
    }

    /// The state a new tracked transaction starts with, published as its topic's state. Without
    /// a view there is nothing honest to say yet, so nothing is published; the next
    /// [`Tracking::publish_txs`] says it.
    fn first_tx_message(&self, st: &mut State, txid: Txid, view: Option<&View>) {
        let topic = Topic::Tx(txid);
        if !self.mempool {
            let msg = json!({ "topic": tx_name(&txid), "type": "off" });
            self.publish(st, topic, &msg);
            return;
        }
        let Some(view) = view else {
            return;
        };
        let shown = view.shown(&txid, None, &st.replacements);
        let msg = tx_message(&txid, &shown);
        if let Some(e) = st.txs.get_mut(&txid) {
            e.sent = Some((shown, Instant::now()));
        }
        self.publish(st, topic, &msg);
    }

    /// Folds one tracker delta in: every replacement goes into the record, and each tracked
    /// transaction that left or came back has its reason set or cleared.
    fn apply(&self, d: &Delta, tracker: &Tracker) {
        let mut st = lock(&self.state);
        let seen = unix_now();
        for (txid, info) in &d.replaced {
            let Ok(i) = d.removed.binary_search_by_key(txid, |(t, _)| *t) else {
                continue;
            };
            let Some((_, Removal::Replaced { by })) = d.removed.get(i) else {
                continue;
            };
            let by_info = d
                .added
                .binary_search_by_key(by, |(t, _)| *t)
                .ok()
                .and_then(|i| d.added.get(i))
                .map(|(_, i)| (i.fee.to_sat(), i.vsize))
                .or_else(|| tracker.read(|m| m.get(by).map(|i| (i.fee.to_sat(), i.vsize))));
            st.replacements.insert(Link {
                txid: *txid,
                by: *by,
                seen,
                fee: info.fee.to_sat(),
                vsize: info.vsize,
                by_fee: by_info.map(|b| b.0),
                by_vsize: by_info.map(|b| b.1),
            });
        }
        if st.txs.is_empty() {
            return;
        }
        for (txid, reason) in &d.removed {
            if let Some(e) = st.txs.get_mut(txid) {
                e.gone = Some(*reason);
            }
        }
        for (txid, _) in &d.added {
            if let Some(e) = st.txs.get_mut(txid) {
                e.gone = None;
            }
        }
    }

    /// After losing our place in the tracker's stream: reasons may be out of date, so forget them.
    fn lost_place(&self) {
        for e in lock(&self.state).txs.values_mut() {
            e.gone = None;
        }
    }

    /// Publishes every tracked transaction whose state changed. A change of position within the
    /// same projected block waits until `WS_POSITION_INTERVAL_MS` after the last message; returns
    /// whether one is waiting.
    fn publish_txs(&self) -> bool {
        let txids: Vec<Txid> = lock(&self.state).txs.keys().copied().collect();
        if txids.is_empty() {
            return false;
        }
        let Some(view) = self.view(&txids) else {
            return false;
        };
        let now = Instant::now();
        let interval = self.bounds.position_interval;
        let mut waiting = false;
        let mut out = Vec::new();
        let mut st = lock(&self.state);
        let State {
            txs, replacements, ..
        } = &mut *st;
        for txid in &txids {
            // Released meanwhile.
            let Some(e) = txs.get_mut(txid) else {
                continue;
            };
            let shown = view.shown(txid, e.gone, replacements);
            let send = match &e.sent {
                None => true,
                Some((last, _)) if *last == shown => false,
                Some((last, at)) if last.shuffled(&shown) => {
                    let due = now.duration_since(*at) >= interval;
                    waiting |= !due;
                    due
                }
                Some(_) => true,
            };
            if send {
                out.push((Topic::Tx(*txid), tx_message(txid, &shown)));
                e.sent = Some((shown, now));
            }
        }
        for (topic, msg) in out {
            self.publish(&mut st, topic, &msg);
        }
        waiting
    }

    // Addresses.

    fn address_message(&self, st: &mut State, sh: ScriptHash) {
        let Some(e) = st.addresses.get(&sh) else {
            return;
        };
        let Some(state) = &e.state else {
            return;
        };
        let fields = match state {
            AddrState::Watching(status) => json!({ "state": "watching", "status": status }),
            AddrState::Unavailable => json!({ "state": "unavailable" }),
            AddrState::Refused(reason) => json!({ "state": "refused", "reason": reason }),
        };
        let msg = merge(json!({ "topic": &*e.name, "type": "address" }), fields);
        self.publish(st, Topic::Address(sh), &msg);
    }

    /// A status the node pushed for a script.
    fn address_status(&self, sh: ScriptHash, status: Option<String>) {
        let mut st = lock(&self.state);
        let Some(e) = st.addresses.get_mut(&sh) else {
            return;
        };
        let next = Some(AddrState::Watching(status));
        if e.sub.is_none() || e.state == next {
            return;
        }
        e.state = next;
        self.address_message(&mut st, sh);
    }

    /// After a reconnect or a lost event: re-read every status, and ask again for the addresses
    /// the node could not be reached for.
    fn refresh_addresses(self: &Arc<Self>) {
        let mut st = lock(&self.state);
        let mut changed = Vec::new();
        let mut retry = Vec::new();
        for (sh, e) in &mut st.addresses {
            match (&e.sub, &e.state) {
                (Some(sub), _) => {
                    let next = match sub.status() {
                        Some(Status::ScriptHash(s)) => AddrState::Watching(s),
                        // The node no longer watches it (it refused a re-subscribe).
                        _ => AddrState::Unavailable,
                    };
                    if e.state.as_ref() != Some(&next) {
                        if next == AddrState::Unavailable {
                            e.sub = None;
                            retry.push((*sh, e.epoch));
                        }
                        e.state = Some(next);
                        changed.push(*sh);
                    }
                }
                (None, Some(AddrState::Unavailable)) => {
                    // Marked pending again so a burst of events asks only once.
                    e.state = None;
                    retry.push((*sh, e.epoch));
                }
                _ => {}
            }
        }
        for sh in changed {
            self.address_message(&mut st, sh);
        }
        drop(st);
        for (sh, epoch) in retry {
            tokio::spawn(watch(Arc::clone(self), sh, epoch));
        }
    }

    /// One `tracking_summary` line, if anything is tracked or happened since the last.
    fn summary(&self) {
        let mut st = lock(&self.state);
        let recorded = st.replacements.take_recorded();
        let s = std::mem::take(&mut st.stats);
        st.stats.refused_txs = s.refused_txs;
        st.stats.refused_addresses = s.refused_addresses;
        let (txs, addresses) = (st.txs.len(), st.addresses.len());
        if txs + addresses == 0 && s.subscribed == 0 && s.refused == 0 && recorded == 0 {
            return;
        }
        logging::emit(&TrackingSummary {
            network: self.network.name(),
            txs,
            addresses,
            subscribed: s.subscribed,
            refused: s.refused,
            messages: s.messages,
            bytes: s.bytes,
            address_errors: s.address_errors,
            replacements: st.replacements.len(),
            replaced: recorded,
        });
    }
}

/// The topic's name for a transaction.
fn tx_name(txid: &Txid) -> String {
    format!("tx:{txid}")
}

/// A mempool transaction's own figures from the tracker: accept time, fee, vsize.
type Record = (u32, u64, u32);

/// The engines' view of some transactions at one moment.
struct View {
    stale: bool,
    blocks: usize,
    /// Those in the mempool: time, fee, vsize, and the position if placed yet.
    txs: HashMap<Txid, (Record, Option<Position>)>,
}

impl View {
    /// What is known about `txid`: in the mempool, or why it left.
    fn shown(&self, txid: &Txid, gone: Option<Removal>, replacements: &Replacements) -> Shown {
        Shown {
            state: state(txid, self.txs.get(txid).copied(), gone, replacements),
            stale: self.stale,
            blocks: self.blocks,
        }
    }
}

fn state(
    txid: &Txid,
    held: Option<(Record, Option<Position>)>,
    gone: Option<Removal>,
    replacements: &Replacements,
) -> TxState {
    if let Some(((time, fee, vsize), position)) = held {
        return TxState::Mempool {
            time,
            fee,
            vsize,
            position,
        };
    }
    let replaced = |by: Option<Txid>| {
        let (chain, cut) = replacements.chain(txid);
        match (chain.is_empty(), by) {
            (false, _) => Some(TxState::Replaced(chain, cut)),
            // Recorded once, but pushed out of the record since: say what we still know.
            (true, Some(by)) => Some(TxState::Replaced(
                vec![Link {
                    txid: *txid,
                    by,
                    seen: 0,
                    fee: 0,
                    vsize: 0,
                    by_fee: None,
                    by_vsize: None,
                }],
                false,
            )),
            (true, None) => None,
        }
    };
    match gone {
        Some(Removal::Confirmed { height, block }) => TxState::Confirmed {
            height,
            hash: block,
        },
        Some(Removal::Replaced { by }) => replaced(Some(by)).unwrap_or(TxState::Evicted),
        Some(Removal::Evicted) => TxState::Evicted,
        None => replaced(None).unwrap_or(TxState::Unknown),
    }
}

/// Sat per 1,000 vB, rounded down.
fn rate(fee: u64, vsize: u32) -> u64 {
    fee.saturating_mul(1000) / u64::from(vsize.max(1))
}

/// `head` with the fields of `more` added.
fn merge(mut head: Value, more: Value) -> Value {
    if let (Value::Object(h), Value::Object(m)) = (&mut head, more) {
        h.extend(m);
    }
    head
}

fn tx_message(txid: &Txid, s: &Shown) -> Value {
    let fields = match &s.state {
        TxState::Mempool {
            time,
            fee,
            vsize,
            position,
        } => json!({
            "state": "mempool",
            "time": time,
            "fee": fee,
            "vsize": vsize,
            "rate": rate(*fee, *vsize),
            "position": position.map(|p| json!({
                "block": p.placed.block,
                "blocks": s.blocks,
                "effective_rate": p.placed.rate,
                "ahead_vsize": p.ahead_vsize,
                "ahead_txs": p.ahead_txs,
            })),
        }),
        TxState::Replaced(chain, cut) => json!({
            "state": "replaced",
            "by": chain.first().map(|l| l.by),
            "chain": chain
                .iter()
                // A link known only from the tracker's reason has no figures.
                .map(|l| if l.vsize == 0 { json!({ "txid": l.txid, "by": l.by }) } else { l.json() })
                .collect::<Vec<_>>(),
            "truncated": cut,
        }),
        TxState::Confirmed { height, hash } => {
            json!({ "state": "confirmed", "height": height, "hash": hash })
        }
        TxState::Evicted => json!({ "state": "evicted" }),
        TxState::Unknown => json!({ "state": "unknown" }),
    };
    merge(
        json!({ "topic": tx_name(txid), "type": "tx", "stale": s.stale }),
        fields,
    )
}

/// Asks the node to watch an address, and says what it answered. An entry released meanwhile is
/// not revived: dropping the answer's handle releases the node's subscription again.
async fn watch(track: Arc<Tracking>, sh: ScriptHash, epoch: u64) {
    let answer = match &track.electrum {
        Some(client) => Some(client.subscribe(electrum::Topic::ScriptHash(sh)).await),
        None => None,
    };
    let mut st = lock(&track.state);
    let Some(e) = st.addresses.get_mut(&sh).filter(|e| e.epoch == epoch) else {
        return;
    };
    let failed = match answer {
        Some(Ok(sub)) => {
            let status = match sub.status() {
                Some(Status::ScriptHash(s)) => s,
                _ => None,
            };
            e.sub = Some(sub);
            e.state = Some(AddrState::Watching(status));
            false
        }
        Some(Err(err)) => {
            e.state = Some(refusal(&err));
            true
        }
        None => {
            e.state = Some(AddrState::Unavailable);
            true
        }
    };
    if failed {
        st.stats.address_errors += 1;
    }
    track.address_message(&mut st, sh);
}

/// What a failed Electrum subscribe means for the client.
fn refusal(e: &electrum::Error) -> AddrState {
    match e {
        electrum::Error::SubscriptionLimit { .. } => AddrState::Refused("server_full"),
        // rbitcoin v0.9.2 answers code 1 with "scripthash index disabled", or "scripthash join
        // exceeds --max-sh-creates (N creates, cap M)" for an address with too much history.
        electrum::Error::Server { message, .. } => {
            if message.contains("index disabled") {
                AddrState::Refused("no_index")
            } else if message.contains("max-sh-creates") {
                AddrState::Refused("too_large")
            } else {
                AddrState::Refused("refused")
            }
        }
        electrum::Error::Disconnected
        | electrum::Error::Timeout
        | electrum::Error::Overloaded
        | electrum::Error::Protocol(_) => AddrState::Unavailable,
    }
}

/// Follows the engines for the tracked transactions: the tracker's stream for removal reasons
/// and replacements, the projected blocks for positions. Publishes at most once per
/// `WS_MEMPOOL_INTERVAL_MS`. One task per network, whatever the number of subscribers.
pub(crate) async fn follow_txs(track: Arc<Tracking>, interval: Duration) {
    let Some((tracker, projector)) = track.engines.get().cloned() else {
        return;
    };
    loop {
        let ((), mut deltas) = tracker.subscribe(|_| ());
        let (_, mut projections) = projector.subscribe();
        track.lost_place();
        let (mut dirty, mut next) = (true, Instant::now());
        loop {
            tokio::select! {
                d = deltas.recv() => match d {
                    Ok(d) => {
                        track.apply(&d, &tracker);
                        dirty = true;
                    }
                    Err(RecvError::Lagged(_)) => break,
                    Err(RecvError::Closed) => return,
                },
                p = projections.recv() => match p {
                    Ok(_) | Err(RecvError::Lagged(_)) => dirty = true,
                    Err(RecvError::Closed) => return,
                },
                () = tokio::time::sleep_until(next), if dirty => {
                    dirty = track.publish_txs();
                    next = Instant::now() + interval;
                }
            }
        }
    }
}

/// Follows the network's Electrum events for the tracked addresses, and writes the minute's
/// `tracking_summary`. One task per network.
pub async fn follow_addresses(
    track: Arc<Tracking>,
    mut events: tokio::sync::broadcast::Receiver<electrum::Event>,
) {
    let mut tick = tokio::time::interval_at(Instant::now() + SUMMARY_EVERY, SUMMARY_EVERY);
    loop {
        tokio::select! {
            e = events.recv() => match e {
                Ok(electrum::Event::Status {
                    topic: electrum::Topic::ScriptHash(sh),
                    status: Status::ScriptHash(s),
                }) => track.address_status(sh, s),
                Ok(electrum::Event::Resynced | electrum::Event::Block(_))
                | Err(RecvError::Lagged(_)) => track.refresh_addresses(),
                Ok(_) => {}
                Err(RecvError::Closed) => return,
            },
            _ = tick.tick() => track.summary(),
        }
    }
}

#[cfg(test)]
mod tests;
