//! SQL for the `mempool_history` table. Every statement is a literal; values are bound
//! (SECURITY.md rule 12).

use super::{
    DAY, Flow, HOUR, Interval, KEEP_HOURS, KEEP_MINUTES, Level, MINUTE, Point, Series, Totals,
};
use crate::store::{Error, Read, Sql, SqlError, Store, params};

/// The value columns, in the order [`read`] and the inserts use them.
macro_rules! columns {
    () => {
        "samples, count, vsize, fees, min_fee, band0, band1, band2, band3, band4, band5, band6, \
         band7, band8, band9, in_count, in_vsize, out_count, out_vsize, mined_count, mined_vsize"
    };
}

macro_rules! sums {
    () => {
        "sum(samples), sum(count), sum(vsize), sum(fees), sum(min_fee), sum(band0), sum(band1), \
         sum(band2), sum(band3), sum(band4), sum(band5), sum(band6), sum(band7), sum(band8), \
         sum(band9), sum(in_count), sum(in_vsize), sum(out_count), sum(out_vsize), \
         sum(mined_count), sum(mined_vsize)"
    };
}

const INSERT: &str = concat!(
    "INSERT OR IGNORE INTO mempool_history (level, time, ",
    columns!(),
    ") VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ",
    "?19, ?20, ?21, ?22, ?23)"
);

/// Adds the rows of level ?1 older than ?4 into level ?2 rows of ?3 seconds, merging with any
/// already there, so totals are kept whatever was rolled before.
const ROLL: &str = concat!(
    "INSERT INTO mempool_history (level, time, ",
    columns!(),
    ") SELECT ?2, time / ?3 * ?3, ",
    sums!(),
    " FROM mempool_history WHERE level = ?1 AND time < ?4 GROUP BY time / ?3 ",
    "ON CONFLICT (level, time) DO UPDATE SET samples = samples + excluded.samples, ",
    "count = count + excluded.count, vsize = vsize + excluded.vsize, ",
    "fees = fees + excluded.fees, min_fee = min_fee + excluded.min_fee, ",
    "band0 = band0 + excluded.band0, band1 = band1 + excluded.band1, ",
    "band2 = band2 + excluded.band2, band3 = band3 + excluded.band3, ",
    "band4 = band4 + excluded.band4, band5 = band5 + excluded.band5, ",
    "band6 = band6 + excluded.band6, band7 = band7 + excluded.band7, ",
    "band8 = band8 + excluded.band8, band9 = band9 + excluded.band9, ",
    "in_count = in_count + excluded.in_count, in_vsize = in_vsize + excluded.in_vsize, ",
    "out_count = out_count + excluded.out_count, out_vsize = out_vsize + excluded.out_vsize, ",
    "mined_count = mined_count + excluded.mined_count, ",
    "mined_vsize = mined_vsize + excluded.mined_vsize"
);

/// Rows moved by one roll-up.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rolled {
    /// Minute rows added into hours, and hour rows into days.
    pub minutes: usize,
    pub hours: usize,
}

fn insert_row(sql: &Sql, level: Level, p: &Point) -> Result<usize, Error> {
    let (t, f) = (&p.sums, &p.sums.flow);
    let values = [p.time, p.samples, t.count, t.vsize, t.fees, t.min_fee]
        .into_iter()
        .chain(t.bands)
        .chain([
            f.in_count,
            f.in_vsize,
            f.out_count,
            f.out_vsize,
            f.mined_count,
            f.mined_vsize,
        ])
        .map(int);
    Ok(sql.prepare(INSERT)?.execute(rusqlite::params_from_iter(
        std::iter::once(level as i64).chain(values),
    ))?)
}

/// SQLite integers are signed; nothing stored here comes near 2^63.
fn int(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

/// Rolls every row past its level's retention, as of `now`, into the next level, in whole
/// periods: minutes into hours, then hours into days. Inside one write, so it is all or nothing.
pub(super) fn roll(sql: &Sql, now: u64) -> Result<Rolled, Error> {
    let mut rolled = Rolled::default();
    for (from, to, period, keep, moved) in [
        (
            Level::Minute,
            Level::Hour,
            HOUR,
            KEEP_MINUTES,
            &mut rolled.minutes,
        ),
        (Level::Hour, Level::Day, DAY, KEEP_HOURS, &mut rolled.hours),
    ] {
        let cut = now.saturating_sub(keep) / period * period;
        sql.prepare(ROLL)?
            .execute(params![from as i64, to as i64, int(period), int(cut)])?;
        *moved = sql
            .prepare("DELETE FROM mempool_history WHERE level = ?1 AND time < ?2")?
            .execute(params![from as i64, int(cut)])?;
    }
    Ok(rolled)
}

/// Stores one minute's sample and rolls up what has aged, in one write. Fails with
/// [`Error::Full`] rather than wait behind a full write queue: a missed minute is a gap, not a
/// delay. A minute already stored is left as it is, and so is one whose hour or day was rolled
/// up already (the clock was ahead and came back), which it would otherwise count twice.
pub async fn record(store: &Store, sample: Point, now: u64) -> Result<Rolled, Error> {
    store
        .try_write(move |sql| {
            let rolled: bool = sql
                .prepare(
                    "SELECT EXISTS (SELECT 1 FROM mempool_history WHERE \
                     (level = 1 AND time = ?1 / 3600 * 3600) OR (level = 2 AND time = ?1 / 86400 * 86400))",
                )?
                .query_row(params![int(sample.time)], |r| r.get(0))?;
            if !rolled {
                insert_row(sql, Level::Minute, &sample)?;
            }
            roll(sql, now)
        })
        .await
}

/// Rolls up what has aged as of `now`. Running it again changes nothing.
pub async fn roll_up(store: &Store, now: u64) -> Result<Rolled, Error> {
    store.write(move |sql| roll(sql, now)).await
}

/// Stores rows at `level` as they are, keeping any already stored at the same time. For tests,
/// benchmarks and imports.
pub async fn insert(store: &Store, level: Level, rows: Vec<Point>) -> Result<(), Error> {
    store
        .write(move |sql| {
            for p in &rows {
                insert_row(sql, level, p)?;
            }
            Ok(())
        })
        .await
}

fn get(r: &rusqlite::Row, i: usize) -> Result<u64, SqlError> {
    let v: i64 = r.get(i)?;
    u64::try_from(v).map_err(|_| SqlError::IntegralValueOutOfRange(i, v))
}

/// A row of `time, ` followed by [`columns!`] or [`sums!`].
fn read(r: &rusqlite::Row) -> Result<Point, SqlError> {
    let mut bands = [0; crate::projected::BANDS];
    for (i, b) in bands.iter_mut().enumerate() {
        *b = get(r, 6 + i)?;
    }
    Ok(Point {
        time: get(r, 0)?,
        samples: get(r, 1)?,
        sums: Totals {
            count: get(r, 2)?,
            vsize: get(r, 3)?,
            fees: get(r, 4)?,
            min_fee: get(r, 5)?,
            bands,
            flow: Flow {
                in_count: get(r, 16)?,
                in_vsize: get(r, 17)?,
                out_count: get(r, 18)?,
                out_vsize: get(r, 19)?,
                mined_count: get(r, 20)?,
                mined_vsize: get(r, 21)?,
            },
        },
    })
}

/// The series for `interval` as of `now`, in one read: a point per step with at least one
/// sample, and how much of the period was sampled.
pub async fn series(store: &Store, interval: Interval, now: u64) -> Result<Series, Error> {
    store
        .read(Read::MempoolHistory, move |sql| {
            // Each level holds older times than the one below it, so the oldest row is in the
            // coarsest level that has any.
            let mut first = None;
            let mut oldest =
                sql.prepare("SELECT min(time) FROM mempool_history WHERE level = ?1")?;
            for level in [Level::Day, Level::Hour, Level::Minute] {
                let t: Option<i64> = oldest.query_row(params![level as i64], |r| r.get(0))?;
                if let Some(t) = t {
                    first = u64::try_from(t).ok();
                    break;
                }
            }
            let (from, step, since) = match interval.span() {
                Some((span, step)) => {
                    let from = now.saturating_sub(span).div_ceil(step) * step;
                    (from, step, from)
                }
                // At most about 1,000 points of whole days over the whole history.
                None => {
                    let first = first.unwrap_or(now);
                    let step = now.saturating_sub(first).div_ceil(1_000 * DAY).max(1) * DAY;
                    (first / step * step, step, first)
                }
            };
            let points = sql
                .prepare(concat!(
                    "SELECT time / ?2 * ?2 AS t, ",
                    sums!(),
                    " FROM mempool_history WHERE level IN (0, 1, 2) AND time BETWEEN ?1 AND ?3 ",
                    "GROUP BY t ORDER BY t"
                ))?
                // Never past `now`: rows stamped while the clock ran ahead stay out.
                .query_map(params![int(from), int(step), int(now)], read)?
                .collect::<Result<Vec<_>, _>>()?;
            // The minute in progress may not be sampled yet; the one before must be.
            let last = (now / MINUTE * MINUTE).saturating_sub(MINUTE);
            let minutes = if last >= since {
                (last - since) / MINUTE + 1
            } else {
                0
            };
            let sampled: i64 = sql
                .prepare(
                    "SELECT coalesce(sum(samples), 0) FROM mempool_history \
                     WHERE level IN (0, 1, 2) AND time BETWEEN ?1 AND ?2",
                )?
                .query_row(params![int(since), int(last)], |r| r.get(0))?;
            Ok(Series {
                from,
                to: now,
                step,
                points,
                first,
                sampled: u64::try_from(sampled).unwrap_or(0),
                minutes,
            })
        })
        .await
}
