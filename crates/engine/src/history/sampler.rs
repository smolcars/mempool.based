//! The sampler: one task per network that takes a sample at every whole minute while the
//! mempool engine is live, with the flow it counted from the tracker's change stream since the
//! minute before, and stores it.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bitcoin::Txid;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;

use super::{Flow, MINUTE, Rolled, db, sample, since_epoch};
use crate::mempool::{Delta, Removal, Subscription, Tracker};
use crate::projected::Projector;
use crate::store::{Error, Store};

/// How far from its minute a sample may be read. Further, and the minute is skipped as late.
pub const ON_TIME: Duration = Duration::from_secs(5);

/// Why a minute has no sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// The engine has not completed its first cycle, or this is the first minute after it
    /// started or came back: the flow over it was not watched whole.
    Syncing,
    /// The node is failing: the projection holds the last good numbers, not the mempool now.
    Stale,
    /// The sampler fell more than the change stream's ring behind; the minute's flow is lost.
    Lagged,
    /// The sampler woke more than [`ON_TIME`] from the minute, or after the next one had
    /// begun: the state is not that minute's, and the flow would not span one minute.
    Late,
    /// The store's write queue was full.
    StoreBusy,
    /// The write failed.
    StoreFailed,
}

impl Skip {
    pub fn label(self) -> &'static str {
        match self {
            Self::Syncing => "syncing",
            Self::Stale => "stale",
            Self::Lagged => "lagged",
            Self::Late => "late",
            Self::StoreBusy => "store_busy",
            Self::StoreFailed => "store_failed",
        }
    }
}

/// What the sampler reports, once a minute. The server summarises it for the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The sample for minute `time` is stored; `rolled` rows aged into coarser ones in the same
    /// write, which took `elapsed`.
    Written {
        time: u64,
        rolled: Rolled,
        elapsed: Duration,
    },
    Skipped {
        time: u64,
        reason: Skip,
    },
}

pub struct Config {
    /// Wall-clock time since the Unix epoch. Samples are stamped and minutes cut by it.
    pub clock: Arc<dyn Fn() -> Duration + Send + Sync>,
    pub observer: Arc<dyn Fn(&Event) + Send + Sync>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            clock: Arc::new(since_epoch),
            observer: Arc::new(|_| {}),
        }
    }
}

/// Counts the flow on a tracker's change stream. It holds the virtual size of every
/// transaction in the mempool, since a removal carries only the txid: about 50 bytes each.
#[derive(Debug, Default)]
pub struct Watch {
    pub(super) sizes: HashMap<Txid, u32>,
    flow: Flow,
    /// The stream lagged since the last [`Watch::take`]: the flow is incomplete.
    lagged: bool,
}

impl Watch {
    /// Starts from a snapshot of the mempool's sizes, as `Tracker::subscribe` gives it.
    pub fn new(sizes: HashMap<Txid, u32>) -> Self {
        Self {
            sizes,
            ..Self::default()
        }
    }

    pub fn apply(&mut self, d: &Delta) {
        let f = &mut self.flow;
        for (txid, info) in &d.added {
            f.in_count += 1;
            f.in_vsize += u64::from(info.vsize);
            self.sizes.insert(*txid, info.vsize);
        }
        for (txid, why) in &d.removed {
            let Some(vsize) = self.sizes.remove(txid) else {
                continue;
            };
            f.out_count += 1;
            f.out_vsize += u64::from(vsize);
            if matches!(why, Removal::Confirmed { .. }) {
                f.mined_count += 1;
                f.mined_vsize += u64::from(vsize);
            }
        }
    }

    /// Starts over from a new snapshot after the stream lagged; the flow so far is incomplete.
    pub fn resync(&mut self, sizes: HashMap<Txid, u32>) {
        self.sizes = sizes;
        self.lagged = true;
    }

    /// The flow since the last call and whether it is whole, starting the next from zero.
    pub fn take(&mut self) -> (Flow, bool) {
        let whole = !std::mem::take(&mut self.lagged);
        (std::mem::take(&mut self.flow), whole)
    }
}

fn subscribe(tracker: &Tracker) -> (HashMap<Txid, u32>, Subscription) {
    tracker.subscribe(|m| m.iter().map(|(t, i)| (*t, i.vsize)).collect())
}

/// Starts sampling into `store`. Abort the task to stop.
pub fn start(
    tracker: Tracker,
    projector: Projector,
    store: Arc<Store>,
    config: Config,
) -> JoinHandle<()> {
    tokio::spawn(run(tracker, projector, store, config))
}

async fn run(tracker: Tracker, projector: Projector, store: Arc<Store>, config: Config) {
    let (sizes, mut sub) = subscribe(&tracker);
    let mut watch = Watch::new(sizes);
    // The last minute looked at, and whether the engine was live then.
    let mut last: Option<(u64, bool)> = None;
    loop {
        let now = (config.clock)();
        let next = Duration::from_secs((now.as_secs() / MINUTE + 1) * MINUTE);
        let sleep = tokio::time::sleep(next.saturating_sub(now));
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                () = &mut sleep => break,
                d = sub.recv() => match d {
                    Ok(d) => watch.apply(&d),
                    Err(RecvError::Lagged(_)) => {
                        let sizes;
                        (sizes, sub) = subscribe(&tracker);
                        watch.resync(sizes);
                    }
                    Err(RecvError::Closed) => return,
                },
            }
        }
        let p = projector.current();
        let min_fee = tracker.node_info().map(|i| i.mempoolminfee);
        let (flow, whole) = watch.take();
        // The clock after the reads, which can wait for a pack: the state must be the minute's.
        // The nearest whole minute, so a timer that fires a hair early still names this one.
        let millis = (config.clock)().as_millis();
        let minute = (millis + 30_000) / 60_000;
        let on_time = millis.abs_diff(minute * 60_000) <= ON_TIME.as_millis();
        let time = u64::try_from(minute)
            .unwrap_or(u64::MAX)
            .saturating_mul(MINUTE);
        if let Some((t, _)) = last.filter(|(t, _)| time <= *t) {
            // The clock went back: this minute was already looked at. Its flow is dropped, and
            // a whole minute past the last one is owed before sampling again.
            last = Some((t, false));
            continue;
        }
        let live = p.tip.is_some() && !p.stale && min_fee.is_some();
        let skip = match last {
            _ if p.stale => Some(Skip::Stale),
            _ if !live => Some(Skip::Syncing),
            None => Some(Skip::Syncing),
            _ if !on_time => Some(Skip::Late),
            Some((t, _)) if t + MINUTE != time => Some(Skip::Late),
            Some((_, false)) => Some(Skip::Syncing),
            _ if !whole => Some(Skip::Lagged),
            Some(_) => None,
        };
        let event = match (skip, min_fee) {
            (None, Some(min_fee)) => {
                let started = Instant::now();
                match db::record(&store, sample(time, &p, min_fee, flow), time).await {
                    Ok(rolled) => Event::Written {
                        time,
                        rolled,
                        elapsed: started.elapsed(),
                    },
                    Err(Error::Full) => Event::Skipped {
                        time,
                        reason: Skip::StoreBusy,
                    },
                    Err(_) => Event::Skipped {
                        time,
                        reason: Skip::StoreFailed,
                    },
                }
            }
            (reason, _) => Event::Skipped {
                time,
                reason: reason.unwrap_or(Skip::Syncing),
            },
        };
        (config.observer)(&event);
        // A late minute's flow ends off the minute, so the next one's starts off it: skip that too.
        last = Some((time, live && on_time));
    }
}
