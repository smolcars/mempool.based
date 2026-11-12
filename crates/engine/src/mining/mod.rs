//! Pure logic behind the mining routes: who mined a block ([`pools`]), the periods a client may
//! ask for ([`Interval`]) and hashrate from proof of work over time ([`hashrate`]). The block
//! table's statements for them are in `blocks::mining`; the routes in the server.

pub mod pools;

use crate::blocks::Bucket;

/// The periods the mining routes take, as the reference spells them. Anything else is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Interval {
    Day,
    ThreeDays,
    Week,
    Month,
    ThreeMonths,
    SixMonths,
    Year,
    TwoYears,
    ThreeYears,
    All,
}

const DAY: u64 = 86_400;

impl Interval {
    pub const ALL: [Self; 10] = [
        Self::Day,
        Self::ThreeDays,
        Self::Week,
        Self::Month,
        Self::ThreeMonths,
        Self::SixMonths,
        Self::Year,
        Self::TwoYears,
        Self::ThreeYears,
        Self::All,
    ];

    /// Exactly one of `24h`, `3d`, `1w`, `1m`, `3m`, `6m`, `1y`, `2y`, `3y` or `all`.
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|i| i.as_str() == s)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Day => "24h",
            Self::ThreeDays => "3d",
            Self::Week => "1w",
            Self::Month => "1m",
            Self::ThreeMonths => "3m",
            Self::SixMonths => "6m",
            Self::Year => "1y",
            Self::TwoYears => "2y",
            Self::ThreeYears => "3y",
            Self::All => "all",
        }
    }

    /// Its length in seconds; a month is 30 days and a year 365. `None` for all of history.
    pub fn seconds(self) -> Option<u64> {
        let days = match self {
            Self::Day => 1,
            Self::ThreeDays => 3,
            Self::Week => 7,
            Self::Month => 30,
            Self::ThreeMonths => 90,
            Self::SixMonths => 180,
            Self::Year => 365,
            Self::TwoYears => 730,
            Self::ThreeYears => 1095,
            Self::All => return None,
        };
        Some(days * DAY)
    }

    /// Where it starts if it ends at `now` (Unix seconds).
    pub fn start(self, now: u64) -> u64 {
        self.seconds().map_or(0, |s| now.saturating_sub(s))
    }

    /// Blocks averaged into each point of the block series, so no answer has more than about
    /// a thousand points: one block a point up to a week, a week's worth over all of history.
    pub fn blocks_per_point(self) -> u32 {
        match self {
            Self::Day | Self::ThreeDays | Self::Week => 1,
            Self::Month => 6,
            Self::ThreeMonths => 18,
            Self::SixMonths => 36,
            Self::Year => 72,
            Self::TwoYears => 144,
            Self::ThreeYears => 216,
            Self::All => 1008,
        }
    }

    /// Whether the hashrate chart has a point a week rather than a day.
    pub fn weekly_hashrate(self) -> bool {
        matches!(self, Self::TwoYears | Self::ThreeYears | Self::All)
    }
}

/// Network hashrate from proof of work. A block at difficulty `d` takes `d × 2^32` hashes on
/// average, so the hashrate over a stretch of chain is the work of its blocks over the time they
/// took: the standard estimate, the one `getnetworkhashps` makes.
pub mod hashrate {
    use super::{Bucket, DAY};

    /// Hashes a block at difficulty 1 takes on average.
    pub const PER_DIFFICULTY: f64 = 4_294_967_296.0;

    /// Hashes per second for blocks whose difficulties sum to `difficulty_sum`, mined over
    /// `seconds`. `None` for no time at all (timestamps can repeat).
    pub fn estimate(difficulty_sum: f64, seconds: u64) -> Option<f64> {
        (seconds > 0).then(|| difficulty_sum * PER_DIFFICULTY / seconds as f64)
    }

    /// One point of a hashrate chart: the start of its day or week, Unix seconds.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct Point {
        pub time: u64,
        pub hashrate: f64,
    }

    /// Daily buckets (keyed by day number, from `blocks::periods(.., Period::Day)`) merged into
    /// weeks starting Monday 00:00 UTC, keyed by the week's start in days.
    pub fn weeks(days: &[Bucket]) -> Vec<Bucket> {
        let mut out: Vec<Bucket> = Vec::new();
        for d in days {
            // Day 4 after the epoch, a Thursday, was a Monday's.
            let key = (d.key - 4).div_euclid(7) * 7 + 4;
            match out.last_mut() {
                Some(w) if w.key == key => {
                    let n = f64::from(w.blocks) + f64::from(d.blocks);
                    w.difficulty = (w.difficulty * f64::from(w.blocks)
                        + d.difficulty * f64::from(d.blocks))
                        / n.max(1.0);
                    w.blocks += d.blocks;
                    w.first_height = w.first_height.min(d.first_height);
                    w.last_height = w.last_height.max(d.last_height);
                    w.first_time = w.first_time.min(d.first_time);
                    w.last_time = w.last_time.max(d.last_time);
                }
                _ => out.push(Bucket { key, ..d.clone() }),
            }
        }
        out
    }

    /// One point per bucket (days or weeks, keyed in days, oldest first, every block between
    /// the first and the last stored): the work of the blocks in the last `window` days up to and
    /// including the bucket, over the time from the last block before them to the bucket's last
    /// block. A bucket with no earlier bucket at least `window` days back has no point, so a chart
    /// starts where its first full window does.
    ///
    /// Daily points use a 7-day window: a day holds about 144 blocks, whose count alone varies
    /// by ±8 % from luck, while a week's 1,008 vary by ±3 % and still show a change within days.
    pub fn series(buckets: &[Bucket], window: i64) -> Vec<Point> {
        let mut out = Vec::new();
        for (i, b) in buckets.iter().enumerate() {
            let Some(r) = buckets
                .get(..i)
                .and_then(|before| before.iter().rposition(|p| p.key <= b.key - window))
            else {
                continue;
            };
            let Some(base) = buckets.get(r) else { continue };
            let work: f64 = buckets
                .get(r + 1..=i)
                .unwrap_or_default()
                .iter()
                .map(|x| x.difficulty * f64::from(x.blocks))
                .sum();
            let seconds = b.last_time.saturating_sub(base.last_time);
            if let (Some(hashrate), Ok(day)) = (estimate(work, seconds), u64::try_from(b.key)) {
                out.push(Point {
                    time: day * DAY,
                    hashrate,
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;

    fn day(key: i64, blocks: u32, difficulty: f64, last_time: u64) -> Bucket {
        Bucket {
            key,
            blocks,
            first_height: 0,
            last_height: 0,
            first_time: last_time.saturating_sub(DAY),
            last_time,
            tx_count: 0,
            size: 0,
            weight: 0,
            fees: 0,
            subsidy: 0,
            reward: 0,
            difficulty,
            rated: 0,
            fee_rate: None,
        }
    }

    #[test]
    fn intervals_parse_strictly() {
        for i in Interval::ALL {
            assert_eq!(Interval::parse(i.as_str()), Some(i));
        }
        for bad in [
            "", "1W", "24H", " 1w", "1w ", "7d", "1d", "all/", "1y\0", "%31w", "1",
        ] {
            assert_eq!(Interval::parse(bad), None, "{bad:?}");
        }
        assert_eq!(Interval::Month.start(100 * DAY), 70 * DAY);
        assert_eq!(Interval::All.start(100 * DAY), 0);
        assert_eq!(Interval::Year.start(5), 0);
        // The block series stay near a thousand points at mainnet's pace.
        for i in Interval::ALL {
            let blocks = i.seconds().unwrap_or(17 * 365 * DAY) / 600;
            assert!(blocks / u64::from(i.blocks_per_point()) <= 1100, "{i:?}");
        }
    }

    #[test]
    fn estimate_by_hand() {
        // Difficulty 1 at one block every 600 s: 2^32 / 600 = 7,158,278.83 hashes a second.
        let h = hashrate::estimate(1.0, 600).unwrap();
        assert!((h - 7_158_278.826_666_667).abs() < 1e-6);
        // 1,008 blocks at difficulty 1.5e14 over exactly a week: 1.5e14 × 2^32 / 600.
        let h = hashrate::estimate(1008.0 * 1.5e14, 7 * DAY).unwrap();
        assert!((h / (1.5e14 * 4_294_967_296.0 / 600.0) - 1.0).abs() < 1e-12);
        assert_eq!(hashrate::estimate(5.0, 0), None);
    }

    #[test]
    fn series_uses_a_trailing_window() {
        // Ten days of 144 blocks at difficulty 2, each day's last block at the end of the day.
        let days: Vec<Bucket> = (0..10)
            .map(|k| day(100 + k, 144, 2.0, (101 + k as u64) * DAY - 1))
            .collect();
        let points = hashrate::series(&days, 7);
        // Days 100..=106 have no bucket 7 days back; 107, 108, 109 do.
        assert_eq!(points.len(), 3);
        assert_eq!(points[0].time, 107 * DAY);
        // 7 × 144 blocks × 2 × 2^32 over exactly 7 days.
        let want = 7.0 * 144.0 * 2.0 * 4_294_967_296.0 / (7 * DAY) as f64;
        for p in &points {
            assert!((p.hashrate / want - 1.0).abs() < 1e-12);
        }
        // A missing day in the window still counts the time it covers, not a false zero.
        let mut gap = days.clone();
        gap.remove(8);
        let p = hashrate::series(&gap, 7);
        assert_eq!(p.last().unwrap().time, 109 * DAY);
        let want_gap = 6.0 * 144.0 * 2.0 * 4_294_967_296.0 / (7 * DAY) as f64;
        assert!((p.last().unwrap().hashrate / want_gap - 1.0).abs() < 1e-12);
        // Timestamps that do not advance give no point instead of infinity.
        let flat = vec![day(1, 1, 1.0, 50), day(2, 1, 1.0, 50)];
        assert!(hashrate::series(&flat, 1).is_empty());
    }

    #[test]
    fn weeks_start_on_monday() {
        // Day 20549 (2026-04-06) is a Monday.
        let days: Vec<Bucket> = (20_547..20_557)
            .map(|k| day(k, 100, 1.0 + (k % 2) as f64, k as u64 * DAY + 100))
            .collect();
        let w = hashrate::weeks(&days);
        assert_eq!(
            w.iter().map(|b| b.key).collect::<Vec<_>>(),
            [20_542, 20_549, 20_556]
        );
        assert_eq!(
            w.iter().map(|b| b.blocks).collect::<Vec<_>>(),
            [200, 700, 100]
        );
        // Mean difficulty weighted by blocks: four odd days (2.0) and three even (1.0).
        assert!((w[1].difficulty - 11.0 / 7.0).abs() < 1e-12);
        assert_eq!(w[1].last_time, 20_555 * DAY + 100);
    }
}
