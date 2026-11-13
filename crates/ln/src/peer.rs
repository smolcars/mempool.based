//! The peer: LDK's peer manager over tokio sockets, and the supervisor task that dials peers,
//! drives timers, saves and prunes the graph and reports status.

use std::collections::HashSet;
use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lightning::ln::peer_handler::{ErroringMessageHandler, IgnoringMessageHandler, PeerManager};
use lightning::sign::{EntropySource, KeysManager, NodeSigner, Recipient};
use lightning::util::ser::Writeable;
use tokio::net::TcpStream;
use tokio::sync::{Notify, broadcast, mpsc, watch};
use tokio::task::JoinHandle;

use crate::chain::{self, Chain};
use crate::closes::{self, Opened};
use crate::gossip::{Gossip, Graph, Replay};
use crate::log::LogSink;
use crate::pace::Pace;
use crate::socket::{self, Socket};
use crate::store::{self, GraphLoad};
use crate::{
    Change, Closes, Config, Event, PeerAddr, PeerStatus, PublicKey, ReadOnlyNetworkGraph, Record,
    Stats, Status, Totals, Validation,
};

pub(crate) type Pm = PeerManager<
    Socket,
    ErroringMessageHandler,
    Arc<Gossip>,
    IgnoringMessageHandler,
    Arc<LogSink>,
    IgnoringMessageHandler,
    Arc<KeysManager>,
    IgnoringMessageHandler,
>;

/// LDK's recommended period for `timer_tick_occurred`: pings and stale-peer checks. A peer that
/// has not completed the handshake by the second tick is dropped. Tests tick faster.
const TIMER_TICK: Duration = Duration::from_secs(if cfg!(test) { 1 } else { 10 });
/// LDK's staleness horizon: channels without an update this old are pruned.
const PRUNE_EVERY: Duration = Duration::from_secs(3600);
/// A saved graph younger than this is shown at once after a restart.
const FRESH_ON_DISK: Duration = Duration::from_secs(13 * 24 * 3600);
/// After a restart, gossip is requested from this long before the saved graph's time: LDK's own
/// window for peers it does not fully sync. A day would be most of the graph again, as nodes
/// refresh their channel updates about daily.
const RESUME_MARGIN_SECS: u64 = 3600;
/// The channel-range sync holds back [`Status::synced`] for at most this long after the first
/// peer connected: a peer whose channel list never ends, or that never sends what it listed,
/// cannot hold it for ever. Mainnet's first sync took four minutes. Tests wait less.
const RANGE_WAIT: Duration = Duration::from_secs(if cfg!(test) { 3 } else { 600 });
/// A connection that lasted this long resets the peer's failure count.
const STABLE: Duration = Duration::from_secs(60);

/// A running gossip peer. See the crate documentation.
pub struct Peer {
    shared: Arc<Shared>,
    stop: Arc<Notify>,
    task: JoinHandle<io::Result<u64>>,
}

struct Shared {
    config: Config,
    /// The gate every request to the node goes through.
    pace: Arc<Pace>,
    pm: Arc<Pm>,
    gossip: Arc<Gossip>,
    logger: Arc<LogSink>,
    node_id: PublicKey,
    status: watch::Sender<Status>,
    events: broadcast::Sender<Event>,
    changes: broadcast::Sender<Change>,
}

impl Peer {
    /// Loads or creates the identity seed and the graph, then starts dialling. Funding outputs are
    /// looked up through `chain`. Must be called inside a tokio runtime. Fails only if the data
    /// directory or the seed cannot be used.
    ///
    /// A loaded graph is not shown as it is: its channels are checked again against the chain
    /// and enter the graph as they pass ([`Validation::unchecked`] counts those still to send).
    pub fn start(config: Config, chain: impl Chain) -> io::Result<(Self, GraphLoad)> {
        std::fs::create_dir_all(&config.data_dir)?;
        let seed = store::load_or_create_key(&config.data_dir)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();

        let events = broadcast::channel(256).0;
        let changes = broadcast::channel(4096).0;
        let logger = Arc::new(LogSink::new(
            config.log_level,
            config.log_per_second,
            events.clone(),
        ));
        let (loaded, load) = store::load_graph(
            &config.data_dir,
            config.network,
            config.max_channels,
            &logger,
        );
        let caught_up_at = match &load {
            GraphLoad::Loaded { age, .. } => {
                SystemTime::now().checked_sub(*age).unwrap_or(UNIX_EPOCH)
            }
            _ => UNIX_EPOCH,
        };
        let (fresh_on_disk, resume_from) = match &load {
            GraphLoad::Loaded { age, channels, .. } if *age < FRESH_ON_DISK => {
                let saved_at = now.saturating_sub(*age).as_secs();
                let from = saved_at.saturating_sub(RESUME_MARGIN_SECS);
                (*channels > 0, u32::try_from(from).ok())
            }
            _ => (false, None),
        };
        // Answers saved beside the graph: only with a graph to replay, which is what asks them.
        let verified = loaded.as_ref().and_then(|_| {
            store::load_verified(&config.data_dir, config.network, config.max_channels)
        });
        let mut replay = loaded.as_ref().map(Replay::from_graph).unwrap_or_default();
        drop(loaded);
        if let Some(v) = &verified {
            replay.prefer(&v.channels.iter().map(|c| c.scid).collect());
        }
        let graph = Arc::new(Graph::new(config.network, Arc::clone(&logger)));
        let gossip = Arc::new(Gossip::new(
            &config,
            graph,
            Arc::clone(&logger),
            resume_from,
            replay,
            changes.clone(),
            events.clone(),
        ));
        if let Some(v) = &verified {
            gossip.lookup.restore(v);
            gossip.watch.resume(v.height, v.hash);
        }
        drop(verified);
        let pace = Arc::new(Pace::new(config.lookup_interval));
        let lookups = tokio::spawn(chain::run(
            Arc::clone(&gossip),
            chain.clone(),
            Arc::clone(&pace),
        ));
        let watcher = tokio::spawn(closes::run(
            Arc::clone(&gossip),
            chain,
            Arc::clone(&pace),
            config.tip_every,
        ));

        let keys = Arc::new(KeysManager::new(
            &seed,
            now.as_secs(),
            now.subsec_nanos(),
            true,
        ));
        let node_id = keys
            .get_node_id(Recipient::Node)
            .map_err(|()| io::Error::other("node key"))?;
        // Routing only: the channel handler is LDK's ErroringMessageHandler, which answers every
        // channel message (open_channel included) with an error. There is no channel manager.
        let pm = Arc::new(Pm::new_routing_only(
            Arc::clone(&gossip),
            u32::try_from(now.as_secs()).unwrap_or(u32::MAX),
            &keys.get_secure_random_bytes(),
            Arc::clone(&logger),
            keys,
        ));

        let status = watch::channel(Status {
            validation: Validation::default(),
            closes: Closes::default(),
            synced: false,
            nodes: 0,
            channels: 0,
            at_ceiling: false,
            peers: Vec::new(),
        })
        .0;
        let shared = Arc::new(Shared {
            config,
            pace,
            pm,
            gossip,
            logger,
            node_id,
            status,
            events,
            changes,
        });
        let stop = Arc::new(Notify::new());
        let task = tokio::spawn(supervise(
            Arc::clone(&shared),
            Arc::clone(&stop),
            [lookups, watcher],
            fresh_on_disk,
            caught_up_at,
        ));
        Ok((Self { shared, stop, task }, load))
    }

    /// Runs `request` to the node through the gate Lightning's own requests share: one at a
    /// time, at least [`Config::lookup_interval`] apart, the node left idle three times as long
    /// as each took. For other requests on Lightning's behalf, such as funding txids.
    pub async fn paced<F: std::future::Future>(&self, request: F) -> F::Output {
        self.shared.pace.run(request).await
    }

    #[cfg(test)]
    pub(crate) fn gossip(&self) -> &Arc<Gossip> {
        &self.shared.gossip
    }

    /// This peer's identity. It has no channels and is never announced.
    pub fn node_id(&self) -> PublicKey {
        self.shared.node_id
    }

    /// Reads the graph. Gossip waits while `f` runs, so keep it short.
    pub fn read<R>(&self, f: impl FnOnce(&ReadOnlyNetworkGraph<'_>) -> R) -> R {
        f(&self.shared.gossip.graph().read_only())
    }

    pub fn status(&self) -> watch::Receiver<Status> {
        self.shared.status.subscribe()
    }

    pub fn events(&self) -> broadcast::Receiver<Event> {
        self.shared.events.subscribe()
    }

    /// Graph changes from now on. Read the graph first, then apply changes.
    pub fn changes(&self) -> broadcast::Receiver<Change> {
        self.shared.changes.subscribe()
    }

    /// Opens and closes from now on. A receiver that lags misses records: read [`Peer::recent`]
    /// after the last sequence number it saw, and rescan the graph if that does not reach back.
    pub fn history(&self) -> broadcast::Receiver<Record> {
        self.shared.gossip.watch.subscribe()
    }

    /// The newest [`Config::history`] records after sequence number `after` (0 for all kept).
    pub fn recent(&self, after: u64) -> Vec<Record> {
        self.shared.gossip.watch.recent(after)
    }

    /// The channels open now (every watched funding output), as [`Opened`] records, and the
    /// sequence number they are current at: what [`Peer::history`] adds up to by that record. For
    /// a consumer that lagged past what [`Peer::recent`] keeps.
    pub fn open(&self) -> (u64, Vec<Opened>) {
        self.shared.gossip.watch.open()
    }

    /// Disconnects every peer and writes the graph. Returns the bytes written. Dropping the peer
    /// instead does the same in the background.
    pub async fn shutdown(mut self) -> io::Result<u64> {
        self.stop.notify_one();
        (&mut self.task).await.map_err(io::Error::other)?
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.stop.notify_one();
    }
}

struct Slot {
    peer: PeerAddr,
    /// The connection task while dialling or connected.
    task: Option<JoinHandle<()>>,
    state: State,
    failures: u32,
    next_try: Instant,
}

#[derive(PartialEq)]
enum State {
    Idle,
    Dialing,
    Up(Instant),
}

/// The delay before retrying a peer after `failures` failures in a row: `backoff_min` after the
/// first (or a stable connection), doubling with each one after.
pub(crate) fn backoff(config: &Config, failures: u32) -> Duration {
    config
        .backoff_min
        .saturating_mul(1u32 << failures.saturating_sub(1).min(16))
        .min(config.backoff_max)
}

/// The slot to dial next: an idle peer that is due, fewest failures first, then list order. A
/// peer that keeps failing thus gives way to the next one in the list.
fn pick(slots: &[Slot], now: Instant) -> Option<usize> {
    slots
        .iter()
        .enumerate()
        .filter(|(_, s)| s.state == State::Idle && s.next_try <= now)
        .min_by_key(|(i, s)| (s.failures, *i))
        .map(|(i, _)| i)
}

fn dial(
    shared: &Arc<Shared>,
    i: usize,
    peer: PeerAddr,
    done: mpsc::UnboundedSender<usize>,
) -> JoinHandle<()> {
    let shared = Arc::clone(shared);
    let timeout = shared.config.connect_timeout;
    tokio::spawn(async move {
        if let Ok(Ok(stream)) = tokio::time::timeout(timeout, TcpStream::connect(peer.addr)).await {
            socket::run(&shared.pm, &shared.gossip, peer.node_id, stream).await;
        }
        let _ = done.send(i);
    })
}

pub(crate) fn totals(g: &Gossip) -> Totals {
    let c = &g.counters;
    let get = |a: &std::sync::atomic::AtomicU64| a.load(Ordering::Relaxed);
    Totals {
        channel_announcements: get(&c.channel_announcements),
        node_announcements: get(&c.node_announcements),
        channel_updates: get(&c.channel_updates),
        bytes: get(&c.bytes),
        accepted: get(&c.accepted),
        duplicate: get(&c.duplicate),
        held: get(&c.held),
        rejected: get(&c.rejected),
        trimmed: get(&c.trimmed),
        oversize: get(&c.oversize),
        over_ceiling: get(&c.over_ceiling),
        withheld: get(&c.withheld),
    }
}

/// `answered`: every lookup waiting at the last check has been answered since.
fn validation(g: &Gossip, verified: usize, answered: bool) -> Validation {
    let l = g.lookup.snapshot();
    let unchecked = g.replay_left();
    Validation {
        verified,
        unchecked,
        pending: l.pending,
        early: l.early,
        rejected: l.rejected,
        caught_up: !g.replaying() && answered,
        node_failures: l.failures,
        requests: l.requests,
        outputs: l.outputs,
    }
}

/// Writes the graph with its modification time set to `caught_up_at`, the last moment the peer
/// was connected and caught up, which is where a restart resumes gossip from. A graph saved
/// after a week offline is thus not mistaken for a fresh one.
async fn save(shared: &Arc<Shared>, caught_up_at: SystemTime) -> io::Result<u64> {
    let graph = Arc::clone(shared.gossip.graph());
    let (dir, network) = (shared.config.data_dir.clone(), shared.config.network);
    let verified = shared.gossip.watch.saved();
    let started = Instant::now();
    let r = tokio::task::spawn_blocking(move || {
        let bytes = graph.encode();
        store::write_atomic(&dir.join(store::GRAPH_FILE), |f| {
            f.write_all(&bytes)?;
            f.set_modified(caught_up_at)
        })?;
        // Written after the graph: either file may be the older, and both stay true.
        if let Some(v) = verified {
            store::write_verified(&dir, network, &v)?;
        }
        Ok::<_, io::Error>(bytes.len() as u64)
    })
    .await
    .map_err(io::Error::other)
    .and_then(|r| r);
    let _ = shared.events.send(match &r {
        Ok(bytes) => Event::GraphSaved {
            bytes: *bytes,
            duration: started.elapsed(),
        },
        Err(e) => Event::GraphSaveFailed {
            error: e.to_string(),
        },
    });
    r
}

/// What a prune changed.
pub(crate) struct Pruned {
    /// Channels removed, or that lost a policy.
    pub(crate) channels: Vec<u64>,
    /// Nodes of removed channels: they lost a channel, or left with it.
    pub(crate) nodes: HashSet<crate::NodeId>,
    pub(crate) gone_channels: usize,
    pub(crate) gone_nodes: usize,
}

/// Drops policies without an update for two weeks before `now_unix`, and channels left without
/// policies, as LDK does, and finds every channel and node that changed.
pub(crate) fn prune_graph(graph: &Graph, now_unix: u64) -> Pruned {
    let shape = |c: &crate::ChannelInfo| (c.one_to_two.is_some(), c.two_to_one.is_some());
    let before: Vec<_> = graph
        .read_only()
        .channels()
        .unordered_iter()
        .map(|(scid, c)| (*scid, shape(c), c.node_one, c.node_two))
        .collect();
    graph.remove_stale_channels_and_tracking_with_time(now_unix);
    let ro = graph.read_only();
    let mut p = Pruned {
        channels: Vec::new(),
        nodes: HashSet::new(),
        gone_channels: 0,
        gone_nodes: 0,
    };
    for (scid, was, one, two) in before {
        match ro.channel(scid) {
            None => {
                p.gone_channels += 1;
                p.channels.push(scid);
                p.nodes.extend([one, two]);
            }
            Some(c) if shape(c) != was => p.channels.push(scid),
            Some(_) => {}
        }
    }
    p.gone_nodes = p.nodes.iter().filter(|n| ro.node(n).is_none()).count();
    p
}

/// Prunes the graph and reports the changes. Returns how many channels changed.
fn prune(shared: &Shared) -> usize {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let p = prune_graph(shared.gossip.graph(), now);
    let ro = shared.gossip.graph().read_only();
    for c in &p.channels {
        if ro.channel(*c).is_none() {
            shared.gossip.watch.forget(*c);
        }
        let _ = shared.changes.send(Change::Channel(*c));
    }
    drop(ro);
    for n in &p.nodes {
        let _ = shared.changes.send(Change::Node(*n));
    }
    if !p.channels.is_empty() {
        let _ = shared.events.send(Event::Pruned {
            channels: p.gone_channels,
            nodes: p.gone_nodes,
        });
    }
    shared.gossip.after_prune();
    p.channels.len()
}

async fn supervise(
    shared: Arc<Shared>,
    stop: Arc<Notify>,
    tasks: [JoinHandle<()>; 2],
    fresh_on_disk: bool,
    mut caught_up_at: SystemTime,
) -> io::Result<u64> {
    let config = &shared.config;
    let started = Instant::now();
    let mut slots: Vec<Slot> = config
        .peers
        .iter()
        .map(|p| Slot {
            peer: *p,
            task: None,
            state: State::Idle,
            failures: 0,
            next_try: started,
        })
        .collect();
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<usize>();
    // The first tick waits a second, so a caller that subscribes right after `start` sees it.
    let mut tick = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(1),
        Duration::from_secs(1),
    );
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let (mut last_timer, mut last_stats, mut last_save) = (started, started, started);
    let mut last_prune: Option<Instant> = None;
    let mut last_totals = totals(&shared.gossip);
    // Graph changes so far: accepted gossip plus prunes. Saved when it moved.
    let mut pruned = 0;
    let mut saved_changes = 0;
    let mut synced = false;
    let mut first_up: Option<Instant> = None;
    // Lookups waiting before this moment must be answered for validation to be caught up.
    let mut mark = Instant::now();

    loop {
        tokio::select! {
            () = stop.notified() => break,
            // Lookups resolved: let the peer manager read the gossip it held back.
            () = shared.gossip.lookup.resolved.notified() => shared.pm.process_events(),
            Some(i) = done_rx.recv() => {
                let now = Instant::now();
                let Some(s) = slots.get_mut(i) else { continue };
                match s.state {
                    State::Up(since) => {
                        let connected_for = now.duration_since(since);
                        s.failures = if connected_for >= STABLE { 0 } else { s.failures + 1 };
                        let _ = shared.events.send(Event::PeerDisconnected { peer: s.peer, connected_for });
                    }
                    _ => {
                        s.failures += 1;
                        let _ = shared.events.send(Event::PeerUnreachable { peer: s.peer, failures: s.failures });
                    }
                }
                s.state = State::Idle;
                s.task = None;
                s.next_try = now + backoff(config, s.failures);
            }
            _ = tick.tick() => {
                // The lookup worker and the close-watcher run until stopped; one that ended
                // panicked, and the peer cannot validate or watch without it.
                if tasks.iter().any(JoinHandle::is_finished) {
                    break;
                }
                let now = Instant::now();
                for s in &mut slots {
                    if s.state == State::Dialing && shared.pm.peer_by_node_id(&s.peer.node_id).is_some() {
                        s.state = State::Up(now);
                        first_up.get_or_insert(now);
                        let _ = shared.events.send(Event::PeerConnected(s.peer));
                    }
                }
                let mut active = slots.iter().filter(|s| s.state != State::Idle).count();
                while active < config.max_peers {
                    let Some(i) = pick(&slots, now) else { break };
                    let Some(s) = slots.get_mut(i) else { break };
                    s.state = State::Dialing;
                    s.task = Some(dial(&shared, i, s.peer, done_tx.clone()));
                    active += 1;
                }

                shared.pm.process_events();
                if now.duration_since(last_timer) >= TIMER_TICK {
                    shared.pm.timer_tick_occurred();
                    last_timer = now;
                }
                shared.logger.flush();
                shared.gossip.range_tick();
                shared.gossip.replay_tick(config.max_pending);

                let (nodes, channels) = {
                    let ro = shared.gossip.graph().read_only();
                    (ro.nodes().len(), ro.channels().len())
                };
                let answered = shared.gossip.lookup.answered_before(mark);
                if answered {
                    mark = now;
                }
                let validation = validation(&shared.gossip, channels, answered);
                let up = slots.iter().filter(|s| matches!(s.state, State::Up(_))).count();

                let settled = |t: Instant| now.duration_since(t) >= config.sync_settle;
                let quiet = shared.gossip.last_new_channel.lock().map(|t| settled(*t)).unwrap_or(false);
                let drained = shared.gossip.range_pending() == 0
                    || first_up.is_some_and(|t| now.duration_since(t) >= RANGE_WAIT);
                let caught_up = up > 0 && channels > 0 && first_up.is_some_and(settled) && quiet && drained
                    && validation.caught_up;
                if caught_up {
                    caught_up_at = SystemTime::now();
                }
                if !synced && validation.caught_up && (fresh_on_disk || caught_up) {
                    synced = true;
                    let _ = shared.events.send(Event::Synced {
                        after: now.duration_since(started), nodes, channels, from_disk: fresh_on_disk,
                    });
                }
                if synced && last_prune.is_none_or(|t| now.duration_since(t) >= PRUNE_EVERY) {
                    pruned += prune(&shared) as u64;
                    last_prune = Some(now);
                }

                if now.duration_since(last_stats) >= config.stats_every {
                    let t = totals(&shared.gossip);
                    let secs = now.duration_since(last_stats).as_secs_f64();
                    let _ = shared.events.send(Event::Stats(Stats {
                        nodes, channels, peers: up, totals: t,
                        pending_channels: shared.gossip.range_pending(),
                        validation,
                        messages_per_sec: t.messages().saturating_sub(last_totals.messages()) as f64 / secs,
                        bytes_per_sec: t.bytes.saturating_sub(last_totals.bytes) as f64 / secs,
                    }));
                    last_totals = t;
                    last_stats = now;
                }

                let changes = shared.gossip.counters.accepted.load(Ordering::Relaxed) + pruned;
                // While stored channels are still to be checked the graph lacks them, and the
                // file on disk is the better copy.
                let replaying = shared.gossip.replaying();
                if now.duration_since(last_save) >= config.save_every && changes != saved_changes && !replaying {
                    // A failure is reported as an event and retried at the next period.
                    if save(&shared, caught_up_at).await.is_ok() {
                        saved_changes = changes;
                    }
                    last_save = now;
                }

                shared.status.send_replace(Status {
                    validation,
                    closes: shared.gossip.watch.status(),
                    synced,
                    nodes,
                    channels,
                    at_ceiling: shared.gossip.at_ceiling(),
                    peers: slots.iter().map(|s| PeerStatus {
                        peer: s.peer,
                        connected: matches!(s.state, State::Up(_)),
                        failures: s.failures,
                    }).collect(),
                });
            }
        }
    }

    // A dial still connecting could otherwise register its peer after the disconnect below.
    for s in &mut slots {
        if let Some(task) = s.task.take()
            && matches!(s.state, State::Dialing)
        {
            task.abort();
            let _ = task.await;
        }
    }
    for t in tasks {
        t.abort();
    }
    shared.pm.disconnect_all_peers();
    if shared.gossip.replaying() {
        // Keep the file: it still holds the channels not yet checked again.
        return Ok(0);
    }
    save(&shared, caught_up_at).await
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn slot(failures: u32, state: State, next_try: Instant) -> Slot {
        Slot {
            peer: crate::default_peers(crate::Network::Bitcoin)[0],
            task: None,
            state,
            failures,
            next_try,
        }
    }

    #[test]
    fn backoff_doubles_up_to_the_cap() {
        let c = Config::new(crate::Network::Regtest, "x".into());
        assert_eq!(backoff(&c, 0), Duration::from_secs(2));
        assert_eq!(backoff(&c, 1), Duration::from_secs(2));
        assert_eq!(backoff(&c, 2), Duration::from_secs(4));
        assert_eq!(backoff(&c, 4), Duration::from_secs(16));
        assert_eq!(backoff(&c, 40), Duration::from_secs(600));
    }

    #[test]
    fn a_failing_peer_gives_way_to_the_next() {
        let now = Instant::now();
        let slots = [
            slot(3, State::Idle, now),
            slot(0, State::Up(now), now),
            slot(0, State::Idle, now),
            slot(0, State::Idle, now),
        ];
        assert_eq!(pick(&slots, now), Some(2));
        // Not yet due: nothing to dial.
        let later = now + Duration::from_secs(5);
        let waiting = [slot(1, State::Idle, later), slot(0, State::Dialing, now)];
        assert_eq!(pick(&waiting, now), None);
        // Once due, it is retried.
        assert_eq!(pick(&waiting, later), Some(0));
    }
}
