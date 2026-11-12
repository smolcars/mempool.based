//! The routing-message handler the peer manager talks to: LDK's `P2PGossipSync` behind a
//! listener-only front that counts, bounds and reports what arrives, and never serves or relays.
//!
//! LDK asks each peer for gossip with a timestamp filter only. Eclair, and LND nodes that ignore
//! historical filters, answer that with new gossip only. So every peer that supports gossip
//! queries is also asked for every channel id it knows (`query_channel_range`), and the channels
//! missing here are requested from it in batches (`query_short_channel_ids`), one batch at a time
//! per peer and never the same channel from two peers at once. Peers' graphs differ (on mainnet
//! eclair listed 21,600 channels and LND 30,250), so asking every peer gives the union.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use lightning::bitcoin::Network;
use lightning::bitcoin::constants::ChainHash;
use lightning::bitcoin::secp256k1::{PublicKey, Secp256k1, VerifyOnly};
use lightning::ln::msgs::{
    BaseMessageHandler, ChannelAnnouncement, ChannelUpdate, ErrorAction, Init, LightningError,
    MessageSendEvent, NodeAnnouncement, QueryChannelRange, QueryShortChannelIds, ReplyChannelRange,
    ReplyShortChannelIdsEnd, RoutingMessageHandler,
};
use lightning::routing::gossip::{NetworkGraph, NodeId, P2PGossipSync, verify_node_announcement};
use lightning::routing::utxo::UtxoLookup;
use lightning::types::features::{InitFeatures, NodeFeatures};
use lightning::util::ser::Writeable;
use tokio::sync::broadcast;

use crate::log::LogSink;
use crate::{Change, Event};

/// The funding-output check LDK runs for each channel announcement. Unset in this crate: see
/// [`crate::Validation`]. The chain-validation task passes one to [`P2PGossipSync::add_utxo_lookup`].
pub(crate) type UtxoHook = Arc<dyn UtxoLookup + Send + Sync>;
pub(crate) type Graph = NetworkGraph<Arc<LogSink>>;
type GossipSync = P2PGossipSync<Arc<Graph>, UtxoHook, Arc<LogSink>>;

/// Largest gossip message kept, in bytes as encoded. Real announcements are under 500 bytes; the
/// wire allows 65 KB of features, addresses and extra data, all of which LDK would keep.
pub(crate) const MAX_GOSSIP_BYTES: usize = 1024;
/// Addresses kept per node. Some real nodes announce one onion address hundreds of times.
pub(crate) const MAX_NODE_ADDRESSES: usize = 8;

/// Channel ids per `query_short_channel_ids`: 16 KB of ids, well inside a 65 KB message.
const SCID_BATCH: usize = 2000;
/// A batch with no answer for this long is given up; its channels return at the next range sync.
const BATCH_TIMEOUT: Duration = Duration::from_secs(120);

/// The channel-range sync with every connected peer.
#[derive(Default)]
struct RangeSync {
    peers: HashMap<PublicKey, PeerRange>,
    /// Channel ids wanted from or requested of some peer. Bounded by the graph ceiling.
    queued: HashSet<u64>,
}

#[derive(Default)]
struct PeerRange {
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

/// Totals since start. Read by the supervisor to compute rates.
#[derive(Default)]
pub(crate) struct Counters {
    pub(crate) channel_announcements: AtomicU64,
    pub(crate) node_announcements: AtomicU64,
    pub(crate) channel_updates: AtomicU64,
    pub(crate) bytes: AtomicU64,
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
    /// Messages of our own for the peer manager to send.
    outbox: Mutex<Vec<MessageSendEvent>>,
    changes: broadcast::Sender<Change>,
    events: broadcast::Sender<Event>,
}

impl Gossip {
    pub(crate) fn new(
        network: Network,
        graph: Arc<Graph>,
        logger: Arc<LogSink>,
        max_channels: usize,
        resume_from: Option<u32>,
        changes: broadcast::Sender<Change>,
        events: broadcast::Sender<Event>,
    ) -> Self {
        Self {
            // The UTXO hook is left unset: the graph is unverified (crate::Validation).
            sync: P2PGossipSync::new(graph, None, logger),
            chain: ChainHash::using_genesis_block(network),
            secp: Secp256k1::verification_only(),
            max_channels,
            resume_from,
            counters: Counters::default(),
            at_ceiling: AtomicBool::new(false),
            last_new_channel: Mutex::new(Instant::now()),
            range: Mutex::new(RangeSync::default()),
            outbox: Mutex::new(Vec::new()),
            changes,
            events,
        }
    }

    pub(crate) fn graph(&self) -> &Arc<Graph> {
        self.sync.network_graph()
    }

    /// Clears the ceiling flag once the graph is back under it, so the next hit is reported.
    pub(crate) fn recheck_ceiling(&self) {
        if self.graph().read_only().channels().len() < self.max_channels {
            self.at_ceiling.store(false, Ordering::Relaxed);
        }
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
            Err(_) => c.rejected.fetch_add(1, Ordering::Relaxed),
        };
        r.map(|_| false)
    }

    /// An oversized node announcement whose signature holds is kept without its excess: at most
    /// [`MAX_NODE_ADDRESSES`] distinct addresses and no unknown fields. Stored unsigned, which
    /// only matters for relaying, and we do not relay.
    /// `None` when it is still too big.
    fn trim(&self, msg: &NodeAnnouncement) -> Option<Result<bool, LightningError>> {
        if let Err(e) = verify_node_announcement(msg, &self.secp) {
            return Some(Err(e));
        }
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
        let r = self
            .graph()
            .update_node_from_unsigned_announcement(&contents);
        if r.is_ok() {
            self.counters.trimmed.fetch_add(1, Ordering::Relaxed);
        }
        Some(r.map(|()| false))
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
    pub(crate) fn range_pending(&self) -> usize {
        self.range.lock().map_or(0, |r| r.queued.len())
    }

    /// Moves on from batches that fully arrived without their end marker, or timed out.
    pub(crate) fn range_tick(&self) {
        let Ok(mut r) = self.range.lock() else { return };
        let graph = self.graph().read_only();
        let done: Vec<PublicKey> = r
            .peers
            .iter()
            .filter(|(_, p)| {
                p.in_flight.as_ref().is_some_and(|(batch, sent)| {
                    sent.elapsed() >= BATCH_TIMEOUT
                        || batch.iter().all(|scid| graph.channel(*scid).is_some())
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
        let RangeSync { peers, queued } = r;
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
            if graph.channel(scid).is_none() {
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
            r.peers.insert(their_node_id, PeerRange::default());
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
        let r = if bytes > MAX_GOSSIP_BYTES {
            self.trim(msg)
                .ok_or_else(|| ignore("message over size limit"))?
        } else {
            self.sync.handle_node_announcement(their_node_id, msg)
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
        let scid = msg.contents.short_channel_id;
        let graph = self.graph().read_only();
        let (known, len) = (graph.channel(scid).is_some(), graph.channels().len());
        drop(graph);
        // Nodes enter the graph only through channels, so this also bounds nodes at twice as many.
        if !known && len >= self.max_channels {
            self.counters.over_ceiling.fetch_add(1, Ordering::Relaxed);
            if !self.at_ceiling.swap(true, Ordering::Relaxed) {
                let _ = self.events.send(Event::CeilingReached { channels: len });
            }
            return Err(ignore("graph at its ceiling"));
        }
        let r = self.sync.handle_channel_announcement(their_node_id, msg);
        if r.is_ok() {
            if !known && let Ok(mut t) = self.last_new_channel.lock() {
                *t = Instant::now();
            }
            self.changed(Change::Channel(scid));
            self.changed(Change::Node(msg.contents.node_id_1));
            self.changed(Change::Node(msg.contents.node_id_2));
        }
        self.tally(r)
    }

    fn handle_channel_update(
        &self,
        their_node_id: Option<PublicKey>,
        msg: &ChannelUpdate,
    ) -> Result<bool, LightningError> {
        self.admit(&self.counters.channel_updates, msg.serialized_length())?;
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
        let RangeSync { peers, queued } = &mut *r;
        let Some(p) = peers.get_mut(&their_node_id) else {
            return Err(ignore("unsolicited reply_channel_range"));
        };
        if msg.chain_hash != self.chain {
            return Err(ignore("reply_channel_range for another chain"));
        }
        let graph = self.graph().read_only();
        for scid in msg.short_channel_ids {
            if queued.len() >= self.max_channels {
                break;
            }
            if graph.channel(scid).is_none() && queued.insert(scid) {
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

    fn processing_queue_high(&self) -> bool {
        self.sync.processing_queue_high()
    }
}
