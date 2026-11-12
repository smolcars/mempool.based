//! The routing-message handler the peer manager talks to: LDK's `P2PGossipSync` behind a
//! listener-only front that counts, bounds and reports what arrives, and never serves or relays.
//!
//! LDK asks each peer for gossip with a timestamp filter only. Eclair, and LND nodes that ignore
//! historical filters, answer that with new gossip only. So every peer that supports gossip
//! queries is also asked for every channel id it knows (`query_channel_range`), and the channels
//! missing here are requested from it in batches (`query_short_channel_ids`), one batch at a time
//! per peer and never the same channel from two peers at once. Peers' graphs differ (on mainnet
//! eclair listed 21,600 channels and LND 30,250), so asking every peer gives the union. Channels
//! we hold whose nodes have sent no announcement are requested too, once per node and process: a
//! timestamp filter brings only the last two weeks of node announcements, and most nodes announce
//! less often, while a peer answering for a channel also sends its nodes' announcements.
//!
//! Every channel announcement is checked against the chain before it enters the graph
//! ([`crate::chain`]). A graph loaded from disk is not trusted: its channels are replayed through
//! the same check ([`Replay`]), and are absent from the graph until they pass.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use lightning::bitcoin::constants::ChainHash;
use lightning::bitcoin::secp256k1::{PublicKey, Secp256k1, VerifyOnly};
use lightning::ln::chan_utils::make_funding_redeemscript;
use lightning::ln::msgs::{
    BaseMessageHandler, ChannelAnnouncement, ChannelUpdate, ErrorAction, Init, LightningError,
    MessageSendEvent, NodeAnnouncement, QueryChannelRange, QueryShortChannelIds, ReplyChannelRange,
    ReplyShortChannelIdsEnd, RoutingMessageHandler, UnsignedNodeAnnouncement,
};
use lightning::routing::gossip::{
    NetworkGraph, NodeId, P2PGossipSync, ReadOnlyNetworkGraph, verify_node_announcement,
};
use lightning::routing::utxo::UtxoLookup;
use lightning::types::features::{InitFeatures, NodeFeatures};
use lightning::util::ser::Writeable;
use tokio::sync::broadcast;

use crate::chain::Lookup;
use crate::log::LogSink;
use crate::{Change, Config, Event};

/// The funding-output check LDK runs for each channel announcement: [`Lookup`].
pub(crate) type UtxoHook = Arc<dyn UtxoLookup + Send + Sync>;
pub(crate) type Graph = NetworkGraph<Arc<LogSink>>;
type GossipSync = P2PGossipSync<Arc<Graph>, UtxoHook, Arc<LogSink>>;

/// Largest gossip message kept, in bytes as encoded. Real announcements are under 500 bytes; the
/// wire allows 65 KB of features, addresses and extra data, all of which LDK would keep.
pub(crate) const MAX_GOSSIP_BYTES: usize = 1024;
/// Addresses kept per node. Some real nodes announce one onion address hundreds of times.
pub(crate) const MAX_NODE_ADDRESSES: usize = 8;

/// Stored channels sent for checking per supervisor tick, at most: each costs four signature
/// checks now and four when its lookup resolves.
const REPLAY_PER_TICK: usize = 2000;

/// Channel ids per `query_short_channel_ids`: 16 KB of ids, well inside a 65 KB message.
const SCID_BATCH: usize = 2000;
/// A batch with no answer for this long is given up; its channels return at the next range sync.
const BATCH_TIMEOUT: Duration = Duration::from_secs(120);
/// A channel list whose replies stop for this long, short of the queried range, counts as done.
const LISTING_TIMEOUT: Duration = Duration::from_secs(60);

/// The channel-range sync with every connected peer.
#[derive(Default)]
struct RangeSync {
    peers: HashMap<PublicKey, PeerRange>,
    /// Channel ids wanted from or requested of some peer. Bounded by the graph ceiling.
    queued: HashSet<u64>,
    /// Unannounced nodes a channel was already requested for: many never announce, and asking
    /// again on every connection would fetch their channels each time. Nodes that leave the graph
    /// are forgotten after each prune, so it stays within the graph's nodes.
    asked: HashSet<NodeId>,
}

#[derive(Default)]
struct PeerRange {
    /// While the peer's channel list is still arriving: when its last reply (or the query) came.
    listing: Option<Instant>,
    /// Channel ids this peer has and we lack, not yet requested.
    wanted: VecDeque<u64>,
    /// The batch requested and not yet answered, and when it was sent.
    in_flight: Option<(Vec<u64>, Instant)>,
}

impl RangeSync {
    /// Forgets a peer's sync. What it still owed is freed for the next range reply to claim.
    fn finish_batch(&mut self, peer: &PublicKey) {
        if let Some(p) = self.peers.get_mut(peer)
            && let Some((batch, _)) = p.in_flight.take()
        {
            for scid in batch {
                self.queued.remove(&scid);
            }
        }
    }
}

/// A channel from the graph file, waiting to be checked again: its announcement and latest updates.
pub(crate) struct Stored {
    announcement: ChannelAnnouncement,
    /// By direction: `channel_flags & 1`.
    updates: [Option<ChannelUpdate>; 2],
}

/// What the graph file held, checked again after a restart. Updates and node announcements that
/// arrive meanwhile for a stored channel or node are kept here if newer, so they are not refused
/// for a channel LDK does not know yet.
#[derive(Default)]
pub(crate) struct Replay {
    channels: HashMap<u64, Stored>,
    /// Channel ids still to send, popped from the end.
    order: Vec<u64>,
    nodes: HashMap<NodeId, UnsignedNodeAnnouncement>,
    /// Channel ids sent and not yet answered, early ones included.
    sent: Vec<u64>,
    /// Until every stored channel was sent and its lookup answered.
    active: bool,
}

impl Replay {
    /// Takes every channel with its stored announcement (LDK keeps it unless its extra data was
    /// over 1 KB, which we refuse anyway) and every node announcement.
    pub(crate) fn from_graph(graph: &Graph) -> Self {
        let g = graph.read_only();
        let mut r = Self::default();
        for (scid, c) in g.channels().unordered_iter() {
            let Some(announcement) = c.announcement_message.clone() else {
                continue;
            };
            let update = |u: &Option<crate::ChannelUpdateInfo>| {
                u.as_ref().and_then(|u| u.last_update_message.clone())
            };
            r.channels.insert(
                *scid,
                Stored {
                    announcement,
                    updates: [update(&c.one_to_two), update(&c.two_to_one)],
                },
            );
            r.order.push(*scid);
        }
        for (id, n) in g.nodes().unordered_iter() {
            if let Some(info) = &n.announcement_info {
                r.nodes.insert(
                    *id,
                    UnsignedNodeAnnouncement {
                        features: info.features().clone(),
                        timestamp: info.last_update(),
                        node_id: *id,
                        rgb: info.rgb(),
                        alias: *info.alias(),
                        addresses: info.addresses().to_vec(),
                        excess_address_data: Vec::new(),
                        excess_data: Vec::new(),
                    },
                );
            }
        }
        // Oldest channels first: popped from the end.
        r.order.sort_unstable_by(|a, b| b.cmp(a));
        r.active = !r.order.is_empty();
        r
    }
}

/// Totals since start. Read by the supervisor to compute rates.
#[derive(Default)]
pub(crate) struct Counters {
    pub(crate) channel_announcements: AtomicU64,
    pub(crate) node_announcements: AtomicU64,
    pub(crate) channel_updates: AtomicU64,
    pub(crate) bytes: AtomicU64,
    /// Waiting for a funding-output lookup.
    pub(crate) held: AtomicU64,
    /// Accepted into the graph.
    pub(crate) accepted: AtomicU64,
    /// Already known.
    pub(crate) duplicate: AtomicU64,
    /// Refused by LDK: bad signature, unknown channel, stale, malformed.
    pub(crate) rejected: AtomicU64,
    /// Node announcements over [`MAX_GOSSIP_BYTES`] kept with their addresses cut down.
    pub(crate) trimmed: AtomicU64,
    /// Refused by us: over [`MAX_GOSSIP_BYTES`].
    pub(crate) oversize: AtomicU64,
    /// Refused by us: the graph is at its ceiling.
    pub(crate) over_ceiling: AtomicU64,
}

pub(crate) struct Gossip {
    sync: GossipSync,
    pub(crate) lookup: Arc<Lookup>,
    replay: Mutex<Replay>,
    chain: ChainHash,
    secp: Secp256k1<VerifyOnly>,
    max_channels: usize,
    /// Restart resume point: ask peers only for gossip newer than this (unix seconds).
    resume_from: Option<u32>,
    pub(crate) counters: Counters,
    at_ceiling: AtomicBool,
    /// When a channel new to the graph last arrived; the sync-settled signal reads it.
    pub(crate) last_new_channel: Mutex<Instant>,
    range: Mutex<RangeSync>,
    /// Held from the ceiling check to the insert, so concurrent peers cannot overshoot it.
    admission: Mutex<()>,
    /// Messages of our own for the peer manager to send.
    outbox: Mutex<Vec<MessageSendEvent>>,
    changes: broadcast::Sender<Change>,
    pub(crate) events: broadcast::Sender<Event>,
}

impl Gossip {
    pub(crate) fn new(
        config: &Config,
        graph: Arc<Graph>,
        logger: Arc<LogSink>,
        resume_from: Option<u32>,
        replay: Replay,
        changes: broadcast::Sender<Change>,
        events: broadcast::Sender<Event>,
    ) -> Self {
        let lookup = Arc::new(Lookup::new(config));
        Self {
            sync: P2PGossipSync::new(graph, Some(Arc::clone(&lookup) as UtxoHook), logger),
            lookup,
            replay: Mutex::new(replay),
            chain: ChainHash::using_genesis_block(config.network),
            secp: Secp256k1::verification_only(),
            max_channels: config.max_channels,
            resume_from,
            counters: Counters::default(),
            at_ceiling: AtomicBool::new(false),
            last_new_channel: Mutex::new(Instant::now()),
            range: Mutex::new(RangeSync::default()),
            admission: Mutex::new(()),
            outbox: Mutex::new(Vec::new()),
            changes,
            events,
        }
    }

    pub(crate) fn graph(&self) -> &Arc<Graph> {
        self.sync.network_graph()
    }

    /// After a prune: forgets nodes that left the graph, so `asked` stays within it, and clears
    /// the ceiling flag once the graph is back under it, so the next hit is reported.
    pub(crate) fn after_prune(&self) {
        if let Ok(mut r) = self.range.lock() {
            let graph = self.graph().read_only();
            r.asked.retain(|n| graph.node(n).is_some());
        }
        if self.graph().read_only().channels().len() < self.max_channels {
            self.at_ceiling.store(false, Ordering::Relaxed);
        }
    }

    #[cfg(test)]
    pub(crate) fn asked_len(&self) -> usize {
        self.range.lock().map_or(0, |r| r.asked.len())
    }

    /// A channel's lookup succeeded and LDK added it: count it and tell subscribers.
    pub(crate) fn admitted(&self, scid: u64) {
        let nodes = self
            .graph()
            .read_only()
            .channel(scid)
            .map(|c| (c.node_one, c.node_two));
        if let Some((one, two)) = nodes {
            self.counters.accepted.fetch_add(1, Ordering::Relaxed);
            if let Ok(mut t) = self.last_new_channel.lock() {
                *t = Instant::now();
            }
            self.changed(Change::Channel(scid));
            self.changed(Change::Node(one));
            self.changed(Change::Node(two));
        }
    }

    /// Stored channels not yet sent for checking.
    pub(crate) fn replay_left(&self) -> usize {
        self.replay.lock().map_or(0, |r| r.channels.len())
    }

    /// Stored channels are still to be sent or answered: the graph lacks some of the file's.
    pub(crate) fn replaying(&self) -> bool {
        self.replay.lock().is_ok_and(|r| r.active)
    }

    /// Sends stored channels for checking while the lookup queue is under half full, so live
    /// gossip keeps room, then applies stored node announcements of nodes now in the graph. Once
    /// every stored channel was sent and checked, the node announcements left (nodes with no
    /// verified channel) are dropped.
    pub(crate) fn replay_tick(&self, max_pending: usize) {
        let room = (max_pending / 2)
            .saturating_sub(self.lookup.waiting())
            .min(REPLAY_PER_TICK);
        let batch: Vec<Stored> = {
            let Ok(mut r) = self.replay.lock() else {
                return;
            };
            let n = r.order.len().min(room);
            let at = r.order.len() - n;
            let ids: Vec<u64> = r.order.drain(at..).collect();
            r.sent.extend(&ids);
            ids.iter().filter_map(|id| r.channels.remove(id)).collect()
        };
        for st in batch {
            let _ = self.check_announcement(None, &st.announcement);
            for u in st.updates.into_iter().flatten() {
                if self.graph().update_channel(&u).is_ok() {
                    self.changed(Change::Channel(u.contents.short_channel_id));
                }
            }
        }
        // The replay lock is never held while the graph is read: gossip handlers take them the
        // other way round.
        let (ids, done) = {
            let Ok(mut r) = self.replay.lock() else {
                return;
            };
            if !r.active {
                return;
            }
            // A stored channel found early is still the file's: wait for it too (about 12 blocks
            // at most, see `chain::EARLY_AHEAD`).
            r.sent.retain(|id| self.lookup.holds(*id));
            let done = r.channels.is_empty() && r.sent.is_empty() && self.lookup.waiting() == 0;
            (r.nodes.keys().copied().collect::<Vec<NodeId>>(), done)
        };
        let graph = self.graph().read_only();
        let ids: Vec<NodeId> = ids
            .into_iter()
            .filter(|id| graph.node(id).is_some())
            .collect();
        drop(graph);
        let ready: Vec<UnsignedNodeAnnouncement> = {
            let Ok(mut r) = self.replay.lock() else {
                return;
            };
            ids.iter().filter_map(|id| r.nodes.remove(id)).collect()
        };
        for n in ready {
            if self
                .graph()
                .update_node_from_unsigned_announcement(&n)
                .is_ok()
            {
                self.changed(Change::Node(n.node_id));
            }
        }
        if done && let Ok(mut r) = self.replay.lock() {
            r.nodes = HashMap::new();
            r.active = false;
        }
    }

    /// Puts a channel announcement through LDK, which asks [`Lookup`] for its funding output. The
    /// ceiling counts the channels in the graph and those waiting to enter it.
    fn check_announcement(
        &self,
        their_node_id: Option<PublicKey>,
        msg: &ChannelAnnouncement,
    ) -> Result<bool, LightningError> {
        let c = &msg.contents;
        if c.chain_hash != self.chain {
            self.lookup.count(|r| r.wrong_chain += 1);
            return Err(ignore("channel announced for another chain"));
        }
        let scid = c.short_channel_id;
        let _admission = self.admission.lock().unwrap_or_else(|e| e.into_inner());
        let graph = self.graph().read_only();
        let (known, len) = (graph.channel(scid).is_some(), graph.channels().len());
        drop(graph);
        let len = len + self.lookup.held() + self.replay_left();
        // Nodes enter the graph only through channels, so this also bounds nodes at twice as many.
        if !known && len >= self.max_channels {
            self.counters.over_ceiling.fetch_add(1, Ordering::Relaxed);
            if !self.at_ceiling.swap(true, Ordering::Relaxed) {
                let _ = self.events.send(Event::CeilingReached { channels: len });
            }
            return Err(ignore("graph at its ceiling"));
        }
        // The two-of-two the funding output must pay; LDK's own check compares the same script.
        let script = c
            .bitcoin_key_1
            .as_pubkey()
            .ok()
            .zip(c.bitcoin_key_2.as_pubkey().ok())
            .map(|(a, b)| make_funding_redeemscript(&a, &b).to_p2wsh());
        self.lookup.expect(scid, script);
        let r = self.sync.handle_channel_announcement(their_node_id, msg);
        self.lookup.expect(scid, None);
        if r.is_ok() {
            if !known && let Ok(mut t) = self.last_new_channel.lock() {
                *t = Instant::now();
            }
            self.changed(Change::Channel(scid));
            self.changed(Change::Node(c.node_id_1));
            self.changed(Change::Node(c.node_id_2));
        }
        r
    }

    /// Keeps a channel update for a stored channel not yet checked, if it is newer.
    fn hold_update(&self, msg: &ChannelUpdate) -> bool {
        let Ok(mut r) = self.replay.lock() else {
            return false;
        };
        let Some(st) = r.channels.get_mut(&msg.contents.short_channel_id) else {
            return false;
        };
        let Some(slot) = st
            .updates
            .get_mut(usize::from(msg.contents.channel_flags & 1))
        else {
            return false;
        };
        if slot
            .as_ref()
            .is_none_or(|u| u.contents.timestamp < msg.contents.timestamp)
        {
            *slot = Some(msg.clone());
        }
        true
    }

    /// Keeps a node announcement for a stored node not yet in the graph, if it is newer. It is
    /// applied unsigned later, so its signature is checked here.
    /// `trimmed` is an oversized announcement already checked and cut down.
    fn hold_node(
        &self,
        msg: &NodeAnnouncement,
        trimmed: Option<&UnsignedNodeAnnouncement>,
    ) -> bool {
        let stored = |r: &Replay| r.nodes.contains_key(&msg.contents.node_id);
        if !self.replay.lock().is_ok_and(|r| stored(&r))
            || self
                .graph()
                .read_only()
                .node(&msg.contents.node_id)
                .is_some()
            || (trimmed.is_none() && verify_node_announcement(msg, &self.secp).is_err())
        {
            return false;
        }
        let contents = trimmed.unwrap_or(&msg.contents);
        let Ok(mut r) = self.replay.lock() else {
            return false;
        };
        match r.nodes.get_mut(&msg.contents.node_id) {
            Some(s) if s.timestamp < contents.timestamp => *s = contents.clone(),
            Some(_) => {}
            None => return false,
        }
        true
    }

    pub(crate) fn at_ceiling(&self) -> bool {
        self.at_ceiling.load(Ordering::Relaxed)
    }

    fn count(&self, counter: &AtomicU64, bytes: usize) {
        counter.fetch_add(1, Ordering::Relaxed);
        self.counters
            .bytes
            .fetch_add(bytes as u64, Ordering::Relaxed);
    }

    fn admit(&self, counter: &AtomicU64, bytes: usize) -> Result<(), LightningError> {
        self.count(counter, bytes);
        if bytes > MAX_GOSSIP_BYTES {
            self.counters.oversize.fetch_add(1, Ordering::Relaxed);
            return Err(ignore("message over size limit"));
        }
        Ok(())
    }

    /// Counts LDK's verdict. Always answers `false` on success so the peer manager never relays.
    fn tally(&self, r: Result<bool, LightningError>) -> Result<bool, LightningError> {
        let c = &self.counters;
        match &r {
            Ok(_) => c.accepted.fetch_add(1, Ordering::Relaxed),
            Err(LightningError {
                action: ErrorAction::IgnoreDuplicateGossip,
                ..
            }) => c.duplicate.fetch_add(1, Ordering::Relaxed),
            // LDK's words for a message waiting on a lookup (lightning 0.2.8, routing/utxo.rs).
            Err(e)
                if e.err == "Channel being checked async"
                    || e.err
                        .starts_with("Awaiting channel_announcement validation") =>
            {
                c.held.fetch_add(1, Ordering::Relaxed)
            }
            Err(_) => c.rejected.fetch_add(1, Ordering::Relaxed),
        };
        r.map(|_| false)
    }

    /// An oversized node announcement, whose signature holds, cut down to at most
    /// [`MAX_NODE_ADDRESSES`] distinct addresses and no unknown fields. It is stored unsigned, which
    /// only matters for relaying, and we do not relay. `None` when it is still too big.
    fn trim(&self, msg: &NodeAnnouncement) -> Option<UnsignedNodeAnnouncement> {
        let mut contents = msg.contents.clone();
        let mut addresses = Vec::new();
        for a in std::mem::take(&mut contents.addresses) {
            if addresses.len() == MAX_NODE_ADDRESSES {
                break;
            }
            if !addresses.contains(&a) {
                addresses.push(a);
            }
        }
        contents.addresses = addresses;
        contents.excess_address_data = Vec::new();
        contents.excess_data = Vec::new();
        if contents.serialized_length() > MAX_GOSSIP_BYTES {
            self.counters.oversize.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        Some(contents)
    }

    fn changed(&self, change: Change) {
        let _ = self.changes.send(change);
    }

    fn send(&self, ev: MessageSendEvent) {
        if let Ok(mut o) = self.outbox.lock() {
            o.push(ev);
        }
    }

    /// Channels listed by peers and not yet received, requested or not.
    /// Peers whose channel list is still arriving count as one each.
    pub(crate) fn range_pending(&self) -> usize {
        self.range.lock().map_or(0, |r| {
            r.queued.len() + r.peers.values().filter(|p| p.listing.is_some()).count()
        })
    }

    /// Moves on from batches that fully arrived without their end marker, or timed out.
    pub(crate) fn range_tick(&self) {
        let Ok(mut r) = self.range.lock() else { return };
        for p in r.peers.values_mut() {
            if p.listing.is_some_and(|t| t.elapsed() >= LISTING_TIMEOUT) {
                p.listing = None;
            }
        }
        let graph = self.graph().read_only();
        let done: Vec<PublicKey> = r
            .peers
            .iter()
            .filter(|(_, p)| {
                p.in_flight.as_ref().is_some_and(|(batch, sent)| {
                    sent.elapsed() >= BATCH_TIMEOUT
                        || batch.iter().all(|scid| complete(&graph, *scid))
                })
            })
            .map(|(id, _)| *id)
            .collect();
        drop(graph);
        for peer in done {
            r.finish_batch(&peer);
            self.next_batch(&mut r, peer);
        }
    }

    /// Requests the peer's next batch of missing channels, unless one is in flight.
    fn next_batch(&self, r: &mut RangeSync, peer: PublicKey) {
        let RangeSync { peers, queued, .. } = r;
        let Some(p) = peers.get_mut(&peer) else {
            return;
        };
        if p.in_flight.is_some() {
            return;
        }
        let graph = self.graph().read_only();
        let mut batch = Vec::new();
        while batch.len() < SCID_BATCH
            && let Some(scid) = p.wanted.pop_front()
        {
            if !complete(&graph, scid) {
                batch.push(scid);
            } else {
                queued.remove(&scid);
            }
        }
        drop(graph);
        if batch.is_empty() {
            return;
        }
        p.in_flight = Some((batch.clone(), Instant::now()));
        self.send(MessageSendEvent::SendShortIdsQuery {
            node_id: peer,
            msg: QueryShortChannelIds {
                chain_hash: self.chain,
                short_channel_ids: batch,
            },
        });
    }
}

/// Whether we hold a channel and both its nodes' announcements: nothing to ask a peer for.
fn complete(graph: &ReadOnlyNetworkGraph<'_>, scid: u64) -> bool {
    graph.channel(scid).is_some_and(|c| {
        [c.node_one, c.node_two]
            .iter()
            .all(|n| graph.node(n).is_some_and(|n| n.announcement_info.is_some()))
    })
}

fn ignore(err: &str) -> LightningError {
    LightningError {
        err: err.to_owned(),
        action: ErrorAction::IgnoreError,
    }
}

impl BaseMessageHandler for Gossip {
    fn get_and_clear_pending_msg_events(&self) -> Vec<MessageSendEvent> {
        let mut events = self.sync.get_and_clear_pending_msg_events();
        if let Ok(mut o) = self.outbox.lock() {
            events.append(&mut o);
        }
        events.retain_mut(|ev| match ev {
            MessageSendEvent::SendGossipTimestampFilter { msg, .. } => {
                // After a restart, ask only for what is newer than the saved graph.
                if let Some(from) = self.resume_from {
                    msg.first_timestamp = msg.first_timestamp.max(from);
                }
                true
            }
            // Gossip validated later (by the UTXO hook) comes back as a broadcast. We do not
            // relay, but the graph changed.
            MessageSendEvent::BroadcastChannelAnnouncement { msg, .. } => {
                self.changed(Change::Channel(msg.contents.short_channel_id));
                false
            }
            MessageSendEvent::BroadcastNodeAnnouncement { msg } => {
                self.changed(Change::Node(msg.contents.node_id));
                false
            }
            MessageSendEvent::BroadcastChannelUpdate { msg } => {
                self.changed(Change::Channel(msg.contents.short_channel_id));
                false
            }
            _ => true,
        });
        events
    }

    fn peer_disconnected(&self, their_node_id: PublicKey) {
        self.sync.peer_disconnected(their_node_id);
        // What it still owed is freed; another peer's next range reply can claim it.
        if let Ok(mut r) = self.range.lock() {
            r.finish_batch(&their_node_id);
            if let Some(p) = r.peers.remove(&their_node_id) {
                for scid in p.wanted {
                    r.queued.remove(&scid);
                }
            }
        }
    }

    fn provided_node_features(&self) -> NodeFeatures {
        self.sync.provided_node_features()
    }

    fn provided_init_features(&self, their_node_id: PublicKey) -> InitFeatures {
        self.sync.provided_init_features(their_node_id)
    }

    fn peer_connected(
        &self,
        their_node_id: PublicKey,
        msg: &Init,
        inbound: bool,
    ) -> Result<(), ()> {
        self.sync.peer_connected(their_node_id, msg, inbound)?;
        if msg.features.supports_gossip_queries()
            && let Ok(mut r) = self.range.lock()
        {
            r.peers.insert(
                their_node_id,
                PeerRange {
                    listing: Some(Instant::now()),
                    ..PeerRange::default()
                },
            );
            self.send(MessageSendEvent::SendChannelRangeQuery {
                node_id: their_node_id,
                msg: QueryChannelRange {
                    chain_hash: self.chain,
                    first_blocknum: 0,
                    number_of_blocks: u32::MAX,
                },
            });
        }
        Ok(())
    }
}

impl RoutingMessageHandler for Gossip {
    fn handle_node_announcement(
        &self,
        their_node_id: Option<PublicKey>,
        msg: &NodeAnnouncement,
    ) -> Result<bool, LightningError> {
        let bytes = msg.serialized_length();
        self.count(&self.counters.node_announcements, bytes);
        // An oversized one is kept trimmed and unsigned, so its signature is checked here.
        let trimmed = if bytes > MAX_GOSSIP_BYTES {
            if let Err(e) = verify_node_announcement(msg, &self.secp) {
                return self.tally(Err(e));
            }
            Some(
                self.trim(msg)
                    .ok_or_else(|| ignore("message over size limit"))?,
            )
        } else {
            None
        };
        if self.hold_node(msg, trimmed.as_ref()) {
            self.counters.held.fetch_add(1, Ordering::Relaxed);
            return Err(ignore("held until its stored channels are checked"));
        }
        let r = match &trimmed {
            Some(contents) => {
                let r = self
                    .graph()
                    .update_node_from_unsigned_announcement(contents);
                if r.is_ok() {
                    self.counters.trimmed.fetch_add(1, Ordering::Relaxed);
                }
                r.map(|()| false)
            }
            None => self.sync.handle_node_announcement(their_node_id, msg),
        };
        if r.is_ok() {
            self.changed(Change::Node(msg.contents.node_id));
        }
        self.tally(r)
    }

    fn handle_channel_announcement(
        &self,
        their_node_id: Option<PublicKey>,
        msg: &ChannelAnnouncement,
    ) -> Result<bool, LightningError> {
        self.admit(
            &self.counters.channel_announcements,
            msg.serialized_length(),
        )?;
        let r = self.check_announcement(their_node_id, msg);
        self.tally(r)
    }

    fn handle_channel_update(
        &self,
        their_node_id: Option<PublicKey>,
        msg: &ChannelUpdate,
    ) -> Result<bool, LightningError> {
        self.admit(&self.counters.channel_updates, msg.serialized_length())?;
        if self.hold_update(msg) {
            self.counters.held.fetch_add(1, Ordering::Relaxed);
            return Err(ignore("held until its stored channel is checked"));
        }
        let r = self.sync.handle_channel_update(their_node_id, msg);
        if r.is_ok() {
            self.changed(Change::Channel(msg.contents.short_channel_id));
        }
        self.tally(r)
    }

    // A listener does not serve the graph: no initial dump to peers, and queries get nothing.
    fn get_next_channel_announcement(
        &self,
        _starting_point: u64,
    ) -> Option<(
        ChannelAnnouncement,
        Option<ChannelUpdate>,
        Option<ChannelUpdate>,
    )> {
        None
    }

    fn get_next_node_announcement(
        &self,
        _starting_point: Option<&NodeId>,
    ) -> Option<NodeAnnouncement> {
        None
    }

    fn handle_reply_channel_range(
        &self,
        their_node_id: PublicKey,
        msg: ReplyChannelRange,
    ) -> Result<(), LightningError> {
        let Ok(mut r) = self.range.lock() else {
            return Ok(());
        };
        let RangeSync {
            peers,
            queued,
            asked,
        } = &mut *r;
        let Some(p) = peers.get_mut(&their_node_id) else {
            return Err(ignore("unsolicited reply_channel_range"));
        };
        if msg.chain_hash != self.chain {
            return Err(ignore("reply_channel_range for another chain"));
        }
        // `sync_complete` is not an end marker: the last reply is the one reaching the end of
        // the queried range, which was every block.
        let end = u64::from(msg.first_blocknum) + u64::from(msg.number_of_blocks);
        p.listing = (end < u64::from(u32::MAX)).then(Instant::now);
        let graph = self.graph().read_only();
        for scid in msg.short_channel_ids {
            if queued.len() >= self.max_channels {
                break;
            }
            let want = match graph.channel(scid) {
                // Unless it is being checked: stored, or waiting for its lookup.
                None => {
                    !self.lookup.holds(scid)
                        && !self
                            .replay
                            .lock()
                            .is_ok_and(|r| r.channels.contains_key(&scid))
                }
                Some(c) => {
                    let mut new = false;
                    for n in [c.node_one, c.node_two] {
                        let announced = graph
                            .node(&n)
                            .is_some_and(|n| n.announcement_info.is_some());
                        new |= !announced && asked.insert(n);
                    }
                    new
                }
            };
            if want && queued.insert(scid) {
                p.wanted.push_back(scid);
            }
        }
        drop(graph);
        self.next_batch(&mut r, their_node_id);
        Ok(())
    }

    fn handle_reply_short_channel_ids_end(
        &self,
        their_node_id: PublicKey,
        _msg: ReplyShortChannelIdsEnd,
    ) -> Result<(), LightningError> {
        let Ok(mut r) = self.range.lock() else {
            return Ok(());
        };
        if r.peers.contains_key(&their_node_id) {
            r.finish_batch(&their_node_id);
            self.next_batch(&mut r, their_node_id);
        }
        Ok(())
    }

    /// The spec requires a reply; ours says we have nothing.
    fn handle_query_channel_range(
        &self,
        their_node_id: PublicKey,
        msg: QueryChannelRange,
    ) -> Result<(), LightningError> {
        self.send(MessageSendEvent::SendReplyChannelRange {
            node_id: their_node_id,
            msg: ReplyChannelRange {
                chain_hash: msg.chain_hash,
                first_blocknum: msg.first_blocknum,
                number_of_blocks: msg.number_of_blocks,
                sync_complete: true,
                short_channel_ids: Vec::new(),
            },
        });
        Ok(())
    }

    // LDK has no message event for the reply; like LDK's own handler, we do not answer.
    fn handle_query_short_channel_ids(
        &self,
        _their_node_id: PublicKey,
        _msg: QueryShortChannelIds,
    ) -> Result<(), LightningError> {
        Err(ignore("queries are not served"))
    }

    /// Ours, not LDK's: LDK counts at most 32 lookups in flight, too few to batch. While this is
    /// true LDK holds each peer's next gossip message and stops reading from it.
    fn processing_queue_high(&self) -> bool {
        self.lookup.queue_high()
    }
}
