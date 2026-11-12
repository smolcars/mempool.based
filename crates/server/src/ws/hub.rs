//! Topics, their subscribers and snapshots, and the `blocks` feed from Electrum.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use bitcoin::BlockHash;
use node::electrum::{self, Tip};
use node::esplora::{Esplora, Route};
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;

use logging::events::{BlockUnavailable, WsClientsDropped, WsFanout};

use super::{Close, Conn, Frame, Sockets, Topic};
use crate::NetworkContext;
use crate::config::Network;

/// How often a new block is asked for while the node's Esplora has not caught up with its
/// Electrum announcement (docs/RBITCOIN.md): at most one request per interval.
const BLOCK_RETRY: Duration = Duration::from_millis(25);
/// How long a new block is waited for before clients are told to resync instead.
const BLOCK_WAIT: Duration = Duration::from_secs(3);

/// What a new subscriber to a topic is sent first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Snapshot {
    /// This message is the topic's current state.
    Replace,
    /// The current state is unknown until the next `Replace`.
    Clear,
    /// This message changes nothing a new subscriber needs.
    Keep,
    /// This message is a change to the current state: a new subscriber gets it after the state
    /// and the changes appended before it. [`Hub::set_snapshot`] folds them into a new state.
    Append,
}

/// The result of one [`Hub::publish`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fanout {
    pub subscribers: usize,
    pub dropped_slow: usize,
    pub elapsed: Duration,
}

/// One network's topics.
#[derive(Debug)]
pub struct Hub {
    network: Network,
    sockets: Arc<Sockets>,
    topics: Mutex<HashMap<Topic, TopicState>>,
    /// Clients dropped for each limit, by close code, for [`WsClientsDropped`].
    dropped: Mutex<HashMap<u16, u64>>,
}

#[derive(Debug, Default)]
struct TopicState {
    subscribers: HashMap<u64, Arc<Conn>>,
    /// The current state, then the changes appended since: what a new subscriber is sent.
    snapshot: Vec<Frame>,
}

impl Hub {
    pub fn new(network: Network, sockets: Arc<Sockets>) -> Self {
        Self {
            network,
            sockets,
            topics: Mutex::default(),
            dropped: Mutex::default(),
        }
    }

    fn topics(&self) -> std::sync::MutexGuard<'_, HashMap<Topic, TopicState>> {
        self.topics.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Adds `conn` and queues the topic's snapshot to it. Under the same lock as [`Hub::publish`],
    /// so the snapshot always comes before the changes that follow it.
    pub(super) fn subscribe(&self, topic: Topic, conn: &Arc<Conn>) {
        let mut topics = self.topics();
        let state = topics.entry(topic).or_default();
        state.subscribers.insert(conn.id, Arc::clone(conn));
        for frame in &state.snapshot {
            conn.send(frame.clone());
        }
    }

    /// Makes `frame` the topic's whole current state for new subscribers, without sending it to
    /// anyone: for folding appended changes into one state.
    pub fn set_snapshot(&self, topic: Topic, frame: Frame) {
        self.topics().entry(topic).or_default().snapshot = vec![frame];
    }

    /// Subscribers of `topic` now.
    pub fn subscribers(&self, topic: Topic) -> usize {
        self.topics().get(&topic).map_or(0, |s| s.subscribers.len())
    }

    /// The network this hub serves.
    pub fn network(&self) -> Network {
        self.network
    }

    /// Forgets a topic and its stored state: for a tracked item nobody follows any more.
    pub(super) fn remove(&self, topic: Topic) {
        self.topics().remove(&topic);
    }

    pub(super) fn unsubscribe(&self, topic: Topic, id: u64) {
        if let Some(state) = self.topics().get_mut(&topic) {
            state.subscribers.remove(&id);
        }
    }

    /// Queues `frame` to every subscriber of `topic`: one reference-count increment and one
    /// non-blocking queue push each. A subscriber whose queue is full is closed as too slow.
    pub fn publish(&self, topic: Topic, frame: &Frame, snapshot: Snapshot) -> Fanout {
        let started = Instant::now();
        let mut topics = self.topics();
        let state = topics.entry(topic).or_default();
        match snapshot {
            Snapshot::Replace => state.snapshot = vec![frame.clone()],
            Snapshot::Clear => state.snapshot.clear(),
            Snapshot::Keep => {}
            Snapshot::Append => state.snapshot.push(frame.clone()),
        }
        let dropped_slow = state
            .subscribers
            .values()
            .filter(|conn| !conn.send(frame.clone()))
            .count();
        Fanout {
            subscribers: state.subscribers.len(),
            dropped_slow,
            elapsed: started.elapsed(),
        }
    }

    /// Counts a client dropped for `reason`; logged when the count reaches a power of two.
    pub(super) fn dropped(&self, reason: Close) {
        let total = {
            let mut dropped = self.dropped.lock().unwrap_or_else(PoisonError::into_inner);
            let n = dropped.entry(reason.code).or_default();
            *n += 1;
            *n
        };
        if total.is_power_of_two() {
            logging::emit(&WsClientsDropped {
                network: self.network.name(),
                reason: reason.label,
                total,
            });
        }
    }

    /// [`Hub::publish`] for a message caused by a node notification received at `since`.
    pub(crate) fn announce(
        &self,
        topic: Topic,
        kind: &'static str,
        msg: &Value,
        snapshot: Snapshot,
        since: Instant,
    ) {
        let frame = Frame::text(msg.to_string().as_bytes());
        let fanout = self.publish(topic, &frame, snapshot);
        logging::emit(&WsFanout {
            network: self.network.name(),
            topic: topic.name(),
            kind,
            subscribers: fanout.subscribers,
            open: self.sockets.open(),
            bytes: frame.len(),
            dropped_slow: fanout.dropped_slow,
            duration_ms: since.elapsed().as_secs_f64() * 1000.0,
            fanout_ms: fanout.elapsed.as_secs_f64() * 1000.0,
        });
    }
}

/// Feeds the `blocks` topic from the network's Electrum events until they end. One task per
/// network; the only upstream work is one Esplora fetch loop per new block, whatever the client count.
pub async fn feed(
    ctx: Arc<NetworkContext>,
    mut events: tokio::sync::broadcast::Receiver<electrum::Event>,
) {
    let hub = &ctx.hub;
    let mut last = Last::default();
    // The first tip may have been announced before `events` was taken.
    if let Some(tip) = ctx.electrum.tip() {
        block(&ctx, &tip, Instant::now(), &mut last).await;
    }
    loop {
        let event = events.recv().await;
        let since = Instant::now();
        match event {
            Ok(electrum::Event::Block(tip)) => block(&ctx, &tip, since, &mut last).await,
            Ok(electrum::Event::Reorg { fork_height }) => {
                last.hash = None;
                let msg = json!({ "topic": "blocks", "type": "reorg", "fork_height": fork_height });
                hub.announce(Topic::Blocks, "reorg", &msg, Snapshot::Clear, since);
            }
            Ok(electrum::Event::Resynced) | Err(RecvError::Lagged(_)) => {
                // The last block is still a fair snapshot; a newer tip arrives as a `block`.
                resync(&ctx, since, Snapshot::Keep);
            }
            Ok(electrum::Event::Status { .. }) => {}
            Err(RecvError::Closed) => return,
        }
    }
}

/// What the feed remembers between events.
#[derive(Default)]
struct Last {
    /// The block last announced, so a repeated tip is not sent twice.
    hash: Option<BlockHash>,
    /// Blocks in a row whose JSON could not be fetched.
    failures: u32,
}

async fn block(ctx: &NetworkContext, tip: &Tip, since: Instant, last: &mut Last) {
    if last.hash == Some(tip.hash) {
        return;
    }
    last.hash = Some(tip.hash);
    match fetch_block(&ctx.esplora, tip.hash).await {
        Some(block) => {
            last.failures = 0;
            let msg = json!({ "topic": "blocks", "type": "block", "block": block });
            ctx.hub
                .announce(Topic::Blocks, "block", &msg, Snapshot::Replace, since);
            ctx.summaries.new_tip(tip.hash, since);
        }
        None => {
            last.failures = last.failures.saturating_add(1);
            if last.failures.is_power_of_two() {
                logging::emit(&BlockUnavailable {
                    network: ctx.config.network.name(),
                    height: tip.height,
                    hash: tip.hash.to_string(),
                    failures: last.failures,
                });
            }
            // A later subscriber must learn it missed this block, not get the one before.
            resync(ctx, since, Snapshot::Replace);
        }
    }
}

fn resync(ctx: &NetworkContext, since: Instant, snapshot: Snapshot) {
    let tip = ctx
        .electrum
        .tip()
        .map(|t| json!({ "height": t.height, "hash": t.hash }));
    let msg = json!({ "topic": "blocks", "type": "resync", "tip": tip });
    ctx.hub
        .announce(Topic::Blocks, "resync", &msg, snapshot, since);
}

/// The block's Esplora JSON, checked to be the block we asked for. Asked for every
/// [`BLOCK_RETRY`] until [`BLOCK_WAIT`] has passed, so at most 120 requests per new block. The
/// full fetch doubles as the existence check: a missing block is a short 404, and the answer is
/// about 500 bytes, so a separate check would only add a round trip once the block is there.
async fn fetch_block(esplora: &Esplora, hash: BlockHash) -> Option<Value> {
    let id = hash.to_string();
    let once = async || {
        let raw = esplora.raw(&Route::Block(hash)).await.ok()?;
        if !raw.status.is_success() {
            return None;
        }
        let block = serde_json::from_slice::<Value>(&raw.body).ok()?;
        (block.get("id").and_then(Value::as_str) == Some(id.as_str())).then_some(block)
    };
    let retry = async {
        loop {
            let started = tokio::time::Instant::now();
            if let Some(block) = once().await {
                return block;
            }
            tokio::time::sleep_until(started + BLOCK_RETRY).await;
        }
    };
    tokio::time::timeout(BLOCK_WAIT, retry).await.ok()
}
