//! The snapshot loop: at every whole ten minutes of the wall clock, takes the network's totals
//! from the graph and stores them.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{EVERY, Point, Stats, db};
use crate::history::{Rolled, since_epoch};
use crate::store::{Error, Store};

/// How far past its ten minutes a snapshot may be taken. Later, it is skipped as late.
pub const ON_TIME: Duration = Duration::from_secs(60);

/// Why a ten-minute slot has no snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// The graph is not synced: its totals would undercount.
    Syncing,
    /// The loop woke more than [`ON_TIME`] after the slot (a stalled or suspended process).
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
            Self::Late => "late",
            Self::StoreBusy => "store_busy",
            Self::StoreFailed => "store_failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Written {
        time: u64,
        stats: Stats,
        rolled: Rolled,
        elapsed: Duration,
    },
    Skipped {
        time: u64,
        reason: Skip,
    },
}

pub struct Config {
    /// Wall-clock time since the Unix epoch. Snapshots are stamped and slots cut by it.
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

/// Takes a snapshot at every whole ten minutes, forever. `stats` gives the graph's totals, or
/// `None` while it is not synced.
pub async fn run(store: Arc<Store>, mut stats: impl FnMut() -> Option<Stats>, config: Config) {
    let mut last = 0;
    loop {
        let now = (config.clock)();
        let slot = (now.as_secs() / EVERY + 1) * EVERY;
        tokio::time::sleep(Duration::from_secs(slot).saturating_sub(now)).await;
        let woke = (config.clock)();
        // The clock went back past a slot already looked at: wait for the next one.
        if slot <= last {
            continue;
        }
        last = slot;
        let event = if woke.saturating_sub(Duration::from_secs(slot)) > ON_TIME {
            Event::Skipped {
                time: slot,
                reason: Skip::Late,
            }
        } else if let Some(s) = stats() {
            let started = Instant::now();
            let point = Point {
                time: slot,
                samples: 1,
                sums: s,
            };
            match db::record(&store, point, woke.as_secs()).await {
                Ok(rolled) => Event::Written {
                    time: slot,
                    stats: s,
                    rolled,
                    elapsed: started.elapsed(),
                },
                Err(Error::Full) => Event::Skipped {
                    time: slot,
                    reason: Skip::StoreBusy,
                },
                Err(_) => Event::Skipped {
                    time: slot,
                    reason: Skip::StoreFailed,
                },
            }
        } else {
            Event::Skipped {
                time: slot,
                reason: Skip::Syncing,
            }
        };
        (config.observer)(&event);
    }
}
