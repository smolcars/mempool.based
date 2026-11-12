//! SQL for the `blocks` table. Every statement is a literal; values are bound (SECURITY.md rule 12).

use std::collections::HashMap;

use bitcoin::BlockHash;
use bitcoin::hashes::Hash;
use rusqlite::types::Type;

use super::stats::Rates;
use super::{Coinbase, Rated, Row, hash_bytes};
use crate::store::{Error, OptionalExtension, Sql, SqlError, Store, params};

/// Most rows [`page`] returns at once.
pub const MAX_PAGE: u32 = 1000;

/// The SELECT list [`read_row`] reads, followed by the rest of the statement.
macro_rules! select {
    ($rest:literal) => {
        concat!(
            "SELECT height, hash, time, tx_count, size, weight, paying_weight, fees, subsidy, ",
            "reward, difficulty, coinbase_tag, payouts, rated, fee_min, fee_p10, fee_p25, ",
            "fee_median, fee_p75, fee_p90, fee_max, band0, band1, band2, band3, band4, band5, ",
            "band6, band7, band8, band9 FROM blocks ",
            $rest
        )
    };
}

macro_rules! insert {
    ($verb:literal, $tail:literal) => {
        concat!(
            $verb,
            " INTO blocks (height, hash, time, tx_count, size, weight, paying_weight, fees, ",
            "subsidy, reward, difficulty, coinbase_tag, payouts, rated, fee_min, fee_p10, fee_p25, ",
            "fee_median, fee_p75, fee_p90, fee_max, band0, band1, band2, band3, band4, band5, ",
            "band6, band7, band8, band9) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ",
            "?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ",
            "?29, ?30, ?31)",
            $tail
        )
    };
}

/// Adds rows the table lacks and leaves every existing row alone: the backfill's write.
pub(super) const INSERT_NEW: &str = insert!("INSERT", " ON CONFLICT DO NOTHING");
/// Writes rows over whatever held their height or hash: [`super::follow`]'s write.
pub(super) const REPLACE: &str = insert!("INSERT OR REPLACE", "");
/// Rates one row, only if it still holds the block they were computed from.
pub(super) const RATE: &str = "UPDATE blocks SET rated = 1, fee_min = ?3, fee_p10 = ?4, \
    fee_p25 = ?5, fee_median = ?6, fee_p75 = ?7, fee_p90 = ?8, fee_max = ?9, band0 = ?10, \
    band1 = ?11, band2 = ?12, band3 = ?13, band4 = ?14, band5 = ?15, band6 = ?16, band7 = ?17, \
    band8 = ?18, band9 = ?19 WHERE height = ?1 AND hash = ?2";

/// SQLite integers are `i64`; no value stored here comes near the limit.
fn int(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn rate_columns(r: Option<&Rated>) -> ([Option<f64>; 7], [Option<i64>; 10]) {
    let rates = r
        .and_then(|r| r.fee_rate)
        .map(|f| [f.min, f.p10, f.p25, f.median, f.p75, f.p90, f.max].map(Some));
    (
        rates.unwrap_or([None; 7]),
        r.map_or([None; 10], |r| r.fee_bands.map(|b| Some(int(b)))),
    )
}

/// Runs [`INSERT_NEW`] or [`REPLACE`] for one row; the number of rows it changed.
pub(super) fn write_row(stmt: &mut rusqlite::Statement, row: &Row) -> Result<usize, SqlError> {
    let (f, b) = rate_columns(row.rates.as_ref());
    stmt.execute(params![
        row.height,
        &hash_bytes(&row.hash)[..],
        int(row.time),
        row.tx_count,
        int(row.size),
        int(row.weight),
        int(row.paying_weight),
        int(row.fees),
        int(row.subsidy),
        int(row.reward),
        row.difficulty,
        row.coinbase.tag,
        row.coinbase.encode_payouts(),
        row.rates.is_some(),
        f[0],
        f[1],
        f[2],
        f[3],
        f[4],
        f[5],
        f[6],
        b[0],
        b[1],
        b[2],
        b[3],
        b[4],
        b[5],
        b[6],
        b[7],
        b[8],
        b[9],
    ])
}

/// Runs [`RATE`]; whether the row still held that block.
pub(super) fn rate_row(
    stmt: &mut rusqlite::Statement,
    height: u32,
    hash: &BlockHash,
    rated: &Rated,
) -> Result<bool, SqlError> {
    let (f, b) = rate_columns(Some(rated));
    let n = stmt.execute(params![
        height,
        &hash_bytes(hash)[..],
        f[0],
        f[1],
        f[2],
        f[3],
        f[4],
        f[5],
        f[6],
        b[0],
        b[1],
        b[2],
        b[3],
        b[4],
        b[5],
        b[6],
        b[7],
        b[8],
        b[9],
    ])?;
    Ok(n > 0)
}

fn read_hash(r: &rusqlite::Row, i: usize) -> Result<BlockHash, SqlError> {
    let bytes: [u8; 32] = r.get(i)?;
    Ok(BlockHash::from_byte_array(bytes))
}

/// Reads a non-negative integer column into `T`, refusing a value that does not fit.
fn get<T: TryFrom<i64>>(r: &rusqlite::Row, i: usize) -> Result<T, SqlError> {
    let v: i64 = r.get(i)?;
    T::try_from(v).map_err(|_| SqlError::IntegralValueOutOfRange(i, v))
}

fn read_row(r: &rusqlite::Row) -> Result<Row, SqlError> {
    let rated: bool = r.get(13)?;
    let rates = if rated {
        let f: [Option<f64>; 7] = [
            r.get(14)?,
            r.get(15)?,
            r.get(16)?,
            r.get(17)?,
            r.get(18)?,
            r.get(19)?,
            r.get(20)?,
        ];
        let mut bands = [0u64; 10];
        for (i, b) in bands.iter_mut().enumerate() {
            *b = get(r, 21 + i)?;
        }
        let fee_rate = match f {
            [
                Some(min),
                Some(p10),
                Some(p25),
                Some(median),
                Some(p75),
                Some(p90),
                Some(max),
            ] => Some(Rates {
                min,
                p10,
                p25,
                median,
                p75,
                p90,
                max,
            }),
            [None, None, None, None, None, None, None] => None,
            _ => {
                return Err(SqlError::InvalidColumnType(
                    14,
                    "fee_min".into(),
                    Type::Null,
                ));
            }
        };
        Some(Rated {
            fee_rate,
            fee_bands: bands,
        })
    } else {
        None
    };
    let payouts: Vec<u8> = r.get(12)?;
    Ok(Row {
        height: get(r, 0)?,
        hash: read_hash(r, 1)?,
        time: get(r, 2)?,
        tx_count: get(r, 3)?,
        size: get(r, 4)?,
        weight: get(r, 5)?,
        paying_weight: get(r, 6)?,
        fees: get(r, 7)?,
        subsidy: get(r, 8)?,
        reward: get(r, 9)?,
        difficulty: r.get(10)?,
        coinbase: Coinbase {
            tag: r.get(11)?,
            payouts: Coinbase::decode_payouts(&payouts),
        },
        rates,
    })
}

/// The row at `height`.
pub async fn by_height(store: &Store, height: u32) -> Result<Option<Row>, Error> {
    store
        .read(move |sql| {
            Ok(sql
                .prepare(select!("WHERE height = ?1"))?
                .query_row(params![height], read_row)
                .optional()?)
        })
        .await
}

/// The row of the block `hash`, if it is on the best chain and stored.
pub async fn by_hash(store: &Store, hash: BlockHash) -> Result<Option<Row>, Error> {
    Ok(by_hashes(store, vec![hash]).await?.pop().flatten())
}

/// The rows of these blocks, in the order asked; `None` where there is none.
pub async fn by_hashes(store: &Store, hashes: Vec<BlockHash>) -> Result<Vec<Option<Row>>, Error> {
    store
        .read(move |sql| {
            let mut stmt = sql.prepare(select!("WHERE hash = ?1"))?;
            let mut out = Vec::with_capacity(hashes.len());
            for h in &hashes {
                out.push(
                    stmt.query_row(params![&hash_bytes(h)[..]], read_row)
                        .optional()?,
                );
            }
            Ok(out)
        })
        .await
}

/// Up to `limit` rows (at most [`MAX_PAGE`]) below height `below`, or from the top when `None`,
/// newest first. The next page is the one below the last row's height. Heights missing from the
/// table are skipped, so a page can span more than `limit` heights while the backfill runs.
pub async fn page(store: &Store, below: Option<u32>, limit: u32) -> Result<Vec<Row>, Error> {
    let below = below.map_or(i64::MAX, i64::from);
    let limit = limit.min(MAX_PAGE);
    store
        .read(move |sql| {
            let mut stmt =
                sql.prepare(select!("WHERE height < ?1 ORDER BY height DESC LIMIT ?2"))?;
            let rows = stmt.query_map(params![below, limit], read_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await
}

/// The lowest and highest height of the blocks timestamped in `from..=to` (Unix seconds).
/// Timestamps may run a little out of order between neighbouring blocks; this takes them as
/// stamped.
pub async fn heights_between(
    store: &Store,
    from: u64,
    to: u64,
) -> Result<Option<(u32, u32)>, Error> {
    store
        .read(move |sql| {
            let (lo, hi): (Option<i64>, Option<i64>) = sql
                .prepare(
                    "SELECT min(height), max(height) FROM blocks WHERE time BETWEEN ?1 AND ?2",
                )?
                .query_row(params![int(from), int(to)], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(lo
                .zip(hi)
                .and_then(|(lo, hi)| Some((u32::try_from(lo).ok()?, u32::try_from(hi).ok()?))))
        })
        .await
}

/// How [`periods`] groups blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    /// By UTC day of the block's timestamp. A bucket's key is days since 1970-01-01.
    Day,
    /// By height: blocks `k*n .. k*n+n-1` share bucket `k`. `n` is at least 1.
    Blocks(u32),
}

/// Totals and averages over one period's stored blocks.
#[derive(Debug, Clone, PartialEq)]
pub struct Bucket {
    /// Day number or `height / n`, per [`Period`].
    pub key: i64,
    pub blocks: u32,
    pub first_height: u32,
    pub last_height: u32,
    /// Lowest and highest timestamp; the average block interval is their difference over
    /// `blocks - 1`.
    pub first_time: u64,
    pub last_time: u64,
    pub tx_count: u64,
    pub size: u64,
    pub weight: u64,
    pub fees: u64,
    pub subsidy: u64,
    pub reward: u64,
    /// Mean difficulty; hashrate is about `difficulty * 2^32 / interval`.
    pub difficulty: f64,
    /// Blocks in the bucket with fee rates; the averages below cover only them.
    pub rated: u32,
    /// Each percentile averaged over the rated blocks that pay fees, unweighted.
    pub fee_rate: Option<Rates>,
}

/// Aggregates over heights `from..=to`, oldest bucket first. Reads only the stored rows in that
/// range; a one-year daily aggregate on a full mainnet table takes milliseconds
/// (`docs/STORE.md`).
pub async fn periods(store: &Store, from: u32, to: u32, by: Period) -> Result<Vec<Bucket>, Error> {
    let n: u32 = match by {
        Period::Day => 0,
        Period::Blocks(n) => n.max(1),
    };
    store
        .read(move |sql| {
            let mut stmt = sql.prepare(
                "SELECT CASE WHEN ?3 = 0 THEN time / 86400 ELSE height / ?3 END AS k, count(*), \
                 min(height), max(height), min(time), max(time), sum(tx_count), sum(size), \
                 sum(weight), sum(fees), sum(subsidy), sum(reward), avg(difficulty), sum(rated), \
                 avg(fee_min), avg(fee_p10), avg(fee_p25), avg(fee_median), avg(fee_p75), \
                 avg(fee_p90), avg(fee_max) \
                 FROM blocks WHERE height BETWEEN ?1 AND ?2 GROUP BY k ORDER BY k",
            )?;
            let rows = stmt.query_map(params![from, to, n], |r| {
                let f: [Option<f64>; 7] = [
                    r.get(14)?,
                    r.get(15)?,
                    r.get(16)?,
                    r.get(17)?,
                    r.get(18)?,
                    r.get(19)?,
                    r.get(20)?,
                ];
                let fee_rate = match f {
                    [
                        Some(min),
                        Some(p10),
                        Some(p25),
                        Some(median),
                        Some(p75),
                        Some(p90),
                        Some(max),
                    ] => Some(Rates {
                        min,
                        p10,
                        p25,
                        median,
                        p75,
                        p90,
                        max,
                    }),
                    _ => None,
                };
                Ok(Bucket {
                    key: r.get(0)?,
                    blocks: get(r, 1)?,
                    first_height: get(r, 2)?,
                    last_height: get(r, 3)?,
                    first_time: get(r, 4)?,
                    last_time: get(r, 5)?,
                    tx_count: get(r, 6)?,
                    size: get(r, 7)?,
                    weight: get(r, 8)?,
                    fees: get(r, 9)?,
                    subsidy: get(r, 10)?,
                    reward: get(r, 11)?,
                    difficulty: r.get(12)?,
                    rated: get(r, 13)?,
                    fee_rate,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await
}

/// Stored hashes and whether each row is rated, for heights `lo..=hi`.
pub(super) fn stored(
    sql: &Sql,
    lo: u32,
    hi: u32,
) -> Result<HashMap<u32, (BlockHash, bool)>, Error> {
    let mut stmt =
        sql.prepare("SELECT height, hash, rated FROM blocks WHERE height BETWEEN ?1 AND ?2")?;
    let rows = stmt.query_map(params![lo, hi], |r| {
        Ok((get::<u32>(r, 0)?, (read_hash(r, 1)?, r.get::<_, bool>(2)?)))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Deletes the row at `height` if it still holds `hash`.
pub(super) fn delete_row(sql: &Sql, height: u32, hash: &BlockHash) -> Result<usize, Error> {
    Ok(sql
        .prepare("DELETE FROM blocks WHERE height = ?1 AND hash = ?2")?
        .execute(params![height, &hash_bytes(hash)[..]])?)
}

/// The highest height at or below `h` that has a row.
pub(super) fn highest(sql: &Sql, h: u32) -> Result<Option<u32>, Error> {
    let top: Option<i64> = sql
        .prepare("SELECT max(height) FROM blocks WHERE height <= ?1")?
        .query_row(params![h], |r| r.get(0))?;
    Ok(top.and_then(|t| u32::try_from(t).ok()))
}

/// Heights in `lo..=hi` that have a row.
pub(super) fn present(sql: &Sql, lo: u32, hi: u32) -> Result<Vec<u32>, Error> {
    let mut stmt = sql.prepare("SELECT height FROM blocks WHERE height BETWEEN ?1 AND ?2")?;
    let rows = stmt.query_map(params![lo, hi], |r| get::<u32>(r, 0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The highest unrated row in `lo..=hi`.
pub(super) fn next_unrated(sql: &Sql, lo: u32, hi: u32) -> Result<Option<(u32, BlockHash)>, Error> {
    Ok(sql
        .prepare(
            "SELECT height, hash FROM blocks WHERE rated = 0 AND height BETWEEN ?1 AND ?2 \
             ORDER BY height DESC LIMIT 1",
        )?
        .query_row(params![lo, hi], |r| {
            Ok((get::<u32>(r, 0)?, read_hash(r, 1)?))
        })
        .optional()?)
}

/// Rows in the table, and how many of them are rated.
pub(super) fn counts(sql: &Sql) -> Result<(u64, u64), Error> {
    let rows: i64 = sql
        .prepare("SELECT count(*) FROM blocks")?
        .query_row([], |r| r.get(0))?;
    let unrated: i64 = sql
        .prepare("SELECT count(*) FROM blocks WHERE rated = 0")?
        .query_row([], |r| r.get(0))?;
    let rows = u64::try_from(rows).unwrap_or(0);
    Ok((
        rows,
        rows.saturating_sub(u64::try_from(unrated).unwrap_or(0)),
    ))
}
