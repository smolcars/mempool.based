#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::HashMap;
use std::net::TcpListener as StdListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lightning::bitcoin::constants::ChainHash;
use lightning::bitcoin::hashes::Hash;
use lightning::bitcoin::hashes::sha256d;
use lightning::bitcoin::secp256k1::{Message, Secp256k1, SecretKey};
use lightning::bitcoin::{BlockHash, ScriptBuf};
use lightning::ln::chan_utils::make_funding_redeemscript;
use lightning::ln::msgs::{
    BaseMessageHandler, ChannelAnnouncement, ChannelUpdate, CommonOpenChannelFields, Init,
    LightningError, MessageSendEvent, NodeAnnouncement, OpenChannel, QueryChannelRange,
    QueryShortChannelIds, ReplyChannelRange, ReplyShortChannelIdsEnd, RoutingMessageHandler,
    UnsignedChannelAnnouncement, UnsignedChannelUpdate, UnsignedNodeAnnouncement,
};
use lightning::ln::peer_handler::PeerManager;
use lightning::ln::types::ChannelId;
use lightning::routing::gossip::{NetworkGraph, P2PGossipSync};
use lightning::sign::KeysManager;
use lightning::types::features::{ChannelFeatures, InitFeatures, NodeFeatures};
use lightning::util::logger::{Logger, Record};
use lightning::util::ser::Writeable;
use node::esplora::model::{PositionedOutput, ScriptType, TxStatus, Vout};
use node::esplora::{ChainTip, Error as NodeError, OutPosition, OutPositions, Stamped};
use tokio::sync::broadcast;

use crate::chain::{self, TRUSTED_SATS, position};
use crate::gossip::{Gossip, Graph, MAX_GOSSIP_BYTES, Replay, UtxoHook};
use crate::log::LogSink;
use crate::store::{self, GraphLoad};
use crate::*;

const NET: Network = Network::Regtest;

fn chain() -> ChainHash {
    ChainHash::using_genesis_block(NET)
}

fn key(n: u8) -> SecretKey {
    SecretKey::from_slice(&[n; 32]).unwrap()
}

fn id(k: &SecretKey) -> NodeId {
    NodeId::from_pubkey(&k.public_key(&Secp256k1::new()))
}

fn sign<T: Writeable>(
    contents: &T,
    k: &SecretKey,
) -> lightning::bitcoin::secp256k1::ecdsa::Signature {
    let hash = sha256d::Hash::hash(&contents.encode());
    Secp256k1::new().sign_ecdsa(&Message::from_digest(hash.to_byte_array()), k)
}

fn now() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as u32
}

/// A signed announcement of channel `scid` between nodes `a` and `b` (node keys a, b; funding
/// keys a+100, b+100).
fn channel(scid: u64, a: u8, b: u8) -> ChannelAnnouncement {
    channel_on(chain(), scid, a, b)
}

fn channel_on(chain: ChainHash, scid: u64, a: u8, b: u8) -> ChannelAnnouncement {
    let (mut k1, mut k2) = (key(a), key(b));
    if id(&k1) > id(&k2) {
        std::mem::swap(&mut k1, &mut k2);
    }
    let (f1, f2) = (key(a + 100), key(b + 100));
    let contents = UnsignedChannelAnnouncement {
        features: ChannelFeatures::empty(),
        chain_hash: chain,
        short_channel_id: scid,
        node_id_1: id(&k1),
        node_id_2: id(&k2),
        bitcoin_key_1: id(&f1),
        bitcoin_key_2: id(&f2),
        excess_data: Vec::new(),
    };
    ChannelAnnouncement {
        node_signature_1: sign(&contents, &k1),
        node_signature_2: sign(&contents, &k2),
        bitcoin_signature_1: sign(&contents, &f1),
        bitcoin_signature_2: sign(&contents, &f2),
        contents,
    }
}

fn node(n: u8, alias: &str, excess: usize) -> NodeAnnouncement {
    let mut a = [0u8; 32];
    a[..alias.len()].copy_from_slice(alias.as_bytes());
    let contents = UnsignedNodeAnnouncement {
        features: NodeFeatures::empty(),
        timestamp: now(),
        node_id: id(&key(n)),
        rgb: [1, 2, 3],
        alias: NodeAlias(a),
        addresses: vec![SocketAddress::TcpIpV4 {
            addr: [127, 0, 0, 1],
            port: 9735,
        }],
        excess_address_data: Vec::new(),
        excess_data: vec![0; excess],
    };
    NodeAnnouncement {
        signature: sign(&contents, &key(n)),
        contents,
    }
}

fn update(scid: u64, signer: u8, direction: u8) -> ChannelUpdate {
    let contents = UnsignedChannelUpdate {
        chain_hash: chain(),
        short_channel_id: scid,
        timestamp: now(),
        message_flags: 1,
        channel_flags: direction,
        cltv_expiry_delta: 144,
        htlc_minimum_msat: 1000,
        htlc_maximum_msat: 1_000_000_000,
        fee_base_msat: 1000,
        fee_proportional_millionths: 100,
        excess_data: Vec::new(),
    };
    ChannelUpdate {
        signature: sign(&contents, &key(signer)),
        contents,
    }
}

struct Handler {
    gossip: Gossip,
    events: broadcast::Receiver<Event>,
    changes: broadcast::Receiver<Change>,
}

/// A handler whose funding lookups all succeed at once, for tests of everything but validation.
fn handler(max_channels: usize) -> Handler {
    let mut config = Config::new(NET, PathBuf::new());
    config.max_channels = max_channels;
    let (gossip, events, changes) = gossip(&config, Replay::default());
    gossip.lookup.trust.store(true, Ordering::Relaxed);
    Handler {
        gossip: Arc::into_inner(gossip).unwrap(),
        events,
        changes,
    }
}

fn gossip(
    config: &Config,
    replay: Replay,
) -> (
    Arc<Gossip>,
    broadcast::Receiver<Event>,
    broadcast::Receiver<Change>,
) {
    let events = broadcast::channel(4096).0;
    let changes = broadcast::channel(4096).0;
    let logger = Arc::new(LogSink::new(Level::Info, 20, events.clone()));
    let graph = Arc::new(Graph::new(NET, Arc::clone(&logger)));
    let g = Gossip::new(
        config,
        graph,
        logger,
        None,
        replay,
        changes.clone(),
        events.clone(),
    );
    (Arc::new(g), events.subscribe(), changes.subscribe())
}

#[test]
fn a_valid_announcement_enters_the_graph_and_is_not_relayed() {
    let mut h = handler(10);
    let ann = channel(1, 1, 2);
    // Ok(false): accepted, but the peer manager must not forward it.
    assert!(matches!(
        h.gossip.handle_channel_announcement(None, &ann),
        Ok(false)
    ));
    assert_eq!(h.changes.try_recv().unwrap(), Change::Channel(1));
    assert!(matches!(
        h.gossip
            .handle_node_announcement(None, &node(1, "alpha", 0)),
        Ok(false)
    ));
    let dir = if id(&key(1)) == ann.contents.node_id_1 {
        0
    } else {
        1
    };
    assert!(matches!(
        h.gossip.handle_channel_update(None, &update(1, 1, dir)),
        Ok(false)
    ));

    let g = h.gossip.graph().read_only();
    let c = g.channel(1).unwrap();
    assert_eq!(c.capacity_sats, Some(TRUSTED_SATS));
    let policy = if dir == 0 {
        &c.one_to_two
    } else {
        &c.two_to_one
    };
    assert_eq!(policy.as_ref().unwrap().fees.proportional_millionths, 100);
    let info = g
        .node(&id(&key(1)))
        .unwrap()
        .announcement_info
        .as_ref()
        .unwrap();
    assert_eq!(&info.alias().0[..5], b"alpha");
    assert_eq!(info.rgb(), [1, 2, 3]);
    assert_eq!(g.nodes().len(), 2);
}

#[test]
fn an_invalid_signature_is_rejected() {
    let h = handler(10);
    let mut ann = channel(1, 1, 2);
    ann.node_signature_1 = sign(&ann.contents, &key(9));
    assert!(h.gossip.handle_channel_announcement(None, &ann).is_err());
    let mut n = node(1, "x", 0);
    n.contents.rgb = [9, 9, 9];
    assert!(h.gossip.handle_node_announcement(None, &n).is_err());
    assert_eq!(h.gossip.graph().read_only().channels().len(), 0);
    assert_eq!(h.gossip.counters.rejected.load(Ordering::Relaxed), 2);
}

#[test]
fn the_ceiling_holds_and_is_reported_once() {
    let mut h = handler(1);
    assert!(
        h.gossip
            .handle_channel_announcement(None, &channel(1, 1, 2))
            .is_ok()
    );
    assert!(
        h.gossip
            .handle_channel_announcement(None, &channel(2, 3, 4))
            .is_err()
    );
    assert!(
        h.gossip
            .handle_channel_announcement(None, &channel(3, 5, 6))
            .is_err()
    );
    // A channel already held is not new; it is a duplicate, not a ceiling refusal.
    assert!(
        h.gossip
            .handle_channel_announcement(None, &channel(1, 1, 2))
            .is_err()
    );
    assert_eq!(h.gossip.graph().read_only().channels().len(), 1);
    assert_eq!(h.gossip.counters.over_ceiling.load(Ordering::Relaxed), 2);
    assert_eq!(h.gossip.counters.duplicate.load(Ordering::Relaxed), 1);
    assert!(h.gossip.at_ceiling());
    let mut ceilings = 0;
    while let Ok(e) = h.events.try_recv() {
        if matches!(e, Event::CeilingReached { channels: 1 }) {
            ceilings += 1;
        }
    }
    assert_eq!(ceilings, 1);
}

#[test]
fn oversized_node_announcements_are_trimmed_or_refused() {
    let h = handler(10);
    h.gossip
        .handle_channel_announcement(None, &channel(1, 1, 2))
        .unwrap();
    let onion = SocketAddress::OnionV3 {
        ed25519_pubkey: [7; 32],
        checksum: 1,
        version: 3,
        port: 9735,
    };
    // As seen on mainnet: one onion address announced hundreds of times, plus junk extra data.
    let resign = |mut n: NodeAnnouncement, k: u8| {
        n.signature = sign(&n.contents, &key(k));
        n
    };
    let mut n = node(1, "dup", 2000);
    n.contents.addresses = vec![onion.clone(); 1000];
    for i in 0..20u8 {
        n.contents.addresses.push(SocketAddress::TcpIpV4 {
            addr: [1, 1, 1, i],
            port: 9735,
        });
    }
    let n = resign(n, 1);
    assert!(n.serialized_length() > MAX_GOSSIP_BYTES);
    assert!(matches!(
        h.gossip.handle_node_announcement(None, &n),
        Ok(false)
    ));
    {
        let g = h.gossip.graph().read_only();
        let info = g
            .node(&id(&key(1)))
            .unwrap()
            .announcement_info
            .clone()
            .unwrap();
        assert_eq!(&info.alias().0[..3], b"dup");
        assert_eq!(info.addresses().len(), crate::gossip::MAX_NODE_ADDRESSES);
        assert_eq!(info.addresses()[0], onion);
        assert!(
            info.announcement_message().is_none(),
            "kept unsigned, never relayed"
        );
    }
    assert_eq!(h.gossip.counters.trimmed.load(Ordering::Relaxed), 1);

    // A forged oversized announcement is still checked.
    let mut forged = node(2, "forged", 2000);
    forged.contents.timestamp += 1;
    let forged = resign(forged, 9);
    assert!(h.gossip.handle_node_announcement(None, &forged).is_err());
    // Features cannot be trimmed: too big after trimming is refused.
    let mut fat = node(2, "fat", 0);
    fat.contents.features = NodeFeatures::from_le_bytes(vec![0; 2000]);
    let fat = resign(fat, 2);
    assert!(h.gossip.handle_node_announcement(None, &fat).is_err());
    assert_eq!(h.gossip.counters.oversize.load(Ordering::Relaxed), 1);
    let g = h.gossip.graph().read_only();
    assert!(g.node(&id(&key(2))).unwrap().announcement_info.is_none());
}

#[test]
fn the_graph_is_never_served() {
    let h = handler(10);
    h.gossip
        .handle_channel_announcement(None, &channel(1, 1, 2))
        .unwrap();
    h.gossip
        .handle_node_announcement(None, &node(1, "a", 0))
        .unwrap();
    assert!(h.gossip.get_next_channel_announcement(0).is_none());
    assert!(h.gossip.get_next_node_announcement(None).is_none());
    let peer = key(7).public_key(&Secp256k1::new());
    let q = QueryChannelRange {
        chain_hash: chain(),
        first_blocknum: 0,
        number_of_blocks: u32::MAX,
    };
    // Answered, as the spec requires, with no channels.
    assert!(h.gossip.handle_query_channel_range(peer, q).is_ok());
    let replies = h.gossip.get_and_clear_pending_msg_events();
    assert!(
        matches!(&replies[..], [MessageSendEvent::SendReplyChannelRange { msg, .. }]
        if msg.short_channel_ids.is_empty() && msg.sync_complete)
    );
    let q = QueryShortChannelIds {
        chain_hash: chain(),
        short_channel_ids: vec![1],
    };
    assert!(h.gossip.handle_query_short_channel_ids(peer, q).is_err());
}

#[test]
fn missing_channels_are_requested_from_every_peer_without_overlap() {
    let h = handler(5);
    h.gossip
        .handle_channel_announcement(None, &channel(1, 1, 2))
        .unwrap();
    // Channel 1 and both its nodes' announcements are held: nothing to ask for.
    h.gossip
        .handle_node_announcement(None, &node(1, "a", 0))
        .unwrap();
    h.gossip
        .handle_node_announcement(None, &node(2, "b", 0))
        .unwrap();
    h.gossip.get_and_clear_pending_msg_events();
    let pk = |n| key(n).public_key(&Secp256k1::new());
    let (a, b, c) = (pk(7), pk(8), pk(9));
    let mut features = InitFeatures::empty();
    features.set_gossip_queries_optional();
    let init = Init {
        features,
        networks: None,
        remote_network_address: None,
    };
    h.gossip.peer_connected(a, &init, false).unwrap();
    h.gossip.peer_connected(b, &init, false).unwrap();
    let mut asked: Vec<_> = h
        .gossip
        .get_and_clear_pending_msg_events()
        .into_iter()
        .filter_map(|e| match e {
            MessageSendEvent::SendChannelRangeQuery { node_id, .. } => Some(node_id),
            _ => None,
        })
        .collect();
    asked.sort();
    let mut both = vec![a, b];
    both.sort();
    assert_eq!(asked, both, "every peer is asked for its channel list");
    assert_eq!(h.gossip.range_pending(), 2, "two channel lists outstanding");

    let reply = |scids: Vec<u64>| ReplyChannelRange {
        chain_hash: chain(),
        first_blocknum: 0,
        number_of_blocks: u32::MAX,
        sync_complete: true,
        short_channel_ids: scids,
    };
    let queries = |h: &Handler| -> Vec<(PublicKey, Vec<u64>)> {
        h.gossip
            .get_and_clear_pending_msg_events()
            .into_iter()
            .filter_map(|e| match e {
                MessageSendEvent::SendShortIdsQuery { node_id, msg } => {
                    Some((node_id, msg.short_channel_ids))
                }
                _ => None,
            })
            .collect()
    };
    let end = || ReplyShortChannelIdsEnd {
        chain_hash: chain(),
        full_information: true,
    };

    assert!(
        h.gossip
            .handle_reply_channel_range(c, reply(vec![9]))
            .is_err(),
        "unsolicited"
    );
    // A first fragment that stops short of the queried range leaves a's list outstanding.
    let mut part = reply(vec![]);
    part.number_of_blocks = 100;
    h.gossip.handle_reply_channel_range(a, part).unwrap();
    assert_eq!(h.gossip.range_pending(), 2);
    // Channel 1 is complete.
    h.gossip
        .handle_reply_channel_range(a, reply(vec![1, 2, 3, 4]))
        .unwrap();
    assert_eq!(queries(&h), [(a, vec![2, 3, 4])]);
    // b's list overlaps a's: only what nobody was asked for, up to the ceiling of 5 for b
    // alone, so a list of invented ids cannot crowd out another peer's.
    h.gossip
        .handle_reply_channel_range(b, reply((3..=11).collect()))
        .unwrap();
    assert_eq!(queries(&h), [(b, vec![5, 6, 7, 8, 9])]);
    assert_eq!(h.gossip.range_pending(), 8);
    // One batch at a time per peer.
    h.gossip
        .handle_reply_short_channel_ids_end(a, end())
        .unwrap();
    assert_eq!(h.gossip.range_pending(), 5);
    // b's list has ended: more of it is refused.
    assert!(
        h.gossip
            .handle_reply_channel_range(b, reply(vec![12]))
            .is_err()
    );
    assert!(queries(&h).is_empty());
    // A peer that leaves frees what it owed.
    h.gossip.peer_disconnected(b);
    assert_eq!(h.gossip.range_pending(), 0);
}

#[test]
fn a_held_channel_with_unannounced_nodes_is_requested_again() {
    let h = handler(5);
    h.gossip
        .handle_channel_announcement(None, &channel(1, 1, 2))
        .unwrap();
    h.gossip
        .handle_node_announcement(None, &node(1, "a", 0))
        .unwrap();
    let a = key(7).public_key(&Secp256k1::new());
    let mut features = InitFeatures::empty();
    features.set_gossip_queries_optional();
    let init = Init {
        features,
        networks: None,
        remote_network_address: None,
    };
    h.gossip.peer_connected(a, &init, false).unwrap();
    let reply = ReplyChannelRange {
        chain_hash: chain(),
        first_blocknum: 0,
        number_of_blocks: u32::MAX,
        sync_complete: true,
        short_channel_ids: vec![1],
    };
    h.gossip.handle_reply_channel_range(a, reply).unwrap();
    assert!(
        h.gossip
            .get_and_clear_pending_msg_events()
            .iter()
            .any(|e| matches!(e,
        MessageSendEvent::SendShortIdsQuery { msg, .. } if msg.short_channel_ids == [1]))
    );
    // Listed again by another peer, it is not asked for twice: node 2 may never announce.
    let b = key(8).public_key(&Secp256k1::new());
    h.gossip.peer_connected(b, &init, false).unwrap();
    let reply = ReplyChannelRange {
        chain_hash: chain(),
        first_blocknum: 0,
        number_of_blocks: u32::MAX,
        sync_complete: true,
        short_channel_ids: vec![1],
    };
    h.gossip.handle_reply_channel_range(b, reply).unwrap();
    assert!(
        !h.gossip
            .get_and_clear_pending_msg_events()
            .iter()
            .any(|e| matches!(e,
        MessageSendEvent::SendShortIdsQuery { node_id, .. } if *node_id == b))
    );
    // Node 2's announcement arrives with the answer; the batch is then complete without an end.
    h.gossip
        .handle_node_announcement(None, &node(2, "b", 0))
        .unwrap();
    h.gossip.range_tick();
    assert_eq!(h.gossip.range_pending(), 0);
}

#[test]
fn nodes_asked_about_are_forgotten_when_pruned() {
    let h = handler(5);
    let a = key(7).public_key(&Secp256k1::new());
    let mut features = InitFeatures::empty();
    features.set_gossip_queries_optional();
    let init = Init {
        features,
        networks: None,
        remote_network_address: None,
    };
    let reply = || ReplyChannelRange {
        chain_hash: chain(),
        first_blocknum: 0,
        number_of_blocks: u32::MAX,
        sync_complete: true,
        short_channel_ids: vec![1],
    };
    let asked_for_1 = |h: &Handler| {
        h.gossip.get_and_clear_pending_msg_events().iter().any(|e| {
            matches!(e,
            MessageSendEvent::SendShortIdsQuery { msg, .. } if msg.short_channel_ids == [1])
        })
    };
    h.gossip
        .handle_channel_announcement(None, &channel(1, 1, 2))
        .unwrap();
    h.gossip.peer_connected(a, &init, false).unwrap();
    h.gossip.handle_reply_channel_range(a, reply()).unwrap();
    assert!(asked_for_1(&h));

    assert_eq!(h.gossip.asked_len(), 2);

    // The channel and its unannounced nodes age out; the nodes are forgotten with them, so a
    // peer cycling invented channels cannot grow the set.
    let p = crate::peer::prune_graph(h.gossip.graph(), u64::from(now()) + 15 * 86400);
    assert_eq!(p.gone_nodes, 2);
    h.gossip.after_prune();
    assert_eq!(h.gossip.asked_len(), 0);
}

#[test]
fn ldk_records_are_bounded_and_rate_limited() {
    let events = broadcast::channel(64).0;
    let mut rx = events.subscribe();
    let sink = LogSink::new(Level::Info, 2, events);
    let long = "é".repeat(1000);
    for level in [Level::Debug, Level::Warn, Level::Warn, Level::Error] {
        sink.log(Record::new(
            level,
            None,
            None,
            format_args!("{long}"),
            "m",
            "f",
            1,
            None,
        ));
    }
    let Event::Ldk(r) = rx.try_recv().unwrap() else {
        panic!()
    };
    assert_eq!(r.level, Level::Warn);
    assert!(r.message.len() <= 512 && r.message.starts_with('é'));
    assert!(matches!(rx.try_recv(), Ok(Event::Ldk(_))));
    assert!(rx.try_recv().is_err(), "the third record is over the limit");
    std::thread::sleep(Duration::from_millis(1100));
    sink.flush();
    assert!(matches!(
        rx.try_recv(),
        Ok(Event::LdkSuppressed { count: 1 })
    ));
}

#[test]
fn a_prune_reports_dropped_policies_channels_and_nodes() {
    let h = handler(10);
    let ann = channel(1, 1, 2);
    h.gossip.handle_channel_announcement(None, &ann).unwrap();
    h.gossip
        .handle_channel_announcement(None, &channel(2, 2, 3))
        .unwrap();
    let dir_of = |k: u8| u8::from(id(&key(k)) != ann.contents.node_id_1);
    let mut old = update(1, 1, dir_of(1));
    old.contents.timestamp = now() - 13 * 86400;
    old.signature = sign(&old.contents, &key(1));
    h.gossip.handle_channel_update(None, &old).unwrap();
    h.gossip
        .handle_channel_update(None, &update(1, 2, dir_of(2)))
        .unwrap();
    let day = 86400;

    // Two days on, channel 1's older policy is stale; the channel and channel 2 stay.
    let p = crate::peer::prune_graph(h.gossip.graph(), u64::from(now()) + 2 * day);
    assert_eq!(p.channels, [1]);
    assert_eq!((p.gone_channels, p.gone_nodes), (0, 0));
    assert!(h.gossip.graph().read_only().channel(1).is_some());

    // Fifteen days on, both channels go, and every node with them.
    let p = crate::peer::prune_graph(h.gossip.graph(), u64::from(now()) + 15 * day);
    let mut c = p.channels.clone();
    c.sort();
    assert_eq!(c, [1, 2]);
    assert_eq!((p.gone_channels, p.gone_nodes, p.nodes.len()), (2, 3, 3));
    assert_eq!(h.gossip.graph().read_only().nodes().len(), 0);
}

// Persistence

static DIRS: AtomicU32 = AtomicU32::new(0);

/// A fresh directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let n = DIRS.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("ln-test-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn sink() -> Arc<LogSink> {
    Arc::new(LogSink::new(Level::Info, 20, broadcast::channel(8).0))
}

#[test]
fn a_saved_graph_reloads_equal() {
    let dir = TempDir::new();
    let h = handler(10);
    h.gossip
        .handle_channel_announcement(None, &channel(1, 1, 2))
        .unwrap();
    h.gossip
        .handle_node_announcement(None, &node(1, "alpha", 0))
        .unwrap();
    let path = dir.0.join(store::GRAPH_FILE);
    let bytes = h.gossip.graph().encode();
    store::write_atomic(&path, |f| std::io::Write::write_all(f, &bytes)).unwrap();

    let (g, load) = store::load_graph(&dir.0, NET, 10, &sink());
    assert!(matches!(
        load,
        GraphLoad::Loaded {
            nodes: 2,
            channels: 1,
            ..
        }
    ));
    assert!(g.unwrap() == **h.gossip.graph());

    let (g, load) = store::load_graph(&dir.0, Network::Bitcoin, 10, &sink());
    assert!(g.is_none());
    assert!(matches!(load, GraphLoad::Unreadable { .. }));

    // Saved under a higher ceiling than now configured.
    assert!(matches!(
        store::load_graph(&dir.0, NET, 0, &sink()),
        (None, GraphLoad::Unreadable { .. })
    ));

    std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
    assert!(matches!(
        store::load_graph(&dir.0, NET, 10, &sink()).1,
        GraphLoad::Unreadable { .. }
    ));
    std::fs::remove_file(&path).unwrap();
    assert!(matches!(
        store::load_graph(&dir.0, NET, 10, &sink()).1,
        GraphLoad::Fresh
    ));
}

#[test]
fn a_graph_of_the_largest_accepted_gossip_reloads() {
    let dir = TempDir::new();
    let h = handler(1);
    // Pad each message to the size limit with extra data, which LDK keeps, and sign it.
    let fill = |len: usize| MAX_GOSSIP_BYTES - len;
    let mut ann = channel(1, 1, 2);
    ann.contents.excess_data = vec![0; fill(ann.serialized_length())];
    let (k1, k2) = if id(&key(1)) == ann.contents.node_id_1 {
        (1, 2)
    } else {
        (2, 1)
    };
    ann.node_signature_1 = sign(&ann.contents, &key(k1));
    ann.node_signature_2 = sign(&ann.contents, &key(k2));
    // `channel` keeps the funding keys in argument order.
    ann.bitcoin_signature_1 = sign(&ann.contents, &key(101));
    ann.bitcoin_signature_2 = sign(&ann.contents, &key(102));
    assert_eq!(ann.serialized_length(), MAX_GOSSIP_BYTES);
    h.gossip.handle_channel_announcement(None, &ann).unwrap();
    for (signer, direction) in [(k1, 0), (k2, 1)] {
        let mut u = update(1, signer, direction);
        u.contents.excess_data = vec![0; fill(u.serialized_length())];
        u.signature = sign(&u.contents, &key(signer));
        h.gossip.handle_channel_update(None, &u).unwrap();
        let mut n = node(signer, "n", 0);
        n.contents.excess_data = vec![0; fill(n.serialized_length())];
        n.signature = sign(&n.contents, &key(signer));
        assert_eq!(n.serialized_length(), MAX_GOSSIP_BYTES);
        h.gossip.handle_node_announcement(None, &n).unwrap();
    }
    let bytes = h.gossip.graph().encode();
    assert!(
        bytes.len() as u64 <= store::MAX_FILE_BYTES_PER_CHANNEL,
        "{}",
        bytes.len()
    );
    let path = dir.0.join(store::GRAPH_FILE);
    store::write_atomic(&path, |f| std::io::Write::write_all(f, &bytes)).unwrap();
    let (_, load) = store::load_graph(&dir.0, NET, 1, &sink());
    assert!(
        matches!(load, GraphLoad::Loaded { channels: 1, .. }),
        "{load:?}"
    );
}

#[test]
fn an_interrupted_write_leaves_the_previous_file() {
    let dir = TempDir::new();
    let path = dir.0.join("f");
    store::write_atomic(&path, |f| std::io::Write::write_all(f, b"old")).unwrap();
    let r = store::write_atomic(&path, |f| {
        std::io::Write::write_all(f, b"half of the n")?;
        Err(std::io::Error::other("crash"))
    });
    assert!(r.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"old");
    assert_eq!(
        std::fs::read_dir(&dir.0).unwrap().count(),
        1,
        "no temporary file left"
    );
}

#[tokio::test]
async fn a_graph_saved_while_offline_keeps_its_caught_up_time() {
    let dir = TempDir::new();
    let h = handler(10);
    h.gossip
        .handle_channel_announcement(None, &channel(1, 1, 2))
        .unwrap();
    let bytes = h.gossip.graph().encode();
    let three_days_ago = SystemTime::now() - Duration::from_secs(3 * 86400);
    store::write_atomic(&dir.0.join(store::GRAPH_FILE), |f| {
        std::io::Write::write_all(f, &bytes)?;
        f.set_modified(three_days_ago)
    })
    .unwrap();
    // No peers: it never catches up, so the shutdown save must not look fresh.
    let mut config = Config::new(NET, dir.0.clone());
    config.peers.clear();
    let (peer, _) = Peer::start(config.clone(), FakeChain::default()).unwrap();
    peer.shutdown().await.unwrap();
    let (_, load) = store::load_graph(&dir.0, NET, 10, &sink());
    let GraphLoad::Loaded { age, .. } = load else {
        panic!("{load:?}")
    };
    assert!(age >= Duration::from_secs(3 * 86400 - 60), "{age:?}");
}

#[cfg(unix)]
#[test]
fn the_key_is_private_and_reused() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new();
    let seed = store::load_or_create_key(&dir.0).unwrap();
    let path = dir.0.join(store::KEY_FILE);
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    assert_eq!(store::load_or_create_key(&dir.0).unwrap(), seed);
    assert_ne!(seed, [0; 32]);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(store::load_or_create_key(&dir.0).is_err());
}

#[test]
fn peer_addresses_parse() {
    let s = "03864ef025fde8fb587d989186ce6a4a186895ee44a926bfc370e2c366597a3f8f@3.33.236.230:9735";
    let p: PeerAddr = s.parse().unwrap();
    assert_eq!(p.to_string(), s);
    assert!("nokey@1.2.3.4:9735".parse::<PeerAddr>().is_err());
    assert!(
        "03864ef025fde8fb587d989186ce6a4a186895ee44a926bfc370e2c366597a3f8f@host.onion:9735"
            .parse::<PeerAddr>()
            .is_err()
    );
    assert_eq!(
        default_peers(Network::Bitcoin).len(),
        crate::MAINNET_PEERS.len()
    );
    assert!(default_peers(Network::Signet).is_empty());
}

// A fake remote node: LDK's own peer manager serving a graph, which can also be told to send us
// any message, open_channel included.

struct Lines(Mutex<Vec<String>>);

impl Logger for Lines {
    fn log(&self, record: Record) {
        self.0.lock().unwrap().push(record.args.to_string());
    }
}

type FakeSync = P2PGossipSync<Arc<NetworkGraph<Arc<Lines>>>, UtxoHook, Arc<Lines>>;

struct FakeRouting {
    sync: FakeSync,
    inject: Mutex<Vec<MessageSendEvent>>,
}

impl BaseMessageHandler for FakeRouting {
    fn get_and_clear_pending_msg_events(&self) -> Vec<MessageSendEvent> {
        let mut v = self.sync.get_and_clear_pending_msg_events();
        v.append(&mut self.inject.lock().unwrap());
        v
    }
    fn peer_disconnected(&self, n: PublicKey) {
        self.sync.peer_disconnected(n)
    }
    fn provided_node_features(&self) -> NodeFeatures {
        self.sync.provided_node_features()
    }
    fn provided_init_features(&self, n: PublicKey) -> InitFeatures {
        self.sync.provided_init_features(n)
    }
    fn peer_connected(&self, n: PublicKey, m: &Init, i: bool) -> Result<(), ()> {
        self.sync.peer_connected(n, m, i)
    }
}

impl RoutingMessageHandler for FakeRouting {
    fn handle_node_announcement(
        &self,
        n: Option<PublicKey>,
        m: &NodeAnnouncement,
    ) -> Result<bool, LightningError> {
        self.sync.handle_node_announcement(n, m)
    }
    fn handle_channel_announcement(
        &self,
        n: Option<PublicKey>,
        m: &ChannelAnnouncement,
    ) -> Result<bool, LightningError> {
        self.sync.handle_channel_announcement(n, m)
    }
    fn handle_channel_update(
        &self,
        n: Option<PublicKey>,
        m: &ChannelUpdate,
    ) -> Result<bool, LightningError> {
        self.sync.handle_channel_update(n, m)
    }
    fn get_next_channel_announcement(
        &self,
        s: u64,
    ) -> Option<(
        ChannelAnnouncement,
        Option<ChannelUpdate>,
        Option<ChannelUpdate>,
    )> {
        self.sync.get_next_channel_announcement(s)
    }
    fn get_next_node_announcement(&self, s: Option<&NodeId>) -> Option<NodeAnnouncement> {
        self.sync.get_next_node_announcement(s)
    }
    fn handle_reply_channel_range(
        &self,
        n: PublicKey,
        m: ReplyChannelRange,
    ) -> Result<(), LightningError> {
        self.sync.handle_reply_channel_range(n, m)
    }
    fn handle_reply_short_channel_ids_end(
        &self,
        n: PublicKey,
        m: ReplyShortChannelIdsEnd,
    ) -> Result<(), LightningError> {
        self.sync.handle_reply_short_channel_ids_end(n, m)
    }
    fn handle_query_channel_range(
        &self,
        n: PublicKey,
        m: QueryChannelRange,
    ) -> Result<(), LightningError> {
        self.sync.handle_query_channel_range(n, m)
    }
    fn handle_query_short_channel_ids(
        &self,
        n: PublicKey,
        m: QueryShortChannelIds,
    ) -> Result<(), LightningError> {
        self.sync.handle_query_short_channel_ids(n, m)
    }
    fn processing_queue_high(&self) -> bool {
        false
    }
}

type FakePm = PeerManager<
    lightning_net_tokio::SocketDescriptor,
    lightning::ln::peer_handler::ErroringMessageHandler,
    Arc<FakeRouting>,
    lightning::ln::peer_handler::IgnoringMessageHandler,
    Arc<Lines>,
    lightning::ln::peer_handler::IgnoringMessageHandler,
    Arc<KeysManager>,
    lightning::ln::peer_handler::IgnoringMessageHandler,
>;

struct Fake {
    pm: Arc<FakePm>,
    routing: Arc<FakeRouting>,
    lines: Arc<Lines>,
    addr: PeerAddr,
}

/// Starts a fake node on 127.0.0.1 with port 0, holding one channel and its two nodes' announcements.
async fn fake() -> Fake {
    let lines = Arc::new(Lines(Mutex::new(Vec::new())));
    let graph = Arc::new(NetworkGraph::new(NET, Arc::clone(&lines)));
    let routing = Arc::new(FakeRouting {
        sync: P2PGossipSync::new(graph, None, Arc::clone(&lines)),
        inject: Mutex::new(Vec::new()),
    });
    routing
        .sync
        .handle_channel_announcement(None, &channel(7, 1, 2))
        .unwrap();
    routing
        .sync
        .handle_node_announcement(None, &node(1, "one", 0))
        .unwrap();
    routing
        .sync
        .handle_node_announcement(None, &node(2, "two", 0))
        .unwrap();
    let keys = Arc::new(KeysManager::new(&[42; 32], 1, 0, true));
    let pm = Arc::new(FakePm::new_routing_only(
        Arc::clone(&routing),
        now(),
        &[7; 32],
        Arc::clone(&lines),
        Arc::clone(&keys),
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = PeerAddr {
        node_id: lightning::sign::NodeSigner::get_node_id(&*keys, lightning::sign::Recipient::Node)
            .unwrap(),
        addr: listener.local_addr().unwrap(),
    };
    let accept_pm = Arc::clone(&pm);
    tokio::spawn(async move {
        while let Ok((s, _)) = listener.accept().await {
            let pm = Arc::clone(&accept_pm);
            tokio::spawn(lightning_net_tokio::setup_inbound(
                pm,
                s.into_std().unwrap(),
            ));
        }
    });
    let tick_pm = Arc::clone(&pm);
    tokio::spawn(async move {
        loop {
            tick_pm.process_events();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
    Fake {
        pm,
        routing,
        lines,
        addr,
    }
}

fn open_channel(to: PublicKey) -> MessageSendEvent {
    let pk = key(3).public_key(&Secp256k1::new());
    MessageSendEvent::SendOpenChannel {
        node_id: to,
        msg: OpenChannel {
            common_fields: CommonOpenChannelFields {
                chain_hash: chain(),
                temporary_channel_id: ChannelId([5; 32]),
                funding_satoshis: 1_000_000,
                dust_limit_satoshis: 546,
                max_htlc_value_in_flight_msat: 1_000_000_000,
                htlc_minimum_msat: 1,
                commitment_feerate_sat_per_1000_weight: 253,
                to_self_delay: 144,
                max_accepted_htlcs: 30,
                funding_pubkey: pk,
                revocation_basepoint: pk,
                payment_basepoint: pk,
                delayed_payment_basepoint: pk,
                htlc_basepoint: pk,
                first_per_commitment_point: pk,
                channel_flags: 1,
                shutdown_scriptpubkey: None,
                channel_type: None,
            },
            push_msat: 0,
            channel_reserve_satoshis: 10_000,
        },
    }
}

async fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
    for _ in 0..500 {
        if ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {what}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn syncs_refuses_channels_reconnects_and_restarts_from_disk() {
    let dir = TempDir::new();
    let fake = fake().await;
    // A peer that refuses connections, listed first: it must give its slot to the fake.
    let dead = {
        let l = StdListener::bind("127.0.0.1:0").unwrap();
        PeerAddr {
            node_id: key(50).public_key(&Secp256k1::new()),
            addr: l.local_addr().unwrap(),
        }
    };
    let mut config = Config::new(NET, dir.0.clone());
    config.peers = vec![dead, fake.addr];
    config.max_peers = 1;
    config.backoff_min = Duration::from_millis(50);
    config.sync_settle = Duration::from_millis(300);

    let node = FakeChain::at(100);
    node.add(7, &funding(1, 2), 50_000, false);
    let (peer, load) = Peer::start(config.clone(), node.clone()).unwrap();
    assert!(matches!(load, GraphLoad::Fresh));
    let status = peer.status();

    // Initial sync from the fake, after the dead peer failed.
    wait_for("the graph", || peer.read(|g| g.channel(7).is_some())).await;
    wait_for("synced", || status.borrow().synced).await;
    let s = status.borrow().clone();
    assert_eq!(s.validation.verified, 1);
    assert!(s.validation.caught_up);
    assert_eq!(
        peer.read(|g| g.channel(7).unwrap().capacity_sats),
        Some(50_000)
    );
    assert!(s.peers[0].failures >= 1 && !s.peers[0].connected);
    assert!(s.peers[1].connected);
    let n1 = peer.read(|g| {
        let info = g
            .node(&id(&key(1)))
            .unwrap()
            .announcement_info
            .clone()
            .unwrap();
        info.alias().0
    });
    assert_eq!(&n1[..3], b"one");

    // The fake asks to open a channel; the peer answers with an error and keeps no state.
    let us = peer.node_id();
    fake.routing.inject.lock().unwrap().push(open_channel(us));
    fake.pm.process_events();
    wait_for("the refusal", || {
        fake.lines.0.lock().unwrap().iter().any(|l| {
            l.contains("Got Err message") && l.contains("We do not support channel messages")
        })
    })
    .await;

    // The fake drops us; the peer reconnects.
    let mut events = peer.events();
    fake.pm.disconnect_all_peers();
    let mut disconnected = false;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match events.recv().await {
                Ok(Event::PeerDisconnected { peer, .. }) if peer == fake.addr => {
                    disconnected = true
                }
                Ok(Event::PeerConnected(p)) if p == fake.addr && disconnected => break,
                _ => {}
            }
        }
    })
    .await
    .expect("reconnected");
    wait_for("connected again", || status.borrow().peers[1].connected).await;

    let written = peer.shutdown().await.unwrap();
    assert!(written > 0);
    wait_for("disconnected", || fake.pm.list_peers().is_empty()).await;

    // A restart loads the graph and reuses the identity.
    let (again, load) = Peer::start(config, node).unwrap();
    assert!(
        matches!(
            load,
            GraphLoad::Loaded {
                channels: 1,
                nodes: 2,
                ..
            }
        ),
        "{load:?}"
    );
    assert_eq!(again.node_id(), us);
    // The stored channel is checked again before it is shown.
    wait_for("synced from disk", || again.status().borrow().synced).await;
    assert!(again.read(|g| g.channel(7).is_some()));
    again.shutdown().await.unwrap();
}

// Validation against a fake node.

/// A fake rbitcoin answering the position routes from a table.
#[derive(Clone, Default)]
struct FakeChain(Arc<FakeNode>);

#[derive(Default)]
struct FakeNode {
    outputs: Mutex<HashMap<OutPosition, PositionedOutput>>,
    tip: AtomicU32,
    down: AtomicBool,
    /// When each batch request arrived and how many outputs it asked for.
    batches: Mutex<Vec<(std::time::Instant, usize)>>,
    tip_reads: AtomicUsize,
    /// The best chain from height 0, for the close-watcher. Positions not in `outputs` are read
    /// from it.
    blocks: Mutex<Vec<lightning::bitcoin::Block>>,
    /// The table in `outputs` answers one request, at the tip in `tip`, then is cleared.
    stale_once: AtomicBool,
    /// Every request of the close-watcher, by route.
    asked: Mutex<Vec<&'static str>>,
    /// How long every request takes; requests running now, the most ever at once, and when each
    /// started.
    delay: Mutex<Duration>,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
    starts: Mutex<Vec<std::time::Instant>>,
}

/// A request being answered: counted in flight until dropped.
struct Busy<'a>(&'a FakeNode);

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

impl FakeChain {
    fn at(tip: u32) -> Self {
        let c = Self::default();
        c.0.tip.store(tip, Ordering::Relaxed);
        c
    }

    fn add(&self, scid: u64, script: &ScriptBuf, value: u64, spent: bool) {
        let out = PositionedOutput {
            out: Vout {
                scriptpubkey: script.to_hex_string(),
                scriptpubkey_asm: String::new(),
                scriptpubkey_type: ScriptType::V0P2wsh,
                scriptpubkey_address: None,
                value,
            },
            spent,
            status: TxStatus {
                confirmed: true,
                block_height: Some(position(scid).height),
                block_hash: None,
                block_time: None,
            },
        };
        self.0.outputs.lock().unwrap().insert(position(scid), out);
    }

    async fn busy(&self) -> Busy<'_> {
        let n = &self.0;
        let now = n.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        n.max_in_flight.fetch_max(now, Ordering::SeqCst);
        n.starts.lock().unwrap().push(std::time::Instant::now());
        let delay = *n.delay.lock().unwrap();
        tokio::time::sleep(delay).await;
        Busy(n)
    }

    fn batches(&self) -> Vec<(std::time::Instant, usize)> {
        self.0.batches.lock().unwrap().clone()
    }

    fn ask(&self, route: &'static str) -> Result<(), NodeError> {
        if self.0.down.load(Ordering::Relaxed) {
            return Err(NodeError::Unavailable("down".to_owned()));
        }
        self.0.asked.lock().unwrap().push(route);
        Ok(())
    }

    fn block_at(&self, height: u32) -> Option<lightning::bitcoin::Block> {
        self.0.blocks.lock().unwrap().get(height as usize).cloned()
    }

    fn find_tx(
        &self,
        txid: lightning::bitcoin::Txid,
    ) -> Option<(u32, u32, lightning::bitcoin::Transaction)> {
        let blocks = self.0.blocks.lock().unwrap();
        blocks.iter().enumerate().find_map(|(h, b)| {
            b.txdata
                .iter()
                .position(|t| t.compute_txid() == txid)
                .map(|i| (h as u32, i as u32, b.txdata[i].clone()))
        })
    }

    /// The input spending an output, with its transaction and block.
    fn spender(
        &self,
        out: lightning::bitcoin::OutPoint,
    ) -> Option<(u32, lightning::bitcoin::Transaction, u32)> {
        let blocks = self.0.blocks.lock().unwrap();
        blocks.iter().enumerate().find_map(|(h, b)| {
            b.txdata.iter().find_map(|t| {
                let vin = t.input.iter().position(|i| i.previous_output == out)?;
                Some((h as u32, t.clone(), vin as u32))
            })
        })
    }

    fn status(&self, height: u32) -> TxStatus {
        let b = self.block_at(height).unwrap();
        TxStatus {
            confirmed: true,
            block_height: Some(height),
            block_hash: Some(b.block_hash()),
            block_time: Some(u64::from(b.header.time)),
        }
    }

    /// The output at a position of the chain in `blocks`.
    fn derived(&self, p: &OutPosition) -> Option<PositionedOutput> {
        let b = self.block_at(p.height)?;
        let tx = b.txdata.get(p.index as usize)?;
        let out = tx.output.get(p.vout as usize)?;
        let spent = self
            .spender(lightning::bitcoin::OutPoint::new(tx.compute_txid(), p.vout))
            .is_some();
        Some(PositionedOutput {
            out: Vout {
                scriptpubkey: out.script_pubkey.to_hex_string(),
                scriptpubkey_asm: String::new(),
                scriptpubkey_type: ScriptType::V0P2wsh,
                scriptpubkey_address: None,
                value: out.value.to_sat(),
            },
            spent,
            status: self.status(p.height),
        })
    }

    fn header(&self, height: u32) -> Option<node::esplora::model::Block> {
        let b = self.block_at(height)?;
        Some(node::esplora::model::Block {
            id: b.block_hash(),
            height,
            version: 2,
            timestamp: u64::from(b.header.time),
            tx_count: b.txdata.len() as u32,
            size: 0,
            weight: 0,
            merkle_root: b.header.merkle_root,
            previousblockhash: (height > 0).then_some(b.header.prev_blockhash),
            mediantime: 0,
            nonce: b.header.nonce,
            bits: 0,
            difficulty: 0.0,
        })
    }
}

impl crate::Chain for FakeChain {
    async fn outputs(
        &self,
        positions: OutPositions,
    ) -> Result<Stamped<Vec<Option<PositionedOutput>>>, NodeError> {
        let _busy = self.busy().await;
        let n = &self.0;
        if n.down.load(Ordering::Relaxed) {
            return Err(NodeError::Unavailable("down".to_owned()));
        }
        n.batches
            .lock()
            .unwrap()
            .push((std::time::Instant::now(), positions.as_slice().len()));
        let map = n.outputs.lock().unwrap().clone();
        let tip = n.tip.load(Ordering::Relaxed);
        if n.stale_once.swap(false, Ordering::Relaxed) {
            n.outputs.lock().unwrap().clear();
            let tip = n.blocks.lock().unwrap().len().saturating_sub(1) as u32;
            n.tip.store(tip, Ordering::Relaxed);
        }
        Ok(Stamped {
            value: positions
                .as_slice()
                .iter()
                .map(|p| map.get(p).cloned().or_else(|| self.derived(p)))
                .collect(),
            tip: Some(ChainTip {
                hash: self
                    .block_at(tip)
                    .map_or(BlockHash::all_zeros(), |b| b.block_hash()),
                height: tip,
            }),
        })
    }

    async fn tip_height(&self) -> Result<u32, NodeError> {
        let _busy = self.busy().await;
        if self.0.down.load(Ordering::Relaxed) {
            return Err(NodeError::Unavailable("down".to_owned()));
        }
        self.0.tip_reads.fetch_add(1, Ordering::Relaxed);
        Ok(self.0.tip.load(Ordering::Relaxed))
    }

    async fn tip_hash(&self) -> Result<BlockHash, NodeError> {
        let _busy = self.busy().await;
        self.ask("tip_hash")?;
        let blocks = self.0.blocks.lock().unwrap();
        blocks
            .last()
            .map(|b| b.block_hash())
            .ok_or(NodeError::NotFound)
    }

    async fn headers(
        &self,
        top: Option<u32>,
    ) -> Result<Vec<node::esplora::model::Block>, NodeError> {
        let _busy = self.busy().await;
        self.ask("headers")?;
        let len = self.0.blocks.lock().unwrap().len() as u32;
        let top = top.unwrap_or(u32::MAX).min(len.saturating_sub(1));
        Ok((0..=top)
            .rev()
            .take(10)
            .filter_map(|h| self.header(h))
            .collect())
    }

    async fn block(&self, hash: BlockHash) -> Result<lightning::bitcoin::Block, NodeError> {
        let _busy = self.busy().await;
        self.ask("block")?;
        let blocks = self.0.blocks.lock().unwrap();
        blocks
            .iter()
            .find(|b| b.block_hash() == hash)
            .cloned()
            .ok_or(NodeError::NotFound)
    }

    async fn block_hash(&self, height: u32) -> Result<BlockHash, NodeError> {
        let _busy = self.busy().await;
        self.ask("block_hash")?;
        self.block_at(height)
            .map(|b| b.block_hash())
            .ok_or(NodeError::NotFound)
    }

    async fn block_txid(
        &self,
        hash: BlockHash,
        index: u32,
    ) -> Result<lightning::bitcoin::Txid, NodeError> {
        let _busy = self.busy().await;
        self.ask("block_txid")?;
        let blocks = self.0.blocks.lock().unwrap();
        let b = blocks
            .iter()
            .find(|b| b.block_hash() == hash)
            .ok_or(NodeError::NotFound)?;
        b.txdata
            .get(index as usize)
            .map(|t| t.compute_txid())
            .ok_or(NodeError::NotFound)
    }

    async fn tx_position(&self, txid: lightning::bitcoin::Txid) -> Result<(u32, u32), NodeError> {
        let _busy = self.busy().await;
        self.ask("tx_position")?;
        self.find_tx(txid)
            .map(|(h, i, _)| (h, i))
            .ok_or(NodeError::NotFound)
    }

    async fn outspend(
        &self,
        txid: lightning::bitcoin::Txid,
        vout: u32,
    ) -> Result<node::esplora::model::Outspend, NodeError> {
        let _busy = self.busy().await;
        self.ask("outspend")?;
        let s = self.spender(lightning::bitcoin::OutPoint::new(txid, vout));
        Ok(node::esplora::model::Outspend {
            spent: s.is_some(),
            txid: s.as_ref().map(|(_, t, _)| t.compute_txid()),
            vin: s.as_ref().map(|(_, _, v)| *v),
            status: s.map(|(h, _, _)| self.status(h)),
        })
    }

    async fn tx(
        &self,
        txid: lightning::bitcoin::Txid,
    ) -> Result<lightning::bitcoin::Transaction, NodeError> {
        let _busy = self.busy().await;
        self.ask("tx")?;
        self.find_tx(txid)
            .map(|(_, _, t)| t)
            .ok_or(NodeError::NotFound)
    }

    async fn tx_fee(&self, txid: lightning::bitcoin::Txid) -> Result<Option<u64>, NodeError> {
        let _busy = self.busy().await;
        self.ask("tx_fee")?;
        let Some((_, _, tx)) = self.find_tx(txid) else {
            return Err(NodeError::NotFound);
        };
        let mut inputs = 0;
        for i in &tx.input {
            let Some((_, _, prev)) = self.find_tx(i.previous_output.txid) else {
                return Ok(None);
            };
            inputs += prev.output[i.previous_output.vout as usize].value.to_sat();
        }
        let out: u64 = tx.output.iter().map(|o| o.value.to_sat()).sum();
        Ok(Some(inputs - out))
    }
}

fn scid(height: u32, index: u32, vout: u32) -> u64 {
    u64::from(height) << 40 | u64::from(index) << 16 | u64::from(vout)
}

/// The output a channel between `a` and `b` (as built by `channel`) must be funded by.
fn funding(a: u8, b: u8) -> ScriptBuf {
    let s = Secp256k1::new();
    make_funding_redeemscript(&key(a + 100).public_key(&s), &key(b + 100).public_key(&s)).to_p2wsh()
}

/// The gate the peer gives its request loops, as `Peer::start` makes it.
fn pace(config: &Config) -> Arc<crate::pace::Pace> {
    Arc::new(crate::pace::Pace::new(config.lookup_interval))
}

fn quick(f: impl FnOnce(&mut Config)) -> Config {
    let mut c = Config::new(NET, PathBuf::new());
    c.lookup_interval = Duration::from_millis(20);
    f(&mut c);
    c
}

/// A handler checking announcements against `node`, with the lookup task running.
fn checked(config: &Config, node: &FakeChain) -> (Arc<Gossip>, broadcast::Receiver<Event>) {
    let (g, events, _) = gossip(config, Replay::default());
    tokio::spawn(chain::run(Arc::clone(&g), node.clone(), pace(config)));
    (g, events)
}

fn rejected(g: &Gossip) -> Rejected {
    g.lookup.snapshot().rejected
}

#[tokio::test]
async fn a_real_channel_is_accepted_with_its_capacity() {
    let node = FakeChain::at(1000);
    let id = scid(500, 3, 1);
    node.add(id, &funding(1, 2), 2_500_000, false);
    let (g, _) = checked(&quick(|_| {}), &node);
    let ann = channel(id, 1, 2);
    // Answered later: held, and not in the graph yet.
    assert!(g.handle_channel_announcement(None, &ann).is_err());
    assert_eq!(g.counters.held.load(Ordering::Relaxed), 1);
    assert!(g.graph().read_only().channel(id).is_none());
    // Its update waits with it.
    let dir = u8::from(id_of(1) != ann.contents.node_id_1);
    assert!(g.handle_channel_update(None, &update(id, 1, dir)).is_err());

    wait_for("validated", || g.graph().read_only().channel(id).is_some()).await;
    let ro = g.graph().read_only();
    let c = ro.channel(id).unwrap();
    assert_eq!(c.capacity_sats, Some(2_500_000));
    assert!(
        c.one_to_two.is_some() || c.two_to_one.is_some(),
        "the held update applied"
    );
    drop(ro);
    assert_eq!(rejected(&g).total(), 0);
    assert_eq!(g.counters.accepted.load(Ordering::Relaxed), 1);
    // Announced again: LDK knows it is chain-validated and asks nothing.
    let before = node.batches().len();
    assert!(g.handle_channel_announcement(None, &ann).is_err());
    assert_eq!(node.batches().len(), before);
}

fn id_of(n: u8) -> NodeId {
    id(&key(n))
}

#[tokio::test]
async fn false_channels_are_refused_for_their_reason_and_early_ones_wait() {
    let node = FakeChain::at(1000);
    let (g, _) = checked(&quick(|_| {}), &node);
    let real = scid(900, 1, 0);
    let wrong_script = scid(900, 2, 0);
    let missing = scid(900, 3, 0);
    let spent = scid(900, 4, 0);
    let shallow = scid(998, 1, 0);
    let just_past_tip = scid(1002, 1, 0);
    let far_future = scid(5000, 1, 0);
    node.add(real, &funding(1, 2), 1_000, false);
    node.add(wrong_script, &funding(3, 4), 1_000, false);
    node.add(spent, &funding(1, 2), 1_000, true);
    node.add(shallow, &funding(1, 2), 7_000, false);

    for id in [
        real,
        wrong_script,
        missing,
        spent,
        shallow,
        just_past_tip,
        far_future,
    ] {
        assert!(
            g.handle_channel_announcement(None, &channel(id, 1, 2))
                .is_err()
        );
    }
    let other_chain = channel_on(
        ChainHash::using_genesis_block(Network::Bitcoin),
        scid(900, 5, 0),
        1,
        2,
    );
    assert!(g.handle_channel_announcement(None, &other_chain).is_err());

    wait_for("lookups", || g.lookup.waiting() == 0).await;
    let r = rejected(&g);
    assert_eq!(
        (r.missing, r.spent, r.wrong_script, r.wrong_chain, r.shallow),
        (2, 1, 1, 1, 0),
        "{r:?}"
    );
    let snap = g.lookup.snapshot();
    assert_eq!(snap.early, 2, "too shallow, and a block just past the tip");
    let in_graph = |id| g.graph().read_only().channel(id).is_some();
    wait_for("the real one", || in_graph(real)).await;
    for id in [
        wrong_script,
        missing,
        spent,
        shallow,
        just_past_tip,
        far_future,
    ] {
        assert!(!in_graph(id));
    }

    // A refused channel announced again is answered from the cache.
    let before = node.batches().len();
    assert!(
        g.handle_channel_announcement(None, &channel(missing, 1, 2))
            .is_err()
    );
    assert_eq!(node.batches().len(), before);
    assert_eq!(rejected(&g).missing, 2);

    // Three blocks later the shallow one is six deep: the next answer's tip promotes it.
    node.0.tip.store(1003, Ordering::Relaxed);
    let later = scid(950, 1, 0);
    node.add(later, &funding(1, 2), 1_000, false);
    assert!(
        g.handle_channel_announcement(None, &channel(later, 1, 2))
            .is_err()
    );
    wait_for("the shallow one", || in_graph(shallow)).await;
    assert_eq!(
        g.graph()
            .read_only()
            .channel(shallow)
            .unwrap()
            .capacity_sats,
        Some(7_000)
    );
    // The one past the tip is still not mined: six blocks on, it is found missing.
    node.0.tip.store(1007, Ordering::Relaxed);
    let last = scid(951, 1, 0);
    node.add(last, &funding(1, 2), 1_000, false);
    assert!(
        g.handle_channel_announcement(None, &channel(last, 1, 2))
            .is_err()
    );
    wait_for("refused once deep", || rejected(&g).missing == 3).await;
    assert_eq!(g.lookup.snapshot().early, 0);
}

#[tokio::test]
async fn early_announcements_are_rechecked_from_the_tip_alone() {
    let node = FakeChain::at(1000);
    let shallow = scid(999, 1, 0);
    node.add(shallow, &funding(1, 2), 5_000, false);
    let (g, _) = checked(&quick(|_| {}), &node);
    assert!(
        g.handle_channel_announcement(None, &channel(shallow, 1, 2))
            .is_err()
    );
    wait_for("early", || g.lookup.snapshot().early == 1).await;
    node.0.tip.store(1004, Ordering::Relaxed);
    // With nothing else to ask, the tip is read now and then (once a minute; tests, a second).
    wait_for("promoted by the tip", || {
        g.graph().read_only().channel(shallow).is_some()
    })
    .await;
    assert!(g.graph().read_only().channel(shallow).is_some());
    assert!(node.0.tip_reads.load(Ordering::Relaxed) >= 1);
}

#[tokio::test]
async fn a_burst_becomes_a_few_batches_within_the_rate_limit() {
    let node = FakeChain::at(10_000);
    let interval = Duration::from_millis(300);
    let config = quick(|c| c.lookup_interval = interval);
    let anns: Vec<_> = (0..1000)
        .map(|i| {
            let id = scid(5000, i, 0);
            node.add(id, &funding(1, 2), 1_000 + u64::from(i), false);
            channel(id, 1, 2)
        })
        .collect();
    let (g, _) = checked(&config, &node);
    for a in &anns {
        assert!(g.handle_channel_announcement(None, a).is_err());
    }
    wait_for("all validated", || {
        g.graph().read_only().channels().len() == 1000
    })
    .await;
    let batches = node.batches();
    let sizes: Vec<usize> = batches.iter().map(|b| b.1).collect();
    assert_eq!(sizes.iter().sum::<usize>(), 1000);
    assert!(sizes.iter().all(|n| *n <= chain::LOOKUP_BATCH));
    assert!(batches.len() <= 12, "{sizes:?}");
    for w in batches.windows(2) {
        assert!(
            w[1].0 - w[0].0 >= interval,
            "requests closer than the limit"
        );
    }
    assert_eq!(g.lookup.snapshot().requests, batches.len() as u64);
}

#[tokio::test]
async fn the_pending_queue_is_bounded() {
    let config = quick(|c| c.max_pending = 10);
    // No lookup task: nothing drains.
    let (g, _, _) = gossip(&config, Replay::default());
    for i in 0..100 {
        assert!(
            g.handle_channel_announcement(None, &channel(scid(10, i, 0), 1, 2))
                .is_err()
        );
        if i == 9 {
            assert!(g.processing_queue_high(), "high at max_pending");
        }
    }
    let s = g.lookup.snapshot();
    // LDK reads at most one more message per peer once high; the slack covers that.
    assert_eq!(s.pending, 10 + 64);
    assert_eq!(s.rejected.queue_full, 100 - 74);
    assert_eq!(g.graph().read_only().channels().len(), 0);
}

#[tokio::test]
async fn the_node_down_then_back_loses_nothing() {
    let node = FakeChain::at(1000);
    node.0.down.store(true, Ordering::Relaxed);
    let ids: Vec<u64> = (0..3).map(|i| scid(800, i, 0)).collect();
    for id in &ids {
        node.add(*id, &funding(1, 2), 9_000, false);
    }
    let (g, mut events) = checked(&quick(|_| {}), &node);
    for id in &ids {
        assert!(
            g.handle_channel_announcement(None, &channel(*id, 1, 2))
                .is_err()
        );
    }
    let failed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(Event::LookupFailed { pending, error, .. }) = events.recv().await {
                // A fixed label, not the node's own text ("down").
                assert_eq!(error, "unavailable");
                return pending;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(failed, 3, "the backlog is reported");
    let s = g.lookup.snapshot();
    assert!(s.failures >= 1 && s.pending == 3);
    assert!(!g.processing_queue_high());
    node.0.down.store(false, Ordering::Relaxed);
    wait_for("validated after the node came back", || {
        g.graph().read_only().channels().len() == 3
    })
    .await;
    assert_eq!(g.lookup.snapshot().failures, 0);
}

#[tokio::test]
async fn invented_channels_cannot_grow_memory_the_cache_or_node_load() {
    let node = FakeChain::at(100_000);
    let interval = Duration::from_millis(50);
    let config = quick(|c| {
        c.lookup_interval = interval;
        c.max_pending = 50;
        c.max_early = 10;
        c.cache_entries = 200;
    });
    let (g, _) = checked(&config, &node);
    let mut most = 0;
    for i in 0..1500u32 {
        // Some claim a block just past the tip, which looks early.
        let height = if i % 10 == 0 { 100_001 } else { 90_000 };
        let _ = g.handle_channel_announcement(None, &channel(scid(height, i, 0), 1, 2));
        let s = g.lookup.snapshot();
        most = most.max(s.pending);
        assert!(s.early <= 10);
        assert!(g.lookup.cache_len() <= 200);
        if i % 100 == 0 {
            tokio::task::yield_now().await;
        }
    }
    assert!(most <= 50 + 64);
    wait_for("drained", || g.lookup.waiting() == 0).await;
    assert!(g.lookup.cache_len() <= 200);
    assert_eq!(g.graph().read_only().channels().len(), 0);
    let batches = node.batches();
    assert!(batches.iter().all(|b| b.1 <= 300));
    for w in batches.windows(2) {
        assert!(w[1].0 - w[0].0 >= interval);
    }
    let r = rejected(&g);
    assert!(r.missing > 0 && r.queue_full > 0 && r.shallow > 0, "{r:?}");
}

/// A graph as the unverified peer saved it: two channels taken on signatures alone, one real and
/// one invented, with their updates and node announcements.
fn unverified_graph(dir: &std::path::Path) -> (u64, u64) {
    let real = scid(700, 1, 0);
    let invented = scid(700, 2, 0);
    let graph = Graph::new(NET, sink());
    for (id, a, b) in [(real, 1, 2), (invented, 3, 4)] {
        let ann = channel(id, a, b);
        graph
            .update_channel_from_announcement_no_lookup(&ann)
            .unwrap();
        let signer = if id_of(a) == ann.contents.node_id_1 {
            a
        } else {
            b
        };
        graph.update_channel(&update(id, signer, 0)).unwrap();
        graph
            .update_node_from_announcement(&node(a, "stored", 0))
            .unwrap();
    }
    assert_eq!(graph.read_only().channel(real).unwrap().capacity_sats, None);
    let bytes = graph.encode();
    store::write_atomic(&dir.join(store::GRAPH_FILE), |f| {
        std::io::Write::write_all(f, &bytes)
    })
    .unwrap();
    (real, invented)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_graph_saved_unverified_is_checked_again_before_it_is_shown() {
    let dir = TempDir::new();
    let (real, invented) = unverified_graph(&dir.0);
    let node = FakeChain::at(1000);
    node.add(real, &funding(1, 2), 4_200_000, false);
    let mut config = quick(|_| {});
    config.data_dir = dir.0.clone();
    config.peers.clear();

    let (peer, load) = Peer::start(config.clone(), node.clone()).unwrap();
    assert!(
        matches!(load, GraphLoad::Loaded { channels: 2, .. }),
        "{load:?}"
    );
    // Nothing from the file is shown before it is checked.
    assert_eq!(peer.read(|g| g.channels().len()), 0);
    let status = peer.status();
    wait_for("checked and synced", || status.borrow().synced).await;
    let v = status.borrow().validation;
    assert_eq!((v.verified, v.unchecked, v.pending), (1, 0, 0));
    assert_eq!(v.rejected.missing, 1);
    peer.read(|g| {
        let c = g.channel(real).unwrap();
        assert_eq!(c.capacity_sats, Some(4_200_000));
        assert!(
            c.one_to_two.is_some() || c.two_to_one.is_some(),
            "stored update kept"
        );
        let info = g
            .node(&id_of(1))
            .unwrap()
            .announcement_info
            .as_ref()
            .unwrap();
        assert_eq!(&info.alias().0[..6], b"stored");
        assert!(g.channel(invented).is_none());
        assert!(g.node(&id_of(3)).is_none());
    });
    assert!(peer.shutdown().await.unwrap() > 0);
    // What is saved now is only what was verified.
    let (g, _) = store::load_graph(&dir.0, NET, 10, &sink());
    let g = g.unwrap();
    assert_eq!(g.read_only().channels().len(), 1);
    assert_eq!(
        g.read_only().channel(real).unwrap().capacity_sats,
        Some(4_200_000)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_while_the_node_is_down_keeps_the_file() {
    let dir = TempDir::new();
    unverified_graph(&dir.0);
    let before = std::fs::read(dir.0.join(store::GRAPH_FILE)).unwrap();
    let node = FakeChain::at(1000);
    node.0.down.store(true, Ordering::Relaxed);
    let mut config = quick(|_| {});
    config.data_dir = dir.0.clone();
    config.peers.clear();
    let (peer, _) = Peer::start(config, node).unwrap();
    let status = peer.status();
    wait_for("sent for checking", || {
        status.borrow().validation.pending == 2
    })
    .await;
    assert!(!status.borrow().synced);
    assert_eq!(peer.read(|g| g.channels().len()), 0);
    // Stopped mid-check: the graph lacks the stored channels, so the file is kept as it was.
    peer.shutdown().await.unwrap();
    let after = std::fs::read(dir.0.join(store::GRAPH_FILE)).unwrap();
    assert!(before == after, "the file was rewritten");
}

/// A graph holding channel `scid` between nodes 1 and 2, unchecked, with node 1 announced as
/// "stored".
fn stored_graph(scid: u64) -> Graph {
    let graph = Graph::new(NET, sink());
    graph
        .update_channel_from_announcement_no_lookup(&channel(scid, 1, 2))
        .unwrap();
    graph
        .update_node_from_announcement(&node(1, "stored", 0))
        .unwrap();
    graph
}

fn alias_of(g: &Gossip, n: u8) -> Option<[u8; 32]> {
    let ro = g.graph().read_only();
    Some(ro.node(&id_of(n))?.announcement_info.as_ref()?.alias().0)
}

#[tokio::test]
async fn a_stored_channel_found_early_keeps_the_replay_open_until_it_is_deep() {
    let id = scid(998, 1, 0);
    let node = FakeChain::at(1000);
    node.add(id, &funding(1, 2), 5_000_000, false);
    let config = quick(|_| {});
    let (g, _, _) = gossip(&config, Replay::from_graph(&stored_graph(id)));
    tokio::spawn(chain::run(Arc::clone(&g), node.clone(), pace(&config)));
    g.replay_tick(config.max_pending);
    wait_for("early", || g.lookup.snapshot().early == 1).await;
    g.replay_tick(config.max_pending);
    assert!(
        g.replaying(),
        "its stored node announcement is still needed"
    );
    node.0.tip.store(1003, Ordering::Relaxed);
    wait_for("deep enough", || {
        g.graph().read_only().channel(id).is_some()
    })
    .await;
    g.replay_tick(config.max_pending);
    assert_eq!(&alias_of(&g, 1).unwrap()[..6], b"stored");
    assert!(!g.replaying());
}

#[tokio::test]
async fn a_newer_oversized_node_announcement_waits_for_its_stored_channel() {
    let id = scid(900, 1, 0);
    let fake = FakeChain::at(1000);
    fake.add(id, &funding(1, 2), 5_000_000, false);
    let config = quick(|_| {});
    let (g, _, _) = gossip(&config, Replay::from_graph(&stored_graph(id)));
    // Newer than the stored one, and over the size limit: kept trimmed.
    let mut big = node(1, "newer", 2000);
    big.contents.timestamp += 10;
    big.signature = sign(&big.contents, &key(1));
    assert!(big.serialized_length() > MAX_GOSSIP_BYTES);
    assert!(g.handle_node_announcement(None, &big).is_err());
    assert_eq!(g.counters.held.load(Ordering::Relaxed), 1);
    tokio::spawn(chain::run(Arc::clone(&g), fake, pace(&config)));
    g.replay_tick(config.max_pending);
    wait_for("checked", || g.graph().read_only().channel(id).is_some()).await;
    g.replay_tick(config.max_pending);
    assert_eq!(&alias_of(&g, 1).unwrap()[..5], b"newer");
}

#[tokio::test]
async fn a_forged_update_cannot_replace_a_stored_one() {
    let id = scid(900, 1, 0);
    let fake = FakeChain::at(1000);
    fake.add(id, &funding(1, 2), 5_000_000, false);
    let graph = stored_graph(id);
    let ann = channel(id, 1, 2);
    let (signer, dir) = if id_of(1) == ann.contents.node_id_1 {
        (1, 0)
    } else {
        (2, 0)
    };
    let good = update(id, signer, dir);
    graph.update_channel(&good).unwrap();
    let config = quick(|_| {});
    let (g, _, _) = gossip(&config, Replay::from_graph(&graph));
    // Signed by a key that is not the channel's: refused, not kept for the replay.
    let mut forged = update(id, 9, dir);
    forged.contents.timestamp = u32::MAX;
    forged.signature = sign(&forged.contents, &key(9));
    assert!(g.handle_channel_update(None, &forged).is_err());
    assert_eq!(g.counters.held.load(Ordering::Relaxed), 0);
    tokio::spawn(chain::run(Arc::clone(&g), fake, pace(&config)));
    g.replay_tick(config.max_pending);
    wait_for("checked", || g.graph().read_only().channel(id).is_some()).await;
    let ro = g.graph().read_only();
    let c = ro.channel(id).unwrap();
    let policy = c.one_to_two.as_ref().or(c.two_to_one.as_ref()).unwrap();
    assert_eq!(policy.last_update, good.contents.timestamp);
}

#[tokio::test]
async fn stored_channels_keep_their_place_under_the_ceiling() {
    let (a, b) = (scid(900, 1, 0), scid(900, 2, 0));
    let fake = FakeChain::at(1000);
    let graph = stored_graph(a);
    graph
        .update_channel_from_announcement_no_lookup(&channel(b, 1, 2))
        .unwrap();
    for id in [a, b] {
        fake.add(id, &funding(1, 2), 5_000_000, false);
    }
    let config = quick(|c| c.max_channels = 2);
    let (g, _, _) = gossip(&config, Replay::from_graph(&graph));
    // The two stored channels fill the ceiling before they are even sent.
    let c = scid(900, 3, 0);
    fake.add(c, &funding(1, 2), 5_000_000, false);
    assert!(
        g.handle_channel_announcement(None, &channel(c, 1, 2))
            .is_err()
    );
    tokio::spawn(chain::run(Arc::clone(&g), fake, pace(&config)));
    g.replay_tick(config.max_pending);
    assert!(
        g.handle_channel_announcement(None, &channel(c, 1, 2))
            .is_err()
    );
    wait_for("both stored checked", || {
        g.graph().read_only().channels().len() == 2
    })
    .await;
    assert_eq!(g.counters.over_ceiling.load(Ordering::Relaxed), 2);
}

mod closes;
mod hostile;
