//! The block table's running totals (`docs/STORE.md`, "Totals"): per UTC day and per block
//! series point, every aggregate [`super::periods`] computes, and per pool the blocks, empty
//! blocks and reward per 1,008 heights and per week. A read over all of history sums a few
//! thousand of these instead of scanning every row.
//!
//! **Exact by construction.** A bucket is never adjusted: it is recomputed from its rows by the
//! very statement a scan runs, so it holds what the scan gives for those rows. The table's
//! triggers record each bucket a row change touches (`totals_dirty`), and [`settle`] recomputes
//! them before the write commits, in the same transaction: an insert, a replacement, a deletion,
//! a rating or a re-attribution alike. Readers see the rows and their totals change together.
//!
//! A read uses a stored bucket only when every one of its rows lies in the range asked; a bucket
//! at either end of the range is computed from the rows in range, as before. Until the rows
//! stored before the totals existed are filled in ([`fill_totals`]), every read scans.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use super::db::{Bucket, get, int, periods_scan, periods_sql, read_bucket};
use super::mining::{
    PoolCount, PoolWeek, WEEK, WEEK_OFFSET, first_since_scan, pool_counts_scan, pool_weeks_scan,
};
use crate::store::{Error, OptionalExtension, Read, Sql, Store, params};

/// `kind` of a day's bucket in `block_totals`, and of a day's key in `totals_dirty`.
const DAY: u32 = 0;
/// `kind` of a week's bucket in `pool_totals` and `totals_dirty`.
const WEEK_KIND: u32 = 7;
/// Heights in a pool bucket; also a block series' kind, and the buckets [`full_from`] counts.
const POOL: u32 = 1008;
/// The block series' points kept, in heights: those of the intervals past a month
/// (`mining::Interval::blocks_per_point`).
const HEIGHTS: [u32; 6] = [18, 36, 72, 144, 216, 1008];
/// Ranges shorter than this many heights are scanned: their rows are about as few as their
/// buckets.
const WORTH: u32 = 2016;
/// Heights the fill totals per write: a write of about a tenth of a second on mainnet.
pub const FILL: u32 = 10_000;
const DAY_SECONDS: i64 = 86_400;

/// Whether a range is long enough that its buckets are cheaper than its rows.
pub(super) fn worth(from: u32, to: u32) -> bool {
    to.saturating_sub(from) >= WORTH
}

/// Whether [`super::periods`] by `n` heights (0: by day) has stored buckets.
pub(super) fn kept(n: u32) -> bool {
    n == DAY || HEIGHTS.contains(&n)
}

/// Whether every row is in its buckets: the fill is done.
fn ready(sql: &Sql) -> Result<bool, Error> {
    Ok(sql
        .prepare("SELECT NOT EXISTS (SELECT 1 FROM totals_pending)")?
        .query_row([], |r| r.get(0))?)
}

/// Recomputes every bucket the write so far has touched, then forgets them. Run by the store
/// before every write that changed the block table commits.
pub(crate) fn settle(sql: &Sql) -> Result<(), Error> {
    // The store's own tests write a `blocks` table of their own, without totals.
    let kept: bool = sql
        .prepare(
            "SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE type = 'table' \
             AND name = 'totals_dirty')",
        )?
        .query_row([], |r| r.get(0))?;
    if !kept {
        return Ok(());
    }
    let dirty: Vec<(u32, i64)> = {
        let mut stmt = sql.prepare("SELECT DISTINCT kind, key FROM totals_dirty")?;
        let rows = stmt.query_map([], |r| Ok((get(r, 0)?, r.get(1)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    if dirty.is_empty() {
        return Ok(());
    }
    let mut drop_block = sql.prepare("DELETE FROM block_totals WHERE kind = ?1 AND key = ?2")?;
    let mut drop_pool = sql.prepare("DELETE FROM pool_totals WHERE kind = ?1 AND key = ?2")?;
    // The scan's own statement, so a stored bucket is what a scan gives for it.
    let mut block = sql.prepare(concat!(
        "INSERT INTO block_totals SELECT ?4, * FROM (",
        periods_sql!(),
        ") WHERE k = ?5"
    ))?;
    let mut day_heights =
        sql.prepare("SELECT min(height), max(height) FROM blocks WHERE time BETWEEN ?1 AND ?2")?;
    let mut pool_heights = sql.prepare(
        "INSERT INTO pool_totals SELECT ?3, ?4, pool, count(*), sum(tx_count = 1), sum(reward) \
         FROM blocks WHERE height BETWEEN ?1 AND ?2 GROUP BY pool",
    )?;
    let mut pool_week = sql.prepare(
        "INSERT INTO pool_totals SELECT ?3, ?4, pool, count(*), sum(tx_count = 1), sum(reward) \
         FROM blocks WHERE time BETWEEN ?1 AND ?2 GROUP BY pool",
    )?;
    for (kind, key) in dirty {
        match kind {
            DAY => {
                drop_block.execute(params![DAY, key])?;
                let start = key.saturating_mul(DAY_SECONDS);
                let (lo, hi): (Option<u32>, Option<u32>) = day_heights
                    .query_row(params![start, start.saturating_add(DAY_SECONDS - 1)], |r| {
                        Ok((r.get(0)?, r.get(1)?))
                    })?;
                if let (Some(lo), Some(hi)) = (lo, hi) {
                    block.execute(params![lo, hi, DAY, DAY, key])?;
                }
            }
            WEEK_KIND => {
                drop_pool.execute(params![WEEK_KIND, key])?;
                let (lo, hi) = week_times(key);
                pool_week.execute(params![lo, hi, WEEK_KIND, key])?;
            }
            n => {
                drop_block.execute(params![n, key])?;
                let lo = key.saturating_mul(i64::from(n));
                let hi = lo.saturating_add(i64::from(n) - 1);
                block.execute(params![lo, hi, n, n, key])?;
                if n == POOL {
                    drop_pool.execute(params![POOL, key])?;
                    pool_heights.execute(params![lo, hi, POOL, key])?;
                }
            }
        }
    }
    sql.prepare("DELETE FROM totals_dirty")?.execute([])?;
    Ok(())
}

/// The timestamps of week `key` as `(time - WEEK_OFFSET) / WEEK` numbers them: SQLite's division
/// truncates, so week 0 also holds the four days before the first Monday.
fn week_times(key: i64) -> (i64, i64) {
    let (week, offset) = (int(WEEK), int(WEEK_OFFSET));
    let start = key.saturating_mul(week).saturating_add(offset);
    let lo = if key <= 0 { 0 } else { start };
    (lo, start.saturating_add(week - 1))
}

/// The week a day falls in, numbered as [`week_times`] numbers them.
fn week_of_day(day: i64) -> i64 {
    (day - 4) / 7
}

/// The days of week `key`.
fn week_days(key: i64) -> (i64, i64) {
    let lo = if key <= 0 { 0 } else { key * 7 + 4 };
    (lo, key * 7 + 10)
}

/// What [`fill_totals`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Filled {
    /// Heights it went over, and the writes it took.
    pub heights: u64,
    pub writes: u64,
    pub elapsed: Duration,
    /// The longest of its writes: how long it held the writer at once.
    pub longest: Duration,
}

/// Totals the rows stored before the totals existed, newest first, [`FILL`] heights a write.
/// Reads scan the table until it is done. `None` when there was nothing to do; resumes where it
/// stopped after a failure or a restart.
pub async fn fill_totals(store: &Store) -> Result<Option<Filled>, Error> {
    fill_by(store, FILL).await
}

/// [`fill_totals`], `chunk` heights a write.
pub(super) async fn fill_by(store: &Store, chunk: u32) -> Result<Option<Filled>, Error> {
    let started = Instant::now();
    let mut done = Filled::default();
    loop {
        let write = Instant::now();
        let step = store
            .write(move |sql| {
                let below: Option<u32> = sql
                    .prepare("SELECT below FROM totals_pending WHERE id = 0")?
                    .query_row([], |r| r.get(0))
                    .optional()?;
                let Some(below) = below else {
                    return Ok(None);
                };
                let lo = below.saturating_sub(chunk);
                if let Some(hi) = below.checked_sub(1) {
                    sql.prepare(
                        "INSERT INTO totals_dirty (kind, key) \
                         SELECT DISTINCT k.kind, CASE k.kind WHEN 0 THEN b.time / 86400 \
                         WHEN 7 THEN (b.time - 345600) / 604800 ELSE b.height / k.kind END \
                         FROM blocks b CROSS JOIN (SELECT 0 AS kind UNION ALL SELECT 7 \
                         UNION ALL SELECT 18 UNION ALL SELECT 36 UNION ALL SELECT 72 \
                         UNION ALL SELECT 144 UNION ALL SELECT 216 UNION ALL SELECT 1008) k \
                         WHERE b.height BETWEEN ?1 AND ?2",
                    )?
                    .execute(params![lo, hi])?;
                    settle(sql)?;
                }
                if lo == 0 {
                    sql.prepare("DELETE FROM totals_pending")?.execute([])?;
                } else {
                    sql.prepare("UPDATE totals_pending SET below = ?1 WHERE id = 0")?
                        .execute(params![lo])?;
                }
                Ok(Some(below - lo))
            })
            .await?;
        let Some(heights) = step else { break };
        done.longest = done.longest.max(write.elapsed());
        done.heights += u64::from(heights);
        done.writes += 1;
    }
    done.elapsed = started.elapsed();
    Ok((done.writes > 0).then_some(done))
}

/// `true` once [`fill_totals`] has finished. For tests and tools.
pub async fn totals_ready(store: &Store) -> Result<bool, Error> {
    store.read(Read::Blocks, ready).await
}

/// Rows and rated rows, summed over the 1,008-height buckets. `None` until the fill is done.
pub(super) fn counts(sql: &Sql) -> Result<Option<(u64, u64)>, Error> {
    if !ready(sql)? {
        return Ok(None);
    }
    let (rows, rated): (Option<i64>, Option<i64>) = sql
        .prepare("SELECT sum(blocks), sum(rated) FROM block_totals WHERE kind = ?1")?
        .query_row(params![POOL], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let n = |v: Option<i64>| v.and_then(|v| u64::try_from(v).ok()).unwrap_or(0);
    Ok(Some((n(rows), n(rated))))
}

/// The lowest height from which the 1,008-height buckets up to `tip`, the highest row, hold every
/// height: `tip` itself when its own bucket does not. `None` until the fill is done.
pub(super) fn full_from(sql: &Sql, tip: u32) -> Result<Option<u32>, Error> {
    if !ready(sql)? {
        return Ok(None);
    }
    let mut stmt = sql.prepare(
        "SELECT key, blocks FROM block_totals WHERE kind = ?1 AND key <= ?2 ORDER BY key DESC",
    )?;
    let mut rows = stmt.query(params![POOL, tip / POOL])?;
    let mut from = tip;
    let mut key = tip / POOL;
    let mut want = tip - key * POOL + 1;
    while let Some(r) = rows.next()? {
        let (k, blocks): (u32, u32) = (get(r, 0)?, get(r, 1)?);
        if k != key || blocks != want {
            break;
        }
        from = key * POOL;
        let Some(next) = key.checked_sub(1) else {
            break;
        };
        (key, want) = (next, POOL);
    }
    Ok(Some(from))
}

/// The highest height at or below `hi` that may lack a row: `hi` unless its 1,008-height bucket
/// holds every height, else the top of the highest bucket below it that does not. `Some(None)`
/// when every height down to 0 has a row; `None` until the fill is done.
pub(super) fn below_full(sql: &Sql, hi: u32) -> Result<Option<Option<u32>>, Error> {
    if !ready(sql)? {
        return Ok(None);
    }
    let mut stmt = sql.prepare(
        "SELECT key, blocks FROM block_totals WHERE kind = ?1 AND key <= ?2 ORDER BY key DESC",
    )?;
    let mut rows = stmt.query(params![POOL, hi / POOL])?;
    let mut key = Some(hi / POOL);
    while let Some(r) = rows.next()? {
        let Some(k) = key else { break };
        let (stored, blocks): (u32, u32) = (get(r, 0)?, get(r, 1)?);
        if stored != k || blocks != POOL {
            break;
        }
        key = k.checked_sub(1);
    }
    Ok(Some(key.map(|k| (k * POOL + POOL - 1).min(hi))))
}

/// The buckets of `kind` that reach into `from..=to`, oldest first: a stored one where all of its
/// rows lie in the range, else computed from those that do. `None` until the fill is done.
pub(super) fn periods(
    sql: &Sql,
    from: u32,
    to: u32,
    kind: u32,
) -> Result<Option<Vec<Bucket>>, Error> {
    if !ready(sql)? {
        return Ok(None);
    }
    let stored: Vec<Bucket> = {
        let mut stmt = sql.prepare(
            "SELECT key, blocks, first_height, last_height, first_time, last_time, tx_count, \
             size, weight, fees, subsidy, reward, difficulty, rated, fee_min, fee_p10, fee_p25, \
             fee_median, fee_p75, fee_p90, fee_max FROM block_totals \
             WHERE kind = ?1 AND last_height >= ?2 AND first_height <= ?3 ORDER BY key",
        )?;
        let rows = stmt.query_map(params![kind, from, to], read_bucket)?;
        rows.collect::<Result<_, _>>()?
    };
    let mut out = Vec::with_capacity(stored.len());
    for b in stored {
        if b.first_height >= from && b.last_height <= to {
            out.push(b);
        } else {
            let (lo, hi) = (b.first_height.max(from), b.last_height.min(to));
            out.extend(
                periods_scan(sql, lo, hi, kind)?
                    .into_iter()
                    .filter(|x| x.key == b.key),
            );
        }
    }
    Ok(Some(out))
}

/// [`super::pool_counts`] from the 1,008-height buckets wholly inside `from..=to` and the rows at
/// either end. `None` until the fill is done.
pub(super) fn pool_counts(sql: &Sql, from: u32, to: u32) -> Result<Option<Vec<PoolCount>>, Error> {
    if !ready(sql)? {
        return Ok(None);
    }
    // Buckets `first..end` lie wholly in the range.
    let first = from.div_ceil(POOL);
    let end = u32::try_from((u64::from(to) + 1) / u64::from(POOL)).unwrap_or(u32::MAX);
    if first >= end {
        return Ok(Some(pool_counts_scan(sql, from, to)?));
    }
    let mut sums: HashMap<u16, PoolCount> = HashMap::new();
    let mut add = |c: PoolCount| {
        let e = sums.entry(c.pool).or_insert(PoolCount {
            pool: c.pool,
            blocks: 0,
            empty: 0,
            reward: 0,
        });
        e.blocks += c.blocks;
        e.empty += c.empty;
        e.reward += c.reward;
    };
    let mut stmt = sql.prepare(
        "SELECT pool, sum(blocks), sum(empty), sum(reward) FROM pool_totals \
         WHERE kind = ?1 AND key BETWEEN ?2 AND ?3 GROUP BY pool",
    )?;
    let mut rows = stmt.query(params![POOL, first, end - 1])?;
    while let Some(r) = rows.next()? {
        add(PoolCount {
            pool: get(r, 0)?,
            blocks: get(r, 1)?,
            empty: get(r, 2)?,
            reward: get(r, 3)?,
        });
    }
    if from < first * POOL {
        pool_counts_scan(sql, from, first * POOL - 1)?
            .into_iter()
            .for_each(&mut add);
    }
    if end * POOL <= to {
        pool_counts_scan(sql, end * POOL, to)?
            .into_iter()
            .for_each(&mut add);
    }
    let mut out: Vec<PoolCount> = sums.into_values().collect();
    out.sort_by(|a, b| b.blocks.cmp(&a.blocks).then(a.pool.cmp(&b.pool)));
    Ok(Some(out))
}

/// [`super::pool_weeks`] from the week buckets whose every block lies in `from..=to` (their
/// heights are their days'), and the rows in range of the weeks at either end. `None` until the
/// fill is done.
pub(super) fn pool_weeks(sql: &Sql, from: u32, to: u32) -> Result<Option<Vec<PoolWeek>>, Error> {
    if !ready(sql)? {
        return Ok(None);
    }
    let (lo_day, hi_day): (Option<i64>, Option<i64>) = sql
        .prepare(
            "SELECT min(key), max(key) FROM block_totals \
             WHERE kind = ?1 AND last_height >= ?2 AND first_height <= ?3",
        )?
        .query_row(params![DAY, from, to], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let (Some(lo_day), Some(hi_day)) = (lo_day, hi_day) else {
        return Ok(Some(Vec::new()));
    };
    let (lo_week, hi_week) = (week_of_day(lo_day), week_of_day(hi_day));
    // Each week's lowest and highest height, from all of its days.
    let mut weeks: BTreeMap<i64, (u32, u32)> = BTreeMap::new();
    let mut stmt = sql.prepare(
        "SELECT key, first_height, last_height FROM block_totals \
         WHERE kind = ?1 AND key BETWEEN ?2 AND ?3",
    )?;
    let mut rows = stmt.query(params![DAY, week_days(lo_week).0, week_days(hi_week).1])?;
    while let Some(r) = rows.next()? {
        let (day, lo, hi): (i64, u32, u32) = (r.get(0)?, get(r, 1)?, get(r, 2)?);
        let w = weeks.entry(week_of_day(day)).or_insert((lo, hi));
        *w = (w.0.min(lo), w.1.max(hi));
    }
    let mut stored: BTreeMap<i64, Vec<(u16, u32)>> = BTreeMap::new();
    let mut stmt = sql.prepare(
        "SELECT key, pool, blocks FROM pool_totals WHERE kind = ?1 AND key BETWEEN ?2 AND ?3 \
         ORDER BY key, pool",
    )?;
    let mut rows = stmt.query(params![WEEK_KIND, lo_week, hi_week])?;
    while let Some(r) = rows.next()? {
        stored
            .entry(r.get(0)?)
            .or_default()
            .push((get(r, 1)?, get(r, 2)?));
    }
    let mut out = Vec::new();
    for (week, (lo, hi)) in weeks {
        if hi < from || lo > to {
            continue;
        }
        let start = u64::try_from(week).unwrap_or(0) * WEEK + WEEK_OFFSET;
        if lo >= from && hi <= to {
            let pools = stored.get(&week).map_or(&[][..], Vec::as_slice);
            out.extend(pools.iter().map(|&(pool, n)| (start, pool, n)));
        } else {
            out.extend(
                pool_weeks_scan(sql, lo.max(from), hi.min(to))?
                    .into_iter()
                    .filter(|w| w.0 == start),
            );
        }
    }
    Ok(Some(out))
}

/// [`super::first_since`] from the rest of `time`'s day and the days after it. `None` until the
/// fill is done.
pub(super) fn first_since(sql: &Sql, floor: u32, time: u64) -> Result<Option<Option<u32>>, Error> {
    if !ready(sql)? {
        return Ok(None);
    }
    let time = int(time);
    let day = time / DAY_SECONDS;
    let day_end = |d: i64| {
        d.saturating_mul(DAY_SECONDS)
            .saturating_add(DAY_SECONDS - 1)
    };
    // The rest of the day `time` falls in.
    let mut best = first_since_scan(sql, floor, time, day_end(day))?;
    // Later days: the lowest height of each that lies wholly at or above the floor, and the
    // lowest at or above it of each that straddles it.
    let mut stmt = sql.prepare(
        "SELECT key, first_height, last_height FROM block_totals \
         WHERE kind = ?1 AND key > ?2 AND last_height >= ?3",
    )?;
    let mut rows = stmt.query(params![DAY, day, floor])?;
    while let Some(r) = rows.next()? {
        let (key, lo, hi): (i64, u32, u32) = (r.get(0)?, get(r, 1)?, get(r, 2)?);
        let found = if lo >= floor {
            Some(lo)
        } else if hi >= floor {
            let start = key.saturating_mul(DAY_SECONDS);
            first_since_scan(sql, floor, start, day_end(key))?
        } else {
            None
        };
        best = match (best, found) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
    }
    Ok(Some(best))
}
