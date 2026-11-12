//! Mempool history: the mempool's size, fees and fee bands sampled once a minute, stored in the
//! `mempool_history` table, rolled into coarser rows as they age, and read back as a series for
//! a chart. Retention, sizes and timings: `docs/STORE.md`, "mempool_history".
//!
//! ```no_run
//! # async fn f(t: engine::mempool::Tracker, p: engine::projected::Projector, s: std::sync::Arc<engine::store::Store>) {
//! use engine::history::{self, Interval};
//! let _task = history::start(t, p, s.clone(), history::Config::default());
//! let day = history::series(&s, Interval::Day, history::unix_now()).await;
//! # }
//! ```

mod db;
mod sampler;

pub use db::{Rolled, insert, record, roll_up, series};
pub use sampler::{Config, Event, ON_TIME, Skip, Watch, start};

use std::time::{Duration, SystemTime};

use bitcoin::Amount;

use crate::projected::{BANDS, Projection};

pub const MINUTE: u64 = 60;
pub const HOUR: u64 = 3_600;
pub const DAY: u64 = 86_400;
/// Minute rows are rolled into hours once their hour is this old: enough for a week at
/// ten-minute points.
pub const KEEP_MINUTES: u64 = 8 * DAY;
/// Hour rows are rolled into days once their day is this old: enough for a year at eight-hour
/// points.
pub const KEEP_HOURS: u64 = 400 * DAY;

/// How fine a stored row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Minute = 0,
    Hour = 1,
    Day = 2,
}

/// Transactions and virtual size that moved through the mempool over a minute or more.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Flow {
    pub in_count: u64,
    pub in_vsize: u64,
    /// Left for any reason: confirmed, replaced or evicted.
    pub out_count: u64,
    pub out_vsize: u64,
    /// Of what left, what was confirmed in a block.
    pub mined_count: u64,
    pub mined_vsize: u64,
}

/// The mempool as one sample saw it, or the sum of several samples' values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    pub count: u64,
    pub vsize: u64,
    /// Sats.
    pub fees: u64,
    /// The node's `mempoolminfee`, sat per 1,000 vB.
    pub min_fee: u64,
    /// Virtual size in each fee band of `docs/DESIGN.md`, by effective rate, cheapest first.
    pub bands: [u64; BANDS],
    pub flow: Flow,
}

impl Totals {
    /// Adds `o` field by field.
    pub fn add(&mut self, o: &Self) {
        for (a, b) in [
            (&mut self.count, o.count),
            (&mut self.vsize, o.vsize),
            (&mut self.fees, o.fees),
            (&mut self.min_fee, o.min_fee),
            (&mut self.flow.in_count, o.flow.in_count),
            (&mut self.flow.in_vsize, o.flow.in_vsize),
            (&mut self.flow.out_count, o.flow.out_count),
            (&mut self.flow.out_vsize, o.flow.out_vsize),
            (&mut self.flow.mined_count, o.flow.mined_count),
            (&mut self.flow.mined_vsize, o.flow.mined_vsize),
        ] {
            *a = a.saturating_add(b);
        }
        for (a, b) in self.bands.iter_mut().zip(o.bands) {
            *a = a.saturating_add(b);
        }
    }
}

/// One stored row, or one point of a series: `sums` over `samples` sampled minutes. A mean is
/// a sum over `samples`; a flow per minute too.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Point {
    /// Unix seconds: the sampled minute, or the start of the row's or the point's period.
    pub time: u64,
    pub samples: u64,
    pub sums: Totals,
}

/// The sample taken at minute `time`: the projection's totals (the whole mempool, as the
/// `mempool` topic sends it), the node's minimum rate, and the flow over the minute before.
pub fn sample(time: u64, p: &Projection, min_fee: Amount, flow: Flow) -> Point {
    let mut t = Totals {
        min_fee: min_fee.to_sat(),
        flow,
        ..Totals::default()
    };
    for b in &p.blocks {
        let s = &b.summary;
        t.count += u64::from(s.txs);
        t.vsize += s.vsize;
        t.fees += s.fees;
        for (total, v) in t.bands.iter_mut().zip(s.bands) {
            *total += v;
        }
    }
    Point {
        time,
        samples: 1,
        sums: t,
    }
}

/// The periods a series can be asked for, as the reference names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Interval {
    TwoHours,
    Day,
    Week,
    Month,
    ThreeMonths,
    SixMonths,
    Year,
    TwoYears,
    ThreeYears,
    FourYears,
    All,
}

impl Interval {
    pub const ALL: [Self; 11] = [
        Self::TwoHours,
        Self::Day,
        Self::Week,
        Self::Month,
        Self::ThreeMonths,
        Self::SixMonths,
        Self::Year,
        Self::TwoYears,
        Self::ThreeYears,
        Self::FourYears,
        Self::All,
    ];

    /// Exactly `2h`, `24h`, `1w`, `1m`, `3m`, `6m`, `1y`, `2y`, `3y`, `4y` or `all`.
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|i| i.name() == s)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::TwoHours => "2h",
            Self::Day => "24h",
            Self::Week => "1w",
            Self::Month => "1m",
            Self::ThreeMonths => "3m",
            Self::SixMonths => "6m",
            Self::Year => "1y",
            Self::TwoYears => "2y",
            Self::ThreeYears => "3y",
            Self::FourYears => "4y",
            Self::All => "all",
        }
    }

    /// How far back it reaches and the seconds a point covers, so that no answer holds more
    /// than 1,460 points and none mixes rows finer than its points with coarser ones: a point is
    /// never shorter than the rows stored for its period (`KEEP_MINUTES`, `KEEP_HOURS`). `None`
    /// for `All`, which depends on the stored history.
    pub fn span(self) -> Option<(u64, u64)> {
        Some(match self {
            Self::TwoHours => (2 * HOUR, MINUTE),
            Self::Day => (DAY, MINUTE),
            Self::Week => (7 * DAY, 10 * MINUTE),
            Self::Month => (30 * DAY, HOUR),
            Self::ThreeMonths => (90 * DAY, 2 * HOUR),
            Self::SixMonths => (180 * DAY, 4 * HOUR),
            Self::Year => (365 * DAY, 8 * HOUR),
            Self::TwoYears => (730 * DAY, DAY),
            Self::ThreeYears => (1_095 * DAY, DAY),
            Self::FourYears => (1_460 * DAY, 2 * DAY),
            Self::All => return None,
        })
    }
}

/// A period's points and how much of it was sampled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Series {
    /// The period answered: from `from`, a multiple of `step`, to `to`, the time asked at.
    pub from: u64,
    pub to: u64,
    /// Seconds per point. Each point's `time` is the start of its step.
    pub step: u64,
    /// Oldest first. A step with no sample has no point: a gap, never a zero.
    pub points: Vec<Point>,
    /// The time of the oldest stored row: history begins there. `None` with nothing stored.
    pub first: Option<u64>,
    /// Minutes sampled from `from` to the last whole minute before `to`, and how many minutes
    /// that is. The sample of the minute in progress is in `points` but not counted.
    pub sampled: u64,
    pub minutes: u64,
}

impl Series {
    /// Every minute of the period was sampled.
    pub fn complete(&self) -> bool {
        self.minutes > 0 && self.sampled >= self.minutes
    }

    /// The time of the earliest point, if any.
    pub fn earliest(&self) -> Option<u64> {
        self.points.first().map(|p| p.time)
    }
}

/// Seconds since the Unix epoch on the system clock.
pub fn unix_now() -> u64 {
    since_epoch().as_secs()
}

pub(crate) fn since_epoch() -> Duration {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
