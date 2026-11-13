//! The block table's statements for the mining routes: how far the stored history reaches, pool
//! counts, retargets, work over a range, and redoing pool attribution when the list changes.

use bitcoin::BlockHash;

use super::Coinbase;
use super::db::{get, int, read_hash};
use crate::difficulty::INTERVAL;
use crate::mining::pools::Pools;
use crate::store::{Error, OptionalExtension, Read, Sql, Store, params};

/// Heights read per statement while walking the table.
const CHUNK: u32 = 10_000;

/// The stored history the mining answers cover: every height from `from` to `tip` has a row.
/// Rows below a missing height (the backfill is still filling in) are left out of every answer,
/// so a chart never shows a hole as a dip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub tip: u32,
    pub tip_hash: BlockHash,
    pub tip_time: u64,
    pub from: u32,
    pub from_time: u64,
}

/// `None` while the table is empty. Skips down the running totals' 1,008-height buckets that
/// hold every height, then walks the rows of the first one that does not to its missing height.
/// Before the totals are filled it walks every row: about 0.1 s on a full mainnet table.
pub async fn span(store: &Store) -> Result<Option<Span>, Error> {
    store
        .read(Read::Mining, |sql| {
            sql.prepare("BEGIN")?.execute([])?;
            let top = sql
                .prepare("SELECT height, hash, time FROM blocks ORDER BY height DESC LIMIT 1")?
                .query_row([], |r| {
                    Ok((get::<u32>(r, 0)?, read_hash(r, 1)?, get::<u64>(r, 2)?))
                })
                .optional()?;
            let Some((tip, tip_hash, tip_time)) = top else {
                return Ok(None);
            };
            let (mut from, mut from_time) = (tip, tip_time);
            if let Some(full) = super::totals::full_from(sql, tip)?
                && full < tip
            {
                from = full;
                from_time = sql
                    .prepare("SELECT time FROM blocks WHERE height = ?1")?
                    .query_row(params![full], |r| get(r, 0))?;
            }
            let mut walk = sql.prepare(
                "SELECT height, time FROM blocks WHERE height BETWEEN ?1 AND ?2 \
                 ORDER BY height DESC",
            )?;
            'chunks: loop {
                let lo = from.saturating_sub(CHUNK);
                let mut rows = walk.query(params![lo, from])?;
                while let Some(r) = rows.next()? {
                    let h: u32 = get(r, 0)?;
                    if h + 1 < from {
                        break 'chunks;
                    }
                    (from, from_time) = (h, get(r, 1)?);
                }
                if from == 0 || from > lo {
                    break;
                }
            }
            Ok(Some(Span {
                tip,
                tip_hash,
                tip_time,
                from,
                from_time,
            }))
        })
        .await
}

/// The highest row's height and hash: whether the chain the table holds has moved.
pub async fn top(store: &Store) -> Result<Option<(u32, BlockHash)>, Error> {
    store
        .read(Read::Mining, |sql| {
            Ok(sql
                .prepare("SELECT height, hash FROM blocks ORDER BY height DESC LIMIT 1")?
                .query_row([], |r| Ok((get(r, 0)?, read_hash(r, 1)?)))
                .optional()?)
        })
        .await
}

/// Rows in the table, and how many of them have fee rates: a sum of the totals' buckets.
pub async fn row_counts(store: &Store) -> Result<(u64, u64), Error> {
    store.read(Read::BlockCounts, super::db::counts).await
}

/// Blocks one pool mined in a height range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolCount {
    pub pool: u16,
    pub blocks: u32,
    /// Blocks holding only their coinbase.
    pub empty: u32,
    /// Sats the pool's blocks paid out.
    pub reward: u64,
}

/// Every pool with a block in `from..=to`, most blocks first, then by id. Over a long range the
/// whole 1,008-height buckets come from the running totals and only the ends are read row by row.
pub async fn pool_counts(store: &Store, from: u32, to: u32) -> Result<Vec<PoolCount>, Error> {
    store
        .read(Read::Mining, move |sql| {
            if super::totals::worth(from, to) {
                sql.prepare("BEGIN")?.execute([])?;
                if let Some(c) = super::totals::pool_counts(sql, from, to)? {
                    return Ok(c);
                }
            }
            pool_counts_scan(sql, from, to)
        })
        .await
}

/// [`pool_counts`] by reading every row in the range.
pub(super) fn pool_counts_scan(sql: &Sql, from: u32, to: u32) -> Result<Vec<PoolCount>, Error> {
    let mut stmt = sql.prepare(
        "SELECT pool, count(*) AS n, sum(tx_count = 1), sum(reward) FROM blocks \
         WHERE height BETWEEN ?1 AND ?2 GROUP BY pool ORDER BY n DESC, pool",
    )?;
    let rows = stmt.query_map(params![from, to], |r| {
        Ok(PoolCount {
            pool: get(r, 0)?,
            blocks: get(r, 1)?,
            empty: get(r, 2)?,
            reward: get(r, 3)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Seconds from the Unix epoch, a Thursday, to the first Monday: weeks here start on Monday.
pub const WEEK_OFFSET: u64 = 4 * 86_400;
pub const WEEK: u64 = 7 * 86_400;

/// One pool's blocks in one week: `(week start, pool, blocks)`.
pub type PoolWeek = (u64, u16, u32);

/// Blocks per pool per week (starting Monday 00:00 UTC) in `from..=to`: `(week start, pool,
/// blocks)`, oldest week first. Over a long range the weeks whose every block lies in it come from
/// the running totals.
pub async fn pool_weeks(store: &Store, from: u32, to: u32) -> Result<Vec<PoolWeek>, Error> {
    store
        .read(Read::Mining, move |sql| {
            if super::totals::worth(from, to) {
                sql.prepare("BEGIN")?.execute([])?;
                if let Some(w) = super::totals::pool_weeks(sql, from, to)? {
                    return Ok(w);
                }
            }
            pool_weeks_scan(sql, from, to)
        })
        .await
}

/// [`pool_weeks`] by reading every row in the range.
pub(super) fn pool_weeks_scan(sql: &Sql, from: u32, to: u32) -> Result<Vec<PoolWeek>, Error> {
    let mut stmt = sql.prepare(
        "SELECT (time - ?3) / ?4 AS w, pool, count(*) FROM blocks \
         WHERE height BETWEEN ?1 AND ?2 GROUP BY w, pool ORDER BY w, pool",
    )?;
    let rows = stmt.query_map(params![from, to, int(WEEK_OFFSET), int(WEEK)], |r| {
        let w: u64 = get(r, 0)?;
        Ok((w * WEEK + WEEK_OFFSET, get(r, 1)?, get(r, 2)?))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// One difficulty retarget.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Retarget {
    pub height: u32,
    pub time: u64,
    pub difficulty: f64,
    /// The difficulty of the period before, from its first block (on testnet4 its last may be a
    /// minimum-difficulty block).
    pub previous: f64,
}

/// Retargets at heights in `from..=to` whose previous period's first block is stored at or above
/// `floor` too, oldest first. `floor` is the bottom of the history the caller answers from, so a
/// retarget is never measured against a row below a gap.
pub async fn retargets(
    store: &Store,
    floor: u32,
    from: u32,
    to: u32,
) -> Result<Vec<Retarget>, Error> {
    let first = from.max(INTERVAL).div_ceil(INTERVAL) * INTERVAL;
    let heights: Vec<u32> = (first..=to).step_by(INTERVAL as usize).collect();
    let heights = serde_json::to_string(&heights).unwrap_or_default();
    store
        .read(Read::Mining, move |sql| {
            let mut stmt = sql.prepare(
                "SELECT a.height, a.time, a.difficulty, b.difficulty FROM blocks a \
                 JOIN blocks b ON b.height = a.height - ?2 \
                 WHERE a.height IN (SELECT value FROM json_each(?1)) AND b.height >= ?3 \
                 ORDER BY a.height",
            )?;
            let rows = stmt.query_map(params![heights, INTERVAL, floor], |r| {
                Ok(Retarget {
                    height: get(r, 0)?,
                    time: get(r, 1)?,
                    difficulty: r.get(2)?,
                    previous: r.get(3)?,
                })
            })?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .await
}

/// The proof of work done over a range: what a hashrate estimate needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Work {
    /// Blocks whose work counts: every one in the range but the first, whose time starts it.
    pub blocks: u32,
    pub difficulty_sum: f64,
    /// Lowest and highest timestamp in the range.
    pub min_time: u64,
    pub max_time: u64,
}

/// The work done in `from + 1..=to`, timed over `from..=to`. `None` with fewer than two rows.
pub async fn work(store: &Store, from: u32, to: u32) -> Result<Option<Work>, Error> {
    store
        .read(Read::Mining, move |sql| {
            let (n, sum, lo, hi): (i64, Option<f64>, Option<i64>, Option<i64>) = sql
                .prepare(
                    "SELECT count(*), sum(difficulty) FILTER (WHERE height > ?1), min(time), \
                     max(time) FROM blocks WHERE height BETWEEN ?1 AND ?2",
                )?
                .query_row(params![from, to], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })?;
            Ok(match (u32::try_from(n), sum, lo, hi) {
                (Ok(n), Some(sum), Some(lo), Some(hi)) if n >= 2 => Some(Work {
                    blocks: n - 1,
                    difficulty_sum: sum,
                    min_time: u64::try_from(lo).unwrap_or(0),
                    max_time: u64::try_from(hi).unwrap_or(0),
                }),
                _ => None,
            })
        })
        .await
}

/// The newest block at height `floor` or above stamped at or before `time`, by its timestamp:
/// `(height, hash, time)`. The floor keeps rows below a gap out of the choice.
pub async fn at_or_before(
    store: &Store,
    floor: u32,
    time: u64,
) -> Result<Option<(u32, BlockHash, u64)>, Error> {
    store
        .read(Read::Mining, move |sql| {
            Ok(sql
                .prepare(
                    "SELECT height, hash, time FROM blocks WHERE time <= ?1 AND height >= ?2 \
                     ORDER BY time DESC, height DESC LIMIT 1",
                )?
                .query_row(params![int(time), floor], |r| {
                    Ok((get(r, 0)?, read_hash(r, 1)?, get(r, 2)?))
                })
                .optional()?)
        })
        .await
}

/// The lowest height at `floor` or above stamped at or after `time`. With the totals, reads one
/// day of the `time` index and the days after it, not every row stamped since.
pub async fn first_since(store: &Store, floor: u32, time: u64) -> Result<Option<u32>, Error> {
    store
        .read(Read::Mining, move |sql| {
            sql.prepare("BEGIN")?.execute([])?;
            if let Some(h) = super::totals::first_since(sql, floor, time)? {
                return Ok(h);
            }
            first_since_scan(sql, floor, int(time), i64::MAX)
        })
        .await
}

/// The lowest height at `floor` or above stamped in `from..=to`.
pub(super) fn first_since_scan(
    sql: &Sql,
    floor: u32,
    from: i64,
    to: i64,
) -> Result<Option<u32>, Error> {
    let h: Option<i64> = sql
        // Walk the `time` index over the period, not every row from the floor up.
        .prepare(
            "SELECT min(height) FROM blocks INDEXED BY blocks_time \
             WHERE time BETWEEN ?1 AND ?2 AND height >= ?3",
        )?
        .query_row(params![from, to, floor], |r| r.get(0))?;
    Ok(h.and_then(|h| u32::try_from(h).ok()))
}

/// What [`reattribute`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Reattributed {
    pub rows: u64,
    pub changed: u64,
}

/// Recomputes every row's pool with `pools` unless the table was last attributed with this
/// list and these rules. Works down from the top in writes of [`CHUNK`] heights; rows written
/// meanwhile already use the build's list. A row replaced between the read and the write keeps
/// its new pool. `None` when nothing needed doing.
pub async fn reattribute(
    store: &Store,
    pools: &'static Pools,
) -> Result<Option<Reattributed>, Error> {
    let version = pools.version().to_owned();
    let (stored, top): (Option<String>, Option<i64>) = store
        .read(Read::Mining, |sql| {
            let v = sql
                .prepare("SELECT version FROM pool_list WHERE id = 0")?
                .query_row([], |r| r.get(0))
                .optional()?;
            let top = sql
                .prepare("SELECT max(height) FROM blocks")?
                .query_row([], |r| r.get(0))?;
            Ok((v, top))
        })
        .await?;
    if stored.as_deref() == Some(version.as_str()) {
        return Ok(None);
    }
    let mut done = Reattributed::default();
    let mut hi = top.and_then(|t| u32::try_from(t).ok());
    while let Some(h) = hi {
        let lo = h.saturating_sub(CHUNK - 1);
        let rows: Vec<(u32, BlockHash, u16, u16)> = store
            .read(Read::Mining, move |sql| {
                let mut stmt = sql.prepare(
                    "SELECT height, hash, coinbase_tag, payouts, pool FROM blocks \
                     WHERE height BETWEEN ?1 AND ?2",
                )?;
                let rows = stmt.query_map(params![lo, h], |r| {
                    let tag: Vec<u8> = r.get(2)?;
                    let payouts: Vec<u8> = r.get(3)?;
                    let (pool, _) = pools.attribute(&tag, &Coinbase::decode_payouts(&payouts));
                    Ok((get(r, 0)?, read_hash(r, 1)?, get(r, 4)?, pool))
                })?;
                Ok(rows.collect::<Result<_, _>>()?)
            })
            .await?;
        done.rows += rows.len() as u64;
        let changed: Vec<_> = rows.into_iter().filter(|r| r.2 != r.3).collect();
        if !changed.is_empty() {
            done.changed += changed.len() as u64;
            store
                .write(move |sql| {
                    let mut stmt =
                        sql.prepare("UPDATE blocks SET pool = ?1 WHERE height = ?2 AND hash = ?3")?;
                    for (height, hash, _, pool) in &changed {
                        stmt.execute(params![pool, height, &super::hash_bytes(hash)[..]])?;
                    }
                    Ok(())
                })
                .await?;
        }
        hi = lo.checked_sub(1);
    }
    store
        .write(move |sql| {
            sql.prepare("INSERT OR REPLACE INTO pool_list (id, version) VALUES (0, ?1)")?
                .execute(params![version])?;
            Ok(())
        })
        .await?;
    Ok(Some(done))
}
