//! A hostile Lightning peer, and what it can make our peer do (`docs/LIGHTNING.md`, Hostile
//! peers). Our peer dials it like any configured peer. It completes the real handshake (LDK's own
//! peer manager does the Noise part), then sends whatever a test queues: any message type with any
//! body, signed or not, as fast as our side reads. A proxy between the two can stop reading what we
//! send, or pass what it sends one byte at a time.
//!
//! The fast attacks run with the normal suite. The floods are `#[ignore]`d; run them with
//! `cargo test -p ln --release -- --ignored --nocapture hostile::flood`. Each lasts under two
//! minutes and prints what it measured.

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

type HostilePm = PeerManager<
    lightning_net_tokio::SocketDescriptor,
    ErroringMessageHandler,
    Arc<Script>,
    IgnoringMessageHandler,
    Arc<Lines>,
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
        let lines = Arc::new(Lines(Mutex::new(Vec::new())));
        let script = Arc::new(Script::default());
        let keys = Arc::new(KeysManager::new(&[66; 32], 1, 0, true));
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
            lines,
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
                        let ms = p2.trickle_ms.load(Ordering::Relaxed);
                        let step = if ms == 0 { n } else { 1 };
                        for chunk in buf[..n].chunks(step) {
                            if ms > 0 {
                                tokio::time::sleep(Duration::from_millis(ms)).await;
                            }
                            if our_w.write_all(chunk).await.is_err() {
                                return;
                            }
                            fwd.fetch_add(chunk.len() as u64, Ordering::Relaxed);
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
    // doubling delay: 50 ms, 100 ms, 200 ms... so a few times in three seconds, not hundreds.
    h.proxy.trickle_ms.store(0, Ordering::Relaxed);
    h.script.refuse.store(true, Ordering::Relaxed);
    let before = h.proxy.accepted.load(Ordering::Relaxed);
    tokio::time::sleep(Duration::from_secs(3)).await;
    let dials = h.proxy.accepted.load(Ordering::Relaxed) - before;
    assert!((1..=8).contains(&dials), "{dials}");
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
    let s = status(&peer);
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
