//! A hostile Lightning peer, and what it can make our peer do (`docs/LIGHTNING.md`, Hostile
//! peers). Our peer dials it like any configured peer. It completes the real handshake (LDK's own
//! peer manager does the Noise part), then sends whatever a test queues: any message type with any
//! body, signed or not, as fast as our side reads. A proxy between the two can stop reading what we
//! send, or pass what it sends one byte at a time.
//!
//! The fast attacks run with the normal suite. The floods are `#[ignore]`d; run them with
//! `cargo test -p ln --release -- --ignored --nocapture hostile::flood`. Each lasts under two
//! minutes and prints what it measured.

// The floods print what they measured; the attack task runs until it is aborted.
#![allow(clippy::print_stdout, clippy::for_unbounded_range)]

use std::collections::VecDeque;
use std::sync::atomic::AtomicU64;

use lightning::ln::msgs::{Ping, Pong};
use lightning::ln::peer_handler::{ErroringMessageHandler, IgnoringMessageHandler, MessageHandler};
use lightning::ln::wire::{CustomMessageReader, Type};
use lightning::util::ser::{LengthLimitedRead, Writer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::*;
use crate::gossip::MAX_NODE_ADDRESSES;

/// A message of any type with any body: what a hostile peer sends.
#[derive(Clone, Debug)]
struct Raw {
    ty: u16,
    body: Vec<u8>,
}

impl Type for Raw {
    fn type_id(&self) -> u16 {
        self.ty
    }
}

impl Writeable for Raw {
    fn write<W: Writer>(&self, w: &mut W) -> Result<(), lightning::io::Error> {
        w.write_all(&self.body)
    }
}

fn raw<T: Writeable>(ty: u16, msg: &T) -> Raw {
    Raw {
        ty,
        body: msg.encode(),
    }
}

/// BOLT 1 and BOLT 7 message types.
const PING: u16 = 18;
const CHANNEL_ANNOUNCEMENT: u16 = 256;
const NODE_ANNOUNCEMENT: u16 = 257;
const CHANNEL_UPDATE: u16 = 258;
const QUERY_SHORT_CHANNEL_IDS: u16 = 261;
const QUERY_CHANNEL_RANGE: u16 = 263;
const REPLY_CHANNEL_RANGE: u16 = 264;

/// Bytes queued for us and not yet passed by the proxy, at most: the hostile side is paced by how
/// fast we read, as a real sender is by TCP.
const WINDOW: u64 = 256 * 1024;
/// Noise framing per message: two MACs and the encrypted length, plus the type.
const FRAMING: u64 = 16 + 16 + 2 + 2;

/// What the hostile peer does and what it saw. Its routing handler answers nothing and serves
/// nothing; everything it sends goes through [`Hostile::send`].
#[derive(Default)]
struct Script {
    queue: Mutex<VecDeque<Raw>>,
    /// Bytes handed to its peer manager so far.
    released: AtomicU64,
    /// Bytes the proxy has passed to us.
    forwarded: Arc<AtomicU64>,
    us: Mutex<Option<PublicKey>>,
    /// Refuse every connection right after the handshake.
    refuse: AtomicBool,
    /// Connections that completed the handshake.
    connections: AtomicUsize,
    /// What we sent it.
    range_queries: AtomicUsize,
    scid_queries: AtomicUsize,
    range_replies: AtomicUsize,
    range_reply_ids: AtomicUsize,
}

impl BaseMessageHandler for Script {
    fn get_and_clear_pending_msg_events(&self) -> Vec<MessageSendEvent> {
        Vec::new()
    }
    fn peer_disconnected(&self, _: PublicKey) {}
    fn provided_node_features(&self) -> NodeFeatures {
        NodeFeatures::empty()
    }
    fn provided_init_features(&self, _: PublicKey) -> InitFeatures {
        let mut f = InitFeatures::empty();
        f.set_gossip_queries_optional();
        f
    }
    fn peer_connected(&self, n: PublicKey, _: &Init, _: bool) -> Result<(), ()> {
        if self.refuse.load(Ordering::Relaxed) {
            return Err(());
        }
        self.connections.fetch_add(1, Ordering::Relaxed);
        *self.us.lock().unwrap() = Some(n);
        Ok(())
    }
}

impl RoutingMessageHandler for Script {
    fn handle_node_announcement(
        &self,
        _: Option<PublicKey>,
        _: &NodeAnnouncement,
    ) -> Result<bool, LightningError> {
        Ok(false)
    }
    fn handle_channel_announcement(
        &self,
        _: Option<PublicKey>,
        _: &ChannelAnnouncement,
    ) -> Result<bool, LightningError> {
        Ok(false)
    }
    fn handle_channel_update(
        &self,
        _: Option<PublicKey>,
        _: &ChannelUpdate,
    ) -> Result<bool, LightningError> {
        Ok(false)
    }
    fn get_next_channel_announcement(
        &self,
        _: u64,
    ) -> Option<(
        ChannelAnnouncement,
        Option<ChannelUpdate>,
        Option<ChannelUpdate>,
    )> {
        None
    }
    fn get_next_node_announcement(&self, _: Option<&NodeId>) -> Option<NodeAnnouncement> {
        None
    }
    fn handle_reply_channel_range(
        &self,
        _: PublicKey,
        m: ReplyChannelRange,
    ) -> Result<(), LightningError> {
        self.range_replies.fetch_add(1, Ordering::Relaxed);
        self.range_reply_ids
            .fetch_add(m.short_channel_ids.len(), Ordering::Relaxed);
        Ok(())
    }
    fn handle_reply_short_channel_ids_end(
        &self,
        _: PublicKey,
        _: ReplyShortChannelIdsEnd,
    ) -> Result<(), LightningError> {
        Ok(())
    }
    fn handle_query_channel_range(
        &self,
        _: PublicKey,
        _: QueryChannelRange,
    ) -> Result<(), LightningError> {
        self.range_queries.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    fn handle_query_short_channel_ids(
        &self,
        _: PublicKey,
        _: QueryShortChannelIds,
    ) -> Result<(), LightningError> {
        self.scid_queries.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    fn processing_queue_high(&self) -> bool {
        false
    }
}

impl CustomMessageReader for Script {
    type CustomMessage = Raw;
    fn read<R: LengthLimitedRead>(
        &self,
        _: u16,
        _: &mut R,
    ) -> Result<Option<Raw>, lightning::ln::msgs::DecodeError> {
        Ok(None)
    }
}

impl lightning::ln::peer_handler::CustomMessageHandler for Script {
    fn handle_custom_message(&self, _: Raw, _: PublicKey) -> Result<(), LightningError> {
        Ok(())
    }

    /// Releases queued messages while fewer than [`WINDOW`] bytes wait for us to read them.
    fn get_and_clear_pending_msg(&self) -> Vec<(PublicKey, Raw)> {
        let Some(us) = *self.us.lock().unwrap() else {
            return Vec::new();
        };
        let mut q = self.queue.lock().unwrap();
        let mut out = Vec::new();
        while self.released.load(Ordering::Relaxed)
            < self.forwarded.load(Ordering::Relaxed) + WINDOW
            && let Some(m) = q.pop_front()
        {
            self.released
                .fetch_add(m.body.len() as u64 + FRAMING, Ordering::Relaxed);
            out.push((us, m));
        }
        out
    }

    fn peer_disconnected(&self, _: PublicKey) {
        *self.us.lock().unwrap() = None;
    }
    fn peer_connected(&self, _: PublicKey, _: &Init, _: bool) -> Result<(), ()> {
        Ok(())
    }
    fn provided_node_features(&self) -> NodeFeatures {
        NodeFeatures::empty()
    }
    fn provided_init_features(&self, _: PublicKey) -> InitFeatures {
        InitFeatures::empty()
    }
}

/// The hostile peer's logger: keeps nothing, so floods measure our side, not its log.
struct Quiet;

impl Logger for Quiet {
    fn log(&self, _: Record) {}
}

type HostilePm = PeerManager<
    lightning_net_tokio::SocketDescriptor,
    ErroringMessageHandler,
    Arc<Script>,
    IgnoringMessageHandler,
    Arc<Quiet>,
    Arc<Script>,
    Arc<KeysManager>,
    IgnoringMessageHandler,
>;

/// What the proxy between us and the hostile peer does to the bytes.
#[derive(Default)]
struct Proxy {
    /// Stop reading what we send: our writes back up.
    stall: AtomicBool,
    /// Pass what the hostile peer sends one byte per this many milliseconds (0: at full speed).
    trickle_ms: AtomicU64,
    /// Connections our peer opened to it.
    accepted: AtomicUsize,
}

struct Hostile {
    pm: Arc<HostilePm>,
    script: Arc<Script>,
    proxy: Arc<Proxy>,
    /// What our peer dials: the proxy, with the hostile peer's key.
    addr: PeerAddr,
}

impl Hostile {
    /// Listens on 127.0.0.1, port 0.
    async fn start() -> Self {
        let script = Arc::new(Script::default());
        // Each its own identity: two at once are two peers.
        static SEEDS: AtomicU32 = AtomicU32::new(66);
        let seed = SEEDS.fetch_add(1, Ordering::Relaxed) as u8;
        let keys = Arc::new(KeysManager::new(&[seed; 32], 1, 0, true));
        let pm = Arc::new(HostilePm::new(
            MessageHandler {
                chan_handler: ErroringMessageHandler::new(),
                route_handler: Arc::clone(&script),
                onion_message_handler: IgnoringMessageHandler {},
                custom_message_handler: Arc::clone(&script),
                send_only_message_handler: IgnoringMessageHandler {},
            },
            now(),
            &[9; 32],
            Arc::new(Quiet),
            Arc::clone(&keys),
        ));
        // The hostile peer's own listener, reached only through the proxy.
        let inner = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let inner_addr = inner.local_addr().unwrap();
        let accept_pm = Arc::clone(&pm);
        tokio::spawn(async move {
            while let Ok((s, _)) = inner.accept().await {
                let pm = Arc::clone(&accept_pm);
                tokio::spawn(lightning_net_tokio::setup_inbound(
                    pm,
                    s.into_std().unwrap(),
                ));
            }
        });
        let proxy = Arc::new(Proxy::default());
        let outer = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = PeerAddr {
            node_id: lightning::sign::NodeSigner::get_node_id(
                &*keys,
                lightning::sign::Recipient::Node,
            )
            .unwrap(),
            addr: outer.local_addr().unwrap(),
        };
        let (p, forwarded) = (Arc::clone(&proxy), Arc::clone(&script.forwarded));
        tokio::spawn(async move {
            while let Ok((ours, _)) = outer.accept().await {
                p.accepted.fetch_add(1, Ordering::Relaxed);
                let Ok(theirs) = TcpStream::connect(inner_addr).await else {
                    continue;
                };
                let (mut our_r, mut our_w) = ours.into_split();
                let (mut their_r, mut their_w) = theirs.into_split();
                let p1 = Arc::clone(&p);
                tokio::spawn(async move {
                    let mut buf = [0u8; 8192];
                    loop {
                        if p1.stall.load(Ordering::Relaxed) {
                            tokio::time::sleep(Duration::from_millis(10)).await;
                            continue;
                        }
                        match our_r.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                if their_w.write_all(&buf[..n]).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                });
                let (p2, fwd) = (Arc::clone(&p), Arc::clone(&forwarded));
                tokio::spawn(async move {
                    let mut buf = [0u8; 8192];
                    loop {
                        let n = match their_r.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => n,
                        };
                        // The mode is read again for every byte, so a switch applies at once.
                        let mut rest = &buf[..n];
                        while !rest.is_empty() {
                            let ms = p2.trickle_ms.load(Ordering::Relaxed);
                            let step = if ms == 0 { rest.len() } else { 1 };
                            if ms > 0 {
                                tokio::time::sleep(Duration::from_millis(ms)).await;
                            }
                            let (chunk, tail) = rest.split_at(step);
                            if our_w.write_all(chunk).await.is_err() {
                                return;
                            }
                            fwd.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                            rest = tail;
                        }
                    }
                });
            }
        });
        let tick = Arc::clone(&pm);
        tokio::spawn(async move {
            loop {
                tick.process_events();
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        });
        Self {
            pm,
            script,
            proxy,
            addr,
        }
    }

    fn send(&self, msgs: impl IntoIterator<Item = Raw>) {
        self.script.queue.lock().unwrap().extend(msgs);
        self.pm.process_events();
    }

    fn queued(&self) -> usize {
        self.script.queue.lock().unwrap().len()
    }

    fn connected(&self) -> bool {
        !self.pm.list_peers().is_empty()
    }

    /// Waits until everything queued was read by us.
    async fn drained(&self) {
        wait_for("the hostile queue to drain", || self.queued() == 0).await;
    }
}

/// Our peer, dialling `peers`, checking channels against `node`, ticking fast.
fn ours(dir: &TempDir, peers: Vec<PeerAddr>, node: &FakeChain) -> Peer {
    let mut config = Config::new(NET, dir.0.clone());
    config.max_peers = peers.len();
    config.peers = peers;
    config.backoff_min = Duration::from_millis(50);
    config.sync_settle = Duration::from_millis(300);
    config.lookup_interval = Duration::from_millis(20);
    Peer::start(config, node.clone()).unwrap().0
}

/// A real channel at height 100 on `node` (tip 200) between nodes `a` and `b`.
fn real(node: &FakeChain, index: u32, a: u8, b: u8) -> u64 {
    let id = scid(100, index, 0);
    node.add(id, &funding(a, b), 1_000_000, false);
    id
}

fn status(peer: &Peer) -> Status {
    peer.status().borrow().clone()
}

/// Connected, as our side sees it.
fn up(peer: &Peer, addr: PeerAddr) -> bool {
    status(peer)
        .peers
        .iter()
        .any(|p| p.peer == addr && p.connected)
}

fn outputs_asked(node: &FakeChain) -> usize {
    node.batches().iter().map(|b| b.1).sum()
}

/// A real channel's announcement and a marker sent after the attack: once it is in the graph,
/// everything before it was handled.
async fn mark(h: &Hostile, peer: &Peer, node: &FakeChain, index: u32) {
    let id = real(node, index, 40, 41);
    h.send([raw(CHANNEL_ANNOUNCEMENT, &channel(id, 40, 41))]);
    wait_for("the marker channel", || {
        peer.read(|g| g.channel(id).is_some())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_signatures_and_false_outputs_cost_one_lookup_each_at_most() {
    let dir = TempDir::new();
    let h = Hostile::start().await;
    let node = FakeChain::at(200);
    let peer = ours(&dir, vec![h.addr], &node);
    wait_for("connected", || up(&peer, h.addr)).await;

    // Four valid signatures over a different channel id: every one is refused by LDK before any
    // lookup.
    let signed = channel(scid(100, 999, 0), 1, 2);
    h.send((0..2000u32).map(|i| {
        let mut a = signed.clone();
        a.contents.short_channel_id = scid(100, 5000 + i, 0);
        raw(CHANNEL_ANNOUNCEMENT, &a)
    }));
    // Good signatures over outputs that are missing, spent or pay another script, each sent 500
    // times. Announcements carry no amount: the capacity is always the output's own value.
    let (missing, spent, wrong) = (scid(100, 1, 0), scid(100, 2, 0), scid(100, 3, 0));
    node.add(spent, &funding(1, 2), 1_000, true);
    node.add(wrong, &funding(3, 4), 1_000, false);
    let three = [missing, spent, wrong].map(|id| raw(CHANNEL_ANNOUNCEMENT, &channel(id, 1, 2)));
    h.send((0..500).flat_map(|_| three.clone()));
    // Absurd ids: a height past any tip, the largest index and output, block 0.
    let absurd = [
        scid(0x00ff_ffff, 0, 0),
        scid(150, 0x00ff_ffff, 0xffff),
        scid(0, 0, 0),
    ];
    h.send(absurd.map(|id| raw(CHANNEL_ANNOUNCEMENT, &channel(id, 1, 2))));
    h.drained().await;
    mark(&h, &peer, &node, 9).await;

    // The status is refreshed every second.
    wait_for("the status", || status(&peer).channels == 1).await;
    let s = status(&peer);
    let r = s.validation.rejected;
    // One output each for the three false channels, the three absurd ones and the marker; the
    // forgeries and the repeats cost nothing.
    assert_eq!(outputs_asked(&node), 7, "{:?}", node.batches());
    assert_eq!((r.missing, r.spent), (4, 1), "{r:?}");
    assert!(r.wrong_script >= 1, "{r:?}");
    assert_eq!(s.channels, 1);
    assert_eq!(s.validation.early, 0);
    let t = totals(&peer);
    assert!(t.rejected >= 2000, "{t:?}");
    assert!(h.connected() && up(&peer, h.addr));
    peer.shutdown().await.unwrap();
}

fn totals(peer: &Peer) -> Totals {
    crate::peer::totals(peer.gossip())
}

/// A node announcement for node `n` signed by its key, with `alias` as raw bytes.
fn node_raw(n: u8, alias: &[u8], addresses: Vec<SocketAddress>, excess: usize) -> NodeAnnouncement {
    let mut a = [0u8; 32];
    a[..alias.len()].copy_from_slice(alias);
    let mut features = NodeFeatures::empty();
    // The largest legal feature field fills what the size limit leaves.
    features.set_optional_custom_bit(255 * 8 + 1).unwrap();
    let contents = UnsignedNodeAnnouncement {
        features,
        timestamp: now(),
        node_id: id(&key(n)),
        rgb: [0xff, 0, 0],
        alias: NodeAlias(a),
        addresses,
        excess_address_data: Vec::new(),
        excess_data: vec![0xaa; excess],
    };
    NodeAnnouncement {
        signature: sign(&contents, &key(n)),
        contents,
    }
}

fn v4(i: u16) -> SocketAddress {
    SocketAddress::TcpIpV4 {
        addr: [10, 0, (i >> 8) as u8, i as u8],
        port: 9735,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hostile_node_announcements_are_kept_bounded_and_as_text() {
    let dir = TempDir::new();
    let h = Hostile::start().await;
    let node = FakeChain::at(200);
    let peer = ours(&dir, vec![h.addr], &node);
    wait_for("connected", || up(&peer, h.addr)).await;
    // Nodes 1 and 2 have a real channel; node 5 has none.
    let id = real(&node, 1, 1, 2);
    h.send([raw(CHANNEL_ANNOUNCEMENT, &channel(id, 1, 2))]);
    wait_for("the channel", || peer.read(|g| g.channel(id).is_some())).await;

    let markup = b"<script>x</script>\x1b[31m\x07\xe2\x80\xaeevil";
    let invalid = [0xffu8, 0xfe, b'<', b'b', b'>', 0x80, 0x00, b'z'];
    h.send([
        // Control characters, ANSI, a right-to-left override and markup, in 32 bytes.
        raw(NODE_ANNOUNCEMENT, &node_raw(1, markup, vec![v4(1)], 0)),
        // Invalid UTF-8, with 200 addresses and 60 KB of extra data: over the size limit, so kept
        // with eight addresses and no extra data.
        raw(
            NODE_ANNOUNCEMENT,
            &node_raw(2, &invalid, (0..200).map(v4).collect(), 60_000),
        ),
        // No channel: not kept at all.
        raw(NODE_ANNOUNCEMENT, &node_raw(5, b"orphan", vec![v4(5)], 0)),
    ]);
    h.drained().await;
    mark(&h, &peer, &node, 9).await;
    wait_for("both announcements", || {
        peer.read(|g| {
            [1, 2].iter().all(|n| {
                g.node(&id_of(*n))
                    .is_some_and(|n| n.announcement_info.is_some())
            })
        })
    })
    .await;
    peer.read(|g| {
        let info = |n: u8| {
            g.node(&id_of(n))
                .unwrap()
                .announcement_info
                .clone()
                .unwrap()
        };
        // LDK prints control characters as U+FFFD and keeps the rest: markup and the override
        // reach the server as text, which cleans them (`crates/server/src/lightning/index.rs`).
        let one = info(1).alias().to_string();
        // LDK's printing also turns the right-to-left override into U+FFFD.
        assert_eq!(one, "<script>x</script>\u{fffd}[31m\u{fffd}\u{fffd}evil");
        // Invalid UTF-8: printable ASCII kept, every other byte U+FFFD, up to the first NUL.
        assert_eq!(info(2).alias().to_string(), "\u{fffd}\u{fffd}<b>\u{fffd}");
        assert_eq!(info(2).addresses().len(), MAX_NODE_ADDRESSES);
        assert!(g.node(&id_of(5)).is_none());
        // Two channels' nodes plus the marker's: nothing else entered the graph.
        assert_eq!(g.nodes().len(), 4);
    });
    let t = totals(&peer);
    assert_eq!(t.trimmed, 1, "{t:?}");
    assert!(h.connected());
    peer.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channel_updates_cannot_reach_the_node_or_grow_the_graph() {
    let dir = TempDir::new();
    let h = Hostile::start().await;
    let node = FakeChain::at(200);
    let peer = ours(&dir, vec![h.addr], &node);
    wait_for("connected", || up(&peer, h.addr)).await;
    let id = real(&node, 1, 1, 2);
    h.send([raw(CHANNEL_ANNOUNCEMENT, &channel(id, 1, 2))]);
    wait_for("the channel", || peer.read(|g| g.channel(id).is_some())).await;
    let asked = outputs_asked(&node);

    let signer = |dir: u8| {
        if id_of(1) == channel(id, 1, 2).contents.node_id_1 {
            [1, 2][usize::from(dir)]
        } else {
            [2, 1][usize::from(dir)]
        }
    };
    let at = |scid: u64, ts: u32, flags: u8| {
        let mut u = update(scid, signer(flags & 1), flags & 1);
        u.contents.timestamp = ts;
        u.contents.channel_flags = flags;
        u.signature = sign(&u.contents, &key(signer(flags & 1)));
        raw(CHANNEL_UPDATE, &u)
    };
    // Unknown channels, a timestamp two days ahead and one three weeks old.
    h.send((0..1000u32).map(|i| at(scid(100, 100 + i, 0), now(), 0)));
    h.send([
        at(id, now() + 2 * 86_400, 0),
        at(id, now() - 21 * 86_400, 0),
    ]);
    // Flapping enabled and disabled a thousand times, a second apart, as far ahead as LDK allows.
    let start = now();
    h.send((0..1000u32).map(|i| at(id, start + i, ((i % 2) << 1) as u8)));
    h.drained().await;
    mark(&h, &peer, &node, 9).await;

    // The marker's own lookup, nothing for the updates.
    assert_eq!(outputs_asked(&node), asked + 1);
    peer.read(|g| {
        assert_eq!(g.channels().len(), 2);
        let c = g.channel(id).unwrap();
        // The newest of the flapping updates won; the one far ahead did not.
        assert_eq!(c.one_to_two.as_ref().unwrap().last_update, start + 999);
        assert!(c.two_to_one.is_none());
    });
    let t = totals(&peer);
    assert!(t.rejected >= 1002, "{t:?}");
    assert!(h.connected());
    peer.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn protocol_abuse_costs_one_connection_and_never_another_peer() {
    let dir = TempDir::new();
    let h = Hostile::start().await;
    let honest = fake().await;
    let node = FakeChain::at(200);
    node.add(7, &funding(1, 2), 50_000, false);
    let peer = ours(&dir, vec![honest.addr, h.addr], &node);
    wait_for("both connected", || {
        up(&peer, h.addr) && up(&peer, honest.addr)
    })
    .await;
    let first = h.script.connections.load(Ordering::Relaxed);

    // An unknown odd type is ignored (BOLT 1), a megabyte of it included.
    h.send((0..16).map(|_| Raw {
        ty: 32_769,
        body: vec![7; 65_000],
    }));
    h.drained().await;
    mark(&h, &peer, &node, 9).await;
    assert_eq!(h.script.connections.load(Ordering::Relaxed), first);

    // Gossip queries aimed at us: each range query gets one empty reply, id queries nothing.
    let range = QueryChannelRange {
        chain_hash: chain(),
        first_blocknum: 0,
        number_of_blocks: u32::MAX,
    };
    let ids = QueryShortChannelIds {
        chain_hash: chain(),
        short_channel_ids: (0..8000).collect(),
    };
    h.send((0..2000).map(|_| raw(QUERY_CHANNEL_RANGE, &range)));
    h.send((0..50).map(|_| raw(QUERY_SHORT_CHANNEL_IDS, &ids)));
    h.drained().await;
    wait_for("the replies", || {
        h.script.range_replies.load(Ordering::Relaxed) >= 2000
    })
    .await;
    assert_eq!(h.script.range_replies.load(Ordering::Relaxed), 2000);
    assert_eq!(h.script.range_reply_ids.load(Ordering::Relaxed), 0);
    assert!(h.connected());

    // Pings asking for the largest pong, while the hostile peer reads nothing: we stop reading
    // from it once a megabyte of pongs waits (LDK), and drop it when its pong to our own ping
    // never comes.
    h.proxy.stall.store(true, Ordering::Relaxed);
    let ping = Ping {
        ponglen: 65_531,
        byteslen: 0,
    };
    h.send((0..5000).map(|_| raw(PING, &ping)));
    let mut events = peer.events();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Ok(Event::PeerDisconnected { peer, .. }) = events.recv().await
                && peer == h.addr
            {
                break;
            }
        }
    })
    .await
    .expect("the stalled peer dropped");
    assert!(up(&peer, honest.addr));
    h.proxy.stall.store(false, Ordering::Relaxed);
    h.script.queue.lock().unwrap().clear();

    // An unknown even type closes the connection (BOLT 1); it is dialled again after a backoff.
    wait_for("reconnected", || up(&peer, h.addr)).await;
    let n = h.script.connections.load(Ordering::Relaxed);
    h.send([Raw {
        ty: 32_768,
        body: vec![1, 2, 3],
    }]);
    wait_for("dropped and dialled again", || {
        h.script.connections.load(Ordering::Relaxed) > n
    })
    .await;
    assert!(up(&peer, honest.addr));
    let _ = Pong { byteslen: 0 };
    peer.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slow_and_flapping_peers_are_dropped_and_backed_off() {
    let dir = TempDir::new();
    let h = Hostile::start().await;
    let honest = fake().await;
    let node = FakeChain::at(200);
    node.add(7, &funding(1, 2), 50_000, false);

    // One byte every 300 ms: the handshake (50 bytes from it) never completes in time.
    h.proxy.trickle_ms.store(300, Ordering::Relaxed);
    let peer = ours(&dir, vec![honest.addr, h.addr], &node);
    wait_for("the honest peer", || up(&peer, honest.addr)).await;
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(h.script.connections.load(Ordering::Relaxed), 0);
    assert!(h.proxy.accepted.load(Ordering::Relaxed) >= 2);
    assert!(up(&peer, honest.addr));

    // A peer that drops every connection right after the handshake is dialled again with a
    // doubling delay: 50 ms, 100 ms, 200 ms... (on from the failures above), so a few times in
    // three seconds, not hundreds.
    h.proxy.trickle_ms.store(0, Ordering::Relaxed);
    h.script.refuse.store(true, Ordering::Relaxed);
    let accepted = || h.proxy.accepted.load(Ordering::Relaxed);
    let before = accepted();
    wait_for("dialled again", || accepted() > before).await;
    let before = accepted();
    tokio::time::sleep(Duration::from_secs(3)).await;
    let dials = accepted() - before;
    assert!(dials <= 8, "{dials}");
    assert!(up(&peer, honest.addr));
    peer.shutdown().await.unwrap();
}

/// Until aborted: four new invented channels every 20 ms (`invent`), and a piece of a channel list
/// that never ends every 200 ms (`list`).
fn hold_unsynced(h: &Arc<Hostile>, invent: bool, list: bool) -> tokio::task::JoinHandle<()> {
    let h = Arc::clone(h);
    tokio::spawn(async move {
        for i in 0u32.. {
            if invent {
                h.send((0..4).map(|j| {
                    raw(
                        CHANNEL_ANNOUNCEMENT,
                        &channel(scid(100, 10_000 + i * 4 + j, 0), 1, 2),
                    )
                }));
            }
            if list && i % 10 == 0 {
                let reply = ReplyChannelRange {
                    chain_hash: chain(),
                    first_blocknum: 0,
                    number_of_blocks: 1000,
                    sync_complete: false,
                    short_channel_ids: (0..8000).map(|j| scid(50, i * 8000 + j, 1)).collect(),
                };
                h.send([raw(REPLY_CHANNEL_RANGE, &reply)]);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invented_channels_cannot_hold_the_graph_unsynced() {
    synced_despite(true, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_endless_channel_list_cannot_hold_the_graph_unsynced() {
    synced_despite(false, true).await;
}

/// The honest peer's graph is shown although the hostile peer keeps sending.
async fn synced_despite(invent: bool, list: bool) {
    let dir = TempDir::new();
    let honest = fake().await;
    let h = Arc::new(Hostile::start().await);
    let node = FakeChain::at(200);
    node.add(7, &funding(1, 2), 50_000, false);
    // A node taking 100 ms a request is asked every 400 ms: the trickle is never all answered.
    *node.0.delay.lock().unwrap() = Duration::from_millis(100);
    let attack = hold_unsynced(&h, invent, list);
    let peer = ours(&dir, vec![honest.addr, h.addr], &node);
    wait_for("the honest channel", || {
        peer.read(|g| g.channel(7).is_some())
    })
    .await;
    // Without the attack this takes about two seconds.
    let synced = |peer: &Peer| {
        let status = peer.status();
        async move {
            tokio::time::timeout(Duration::from_secs(15), async {
                while !status.borrow().synced {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .expect("synced despite the hostile peer");
        }
    };
    synced(&peer).await;
    if invent {
        // Restarted from the graph it saved, under the same attack: the stored channels are
        // checked again and the replay ends, though the queue is never empty.
        peer.shutdown().await.unwrap();
        let peer = ours(&dir, vec![honest.addr, h.addr], &node);
        synced(&peer).await;
        let s = status(&peer);
        assert_eq!((s.validation.unchecked, s.channels), (0, 1));
        assert!(!peer.gossip().replaying());
        peer.shutdown().await.unwrap();
        attack.abort();
        return;
    }
    assert!(up(&peer, h.addr) && up(&peer, honest.addr));
    // A peer's channel list is capped at the ceiling, whatever it claims.
    assert!(peer.gossip().range_pending() <= 2 * 100_000 + 2);
    attack.abort();
    peer.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_lookup_that_never_ends_cannot_hold_the_replay_open() {
    let id = scid(100, 1, 0);
    let node = FakeChain::at(1000);
    node.add(id, &funding(1, 2), 5_000_000, false);
    let config = quick(|_| {});
    let (g, _, _) = gossip(&config, Replay::from_graph(&stored_graph(id)));
    tokio::spawn(chain::run(Arc::clone(&g), node.clone(), pace(&config)));
    g.replay_tick(config.max_pending);
    wait_for("the stored channel", || {
        g.graph().read_only().channel(id).is_some()
    })
    .await;
    // A peer's invented channel waits for a node that no longer answers: the queue is never
    // empty again, as under a steady trickle of them.
    node.0.down.store(true, Ordering::Relaxed);
    let _ = g.handle_channel_announcement(None, &channel(scid(100, 2, 0), 3, 4));
    assert_eq!(g.lookup.waiting(), 1);
    g.replay_tick(config.max_pending);
    assert!(!g.replaying(), "every stored channel was answered");
}

// Floods, at the production settings. Each prints what it measured and lasts under two minutes.
// cargo test -p ln --release -- --ignored --nocapture hostile::flood

/// This process's resident memory, MB (`ps`; the hostile peer's own buffers included).
fn rss_mb() -> f64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<f64>()
        .unwrap_or(0.0)
        / 1024.0
}

/// Our peer at the default settings, dialling `peers`.
fn ours_default(dir: &TempDir, peers: Vec<PeerAddr>, node: &FakeChain) -> Peer {
    let mut config = Config::new(NET, dir.0.clone());
    config.max_peers = peers.len();
    config.peers = peers;
    Peer::start(config, node.clone()).unwrap().0
}

fn report(what: &str, peer: &Peer, node: &FakeChain, started: std::time::Instant, rss0: f64) {
    let s = status(peer);
    let t = totals(peer);
    let secs = started.elapsed().as_secs_f64();
    let batches = node.batches();
    println!(
        "{what}: {secs:.1} s; gossip received {} ({:.0}/s, {:.1} MB); node requests {} ({:.2}/s), outputs {}; \
         pending {} early {}; rejected {:?}; ldk rejected {} duplicate {} held {}; graph {} channels {} nodes; \
         rss {:.0} MB (+{:.0}); peers up {}",
        t.messages(),
        t.messages() as f64 / secs,
        t.bytes as f64 / 1e6,
        batches.len(),
        batches.len() as f64 / secs,
        outputs_asked(node),
        s.validation.pending,
        s.validation.early,
        s.validation.rejected,
        t.rejected,
        t.duplicate,
        t.held,
        s.channels,
        s.nodes,
        rss_mb(),
        rss_mb() - rss0,
        s.peers.iter().filter(|p| p.connected).count(),
    );
}

/// Feeds `make(i)` for i = 0.. to the hostile peer for `secs`, keeping its queue short, and
/// records the most pending lookups seen.
async fn feed(
    h: &Hostile,
    peer: &Peer,
    secs: u64,
    make: impl Fn(u32) -> Raw + Send + Sync + 'static,
) -> usize {
    let make = Arc::new(make);
    let until = std::time::Instant::now() + Duration::from_secs(secs);
    let (mut i, mut most) = (0u32, 0);
    while std::time::Instant::now() < until {
        if h.queued() < 20_000 {
            let m = Arc::clone(&make);
            let from = i;
            let batch = tokio::task::spawn_blocking(move || {
                (from..from + 2000).map(|j| m(j)).collect::<Vec<_>>()
            })
            .await
            .unwrap();
            h.send(batch);
            i += 2000;
        }
        most = most.max(peer.gossip().lookup.waiting());
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    h.script.queue.lock().unwrap().clear();
    most
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "flood: run with --release --ignored"]
async fn flood_one_announcement_repeated() {
    let dir = TempDir::new();
    let h = Hostile::start().await;
    let node = FakeChain::at(200);
    let peer = ours_default(&dir, vec![h.addr], &node);
    wait_for("connected", || up(&peer, h.addr)).await;
    let rss0 = rss_mb();
    let started = std::time::Instant::now();
    let one = raw(CHANNEL_ANNOUNCEMENT, &channel(scid(100, 1, 0), 1, 2));
    // About 7,500 a second are checked (four signatures each), so the run stops at 100 s.
    for _ in 0..100 {
        h.send(std::iter::repeat_n(one.clone(), 10_000));
        while h.queued() > 20_000 && started.elapsed() < Duration::from_secs(100) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    h.script.queue.lock().unwrap().clear();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    report(
        "one announcement repeated, 100 s",
        &peer,
        &node,
        started,
        rss0,
    );
    assert_eq!(outputs_asked(&node), 1);
    peer.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "flood: run with --release --ignored"]
async fn flood_bad_signatures() {
    let dir = TempDir::new();
    let h = Hostile::start().await;
    let node = FakeChain::at(200);
    let peer = ours_default(&dir, vec![h.addr], &node);
    wait_for("connected", || up(&peer, h.addr)).await;
    let rss0 = rss_mb();
    let started = std::time::Instant::now();
    let signed = channel(scid(100, 1, 0), 1, 2);
    feed(&h, &peer, 30, move |i| {
        let mut a = signed.clone();
        a.contents.short_channel_id = scid(100, 10 + i, 0);
        raw(CHANNEL_ANNOUNCEMENT, &a)
    })
    .await;
    report("forged signatures, 30 s", &peer, &node, started, rss0);
    assert_eq!(outputs_asked(&node), 0);
    peer.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "flood: run with --release --ignored"]
async fn flood_distinct_invented_channels_beside_an_honest_peer() {
    let dir = TempDir::new();
    let h = Hostile::start().await;
    let other = Hostile::start().await;
    let node = FakeChain::at(200);
    let peer = ours_default(&dir, vec![h.addr, other.addr], &node);
    wait_for("connected", || up(&peer, h.addr) && up(&peer, other.addr)).await;
    let rss0 = rss_mb();
    let started = std::time::Instant::now();
    // Halfway, the other peer announces a real channel: how long until it is in the graph?
    let real_id = real(&node, 1, 40, 41);
    let honest = raw(CHANNEL_ANNOUNCEMENT, &channel(real_id, 40, 41));
    let flood = feed(&h, &peer, 30, |i| {
        raw(CHANNEL_ANNOUNCEMENT, &channel(scid(150, 10 + i, 0), 1, 2))
    });
    let watch = async {
        tokio::time::sleep(Duration::from_secs(10)).await;
        let sent = std::time::Instant::now();
        other.send([honest]);
        while !peer.read(|g| g.channel(real_id).is_some()) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        sent.elapsed()
    };
    let (most, latency) = tokio::join!(flood, watch);
    report("invented channels, 30 s", &peer, &node, started, rss0);
    println!("most pending {most}; the honest channel waited {latency:?}");
    let s = status(&peer);
    assert!(most <= 5_000 + 64);
    assert_eq!(s.peers.iter().filter(|p| p.connected).count(), 2);
    // At most four requests a second (the fake stamps each a few microseconds after the gate).
    let batches = node.batches();
    for w in batches.windows(2) {
        assert!(w[1].0 - w[0].0 >= Duration::from_millis(245));
    }
    peer.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "flood: run with --release --ignored"]
async fn flood_pings_and_queries_at_a_peer_that_never_reads() {
    let dir = TempDir::new();
    let h = Hostile::start().await;
    let other = fake().await;
    let node = FakeChain::at(200);
    node.add(7, &funding(1, 2), 50_000, false);
    let peer = ours_default(&dir, vec![h.addr, other.addr], &node);
    wait_for("connected", || up(&peer, h.addr) && up(&peer, other.addr)).await;
    let rss0 = rss_mb();
    let started = std::time::Instant::now();
    h.proxy.stall.store(true, Ordering::Relaxed);
    let ping = Ping {
        ponglen: 65_531,
        byteslen: 0,
    };
    let range = QueryChannelRange {
        chain_hash: chain(),
        first_blocknum: 0,
        number_of_blocks: u32::MAX,
    };
    let mut most_rss = rss0;
    let mut events = peer.events();
    let mut dropped = None;
    while started.elapsed() < Duration::from_secs(60) {
        if h.queued() < 50_000 {
            h.send((0..5_000).flat_map(|_| [raw(PING, &ping), raw(QUERY_CHANNEL_RANGE, &range)]));
        }
        most_rss = most_rss.max(rss_mb());
        while let Ok(e) = events.try_recv() {
            if let Event::PeerDisconnected {
                peer,
                connected_for,
            } = e
                && peer == h.addr
            {
                dropped.get_or_insert(connected_for);
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    report(
        "pings for 65 KB pongs and range queries, never read, 60 s",
        &peer,
        &node,
        started,
        rss0,
    );
    println!(
        "dropped after {dropped:?}; read {} KB of it; most rss {most_rss:.0} MB (+{:.0}); honest peer up {}",
        h.script.forwarded.load(Ordering::Relaxed) / 1024,
        most_rss - rss0,
        up(&peer, other.addr)
    );
    assert!(up(&peer, other.addr));
    peer.shutdown().await.unwrap();
}

// Against a real rbitcoin on regtest (`docs/REGTEST.md`): the chain tricks a fake cannot prove.
// REGTEST_PROJECT=<name> dev/regtest/regtest.py up && dev/regtest/regtest.py mine 110
// eval "$(dev/regtest/regtest.py env | sed 's/^/export /')"
// cargo test -p ln -- --ignored --nocapture hostile::regtest

fn regtest(args: &[&str]) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = std::process::Command::new("python3")
        .arg(root.join("dev/regtest/regtest.py"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// A JSON-RPC call to the regtest node; the answer's `result` as text.
fn rpc(method: &str, params: &str) -> String {
    let var = |k: &str| std::env::var(k).unwrap();
    let url = format!(
        "http://{}:{}/",
        var("REGTEST_RPC_HOST"),
        var("REGTEST_RPC_PORT")
    );
    let auth = format!(
        "{}:{}",
        var("REGTEST_RPC_USER"),
        var("REGTEST_RPC_PASSWORD")
    );
    let body = format!(r#"{{"jsonrpc":"1.0","id":1,"method":"{method}","params":{params}}}"#);
    let out = std::process::Command::new("curl")
        .args(["-s", "-u", &auth, "-d", &body, &url])
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains(r#""error":null"#), "{method}: {text}");
    text
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs a regtest node: see above"]
async fn regtest_chain_tricks() {
    use std::str::FromStr;
    let esplora =
        node::esplora::Esplora::new(&std::env::var("REGTEST_ESPLORA_URL").unwrap()).unwrap();
    let txid = |s: &str| lightning::bitcoin::Txid::from_str(s).unwrap();
    let place = async |t| {
        let p = esplora.tx_merkle_proof(t).await.unwrap().value;
        (p.block_height, p.pos)
    };

    // A real two-of-two of nodes 1 and 2, two blocks deep.
    let script = funding(1, 2);
    let address = lightning::bitcoin::Address::from_script(&script, NET).unwrap();
    let paid = txid(&regtest(&["send", &address.to_string(), "100000"]));
    regtest(&["mine", "2"]);
    let (h, i) = place(paid).await;
    let tx = esplora.tx_raw(paid).await.unwrap().value;
    let vout = tx
        .output
        .iter()
        .position(|o| o.script_pubkey == script)
        .unwrap() as u32;
    let real_id = scid(h, i, vout);
    // The coinbase that payment spent; an old coinbase nobody spent, paying the miner's script.
    let coin = tx.input[0].previous_output;
    let (ch, _) = place(coin.txid).await;
    let spent = scid(ch, 0, coin.vout);
    let tip = esplora.tip_height().await.unwrap().value;
    let wrong = scid(tip - 8, 0, 0);
    // Blocks we have not seen: three past the tip (early), and absurdly far.
    let future = scid(tip + 3, 1, 0);
    let absurd = scid(0x00ff_ffff, 0x00ff_ffff, 0xffff);

    let dir = TempDir::new();
    let h_peer = Hostile::start().await;
    let mut config = Config::new(NET, dir.0.clone());
    config.peers = vec![h_peer.addr];
    config.max_peers = 1;
    config.tip_every = Duration::from_secs(1);
    let (peer, _) = Peer::start(config, esplora.clone()).unwrap();
    wait_for("connected", || up(&peer, h_peer.addr)).await;
    let mut history = peer.history();
    h_peer.send(
        [real_id, spent, wrong, future, absurd]
            .map(|id| raw(CHANNEL_ANNOUNCEMENT, &channel(id, 1, 2))),
    );
    wait_for("the first answers", || {
        let v = status(&peer).validation;
        v.early == 2 && v.rejected.spent == 1 && v.rejected.wrong_script == 1
    })
    .await;
    assert_eq!(status(&peer).validation.rejected.missing, 1);

    // Four more blocks: the real channel is six deep and enters the graph with its value.
    regtest(&["mine", "4"]);
    wait_for("verified", || {
        peer.read(|g| g.channel(real_id).and_then(|c| c.capacity_sats)) == Some(100_000)
    })
    .await;
    // Four more: the block three past the old tip is five deep, and has no second transaction.
    regtest(&["mine", "4"]);
    wait_for("the future one refused", || {
        let v = status(&peer).validation;
        v.early == 0 && v.rejected.missing == 2
    })
    .await;

    // The funding block leaves the chain, and empty blocks replace it and the five on top: the
    // channel's output is gone.
    let hash = esplora.block_hash(h).await.unwrap().value;
    rpc("invalidateblock", &format!(r#"["{hash}"]"#));
    for _ in 0..8 {
        rpc("generateblock", r#"["raw(51)",[]]"#);
    }
    wait_for("the channel removed", || {
        peer.read(|g| g.channel(real_id).is_none())
    })
    .await;
    let mut undone = false;
    while let Ok(r) = history.try_recv() {
        undone |= matches!(r.change, History::OpenUndone { scid } if scid == real_id);
    }
    assert!(undone);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let s = status(&peer);
    println!(
        "regtest: validation {:?}; close-watcher requests {} reorgs {} undone {}",
        s.validation, s.closes.requests, s.closes.reorgs, s.closes.undone
    );
    peer.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_full_queue_cannot_starve_the_replay() {
    let id = scid(100, 1, 0);
    let node = FakeChain::at(1000);
    node.add(id, &funding(1, 2), 5_000_000, false);
    let config = quick(|c| c.max_pending = 10);
    let (g, _, _) = gossip(&config, Replay::from_graph(&stored_graph(id)));
    // A peer's invented channels fill the queue to its hard limit, and stay while the node is
    // down: as a flood keeps it full.
    node.0.down.store(true, Ordering::Relaxed);
    for i in 0..100 {
        let _ = g.handle_channel_announcement(None, &channel(scid(100, 10 + i, 0), 3, 4));
    }
    assert_eq!(g.lookup.waiting(), 10 + 64);
    tokio::spawn(chain::run(Arc::clone(&g), node.clone(), pace(&config)));
    // The stored channel is sent all the same: the replay has a share of its own.
    g.replay_tick(config.max_pending);
    assert_eq!(g.replay_left(), 0);
    node.0.down.store(false, Ordering::Relaxed);
    wait_for("the stored channel", || {
        g.graph().read_only().channel(id).is_some()
    })
    .await;
    g.replay_tick(config.max_pending);
    assert!(!g.replaying());
}

#[test]
fn a_peer_list_counts_its_batch_in_flight_against_the_cap() {
    let h = handler(5);
    let peer = key(7).public_key(&Secp256k1::new());
    let mut features = InitFeatures::empty();
    features.set_gossip_queries_optional();
    let init = Init {
        features,
        networks: None,
        remote_network_address: None,
    };
    h.gossip.peer_connected(peer, &init, false).unwrap();
    // Pieces of a list that stays open: the first five ids go out as a batch at once.
    let part = |ids: Vec<u64>| ReplyChannelRange {
        chain_hash: chain(),
        first_blocknum: 0,
        number_of_blocks: 100,
        sync_complete: false,
        short_channel_ids: ids,
    };
    h.gossip
        .handle_reply_channel_range(peer, part((10..15).collect()))
        .unwrap();
    h.gossip
        .handle_reply_channel_range(peer, part((20..25).collect()))
        .unwrap();
    // Five ids and the open list, not ten.
    assert_eq!(h.gossip.range_pending(), 5 + 1);
}
