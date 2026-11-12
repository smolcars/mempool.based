//! SQL for the `audits` table. Every statement is a literal; values are bound (SECURITY.md rule 12).

use bitcoin::BlockHash;
use bitcoin::hashes::Hash;
use rusqlite::types::Type;
use serde::Serialize;

use super::{Audit, Side, Tally, Totals, decode, encode};
use crate::store::{Error, OptionalExtension, Read, SqlError, Store, params};

/// The totals' columns, as [`read_totals`] reads them from index 0.
macro_rules! select {
    ($with:literal, $rest:literal) => {
        concat!(
            "SELECT hash, height, time, seen_at, match_rate, expected_txs, expected_vsize, ",
            "expected_weight, expected_fees, actual_txs, actual_vsize, actual_weight, actual_fees, ",
            "matched_txs, matched_vsize, missing_txs, missing_vsize, displaced_txs, displaced_vsize, ",
            "fresh_txs, fresh_vsize, conflicted_txs, conflicted_vsize, added_txs, added_vsize, ",
            "unseen_txs, unseen_vsize",
            $with,
            " FROM audits ",
            $rest
        )
    };
}

/// One block's score, for the pages and the series.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Scored {
    pub hash: BlockHash,
    pub height: u32,
    pub time: u32,
    pub match_rate: f64,
    pub expected_fees: u64,
    pub expected_weight: u64,
}

fn int(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn get<T: TryFrom<i64>>(r: &rusqlite::Row, i: usize) -> Result<T, SqlError> {
    let v: i64 = r.get(i)?;
    T::try_from(v).map_err(|_| SqlError::IntegralValueOutOfRange(i, v))
}

fn read_totals(r: &rusqlite::Row) -> Result<Audit, SqlError> {
    let side = |i| -> Result<Side, SqlError> {
        Ok(Side {
            txs: get(r, i)?,
            vsize: get(r, i + 1)?,
            weight: get(r, i + 2)?,
            fees: get(r, i + 3)?,
        })
    };
    let tally = |i| -> Result<Tally, SqlError> {
        Ok(Tally {
            txs: get(r, i)?,
            vsize: get(r, i + 1)?,
        })
    };
    let hash: [u8; 32] = r.get(0)?;
    Ok(Audit {
        id: BlockHash::from_byte_array(hash),
        height: get(r, 1)?,
        time: get(r, 2)?,
        seen_at: get(r, 3)?,
        totals: Totals {
            match_rate: r.get(4)?,
            expected: side(5)?,
            actual: side(9)?,
            matched: tally(13)?,
            missing: tally(15)?,
            displaced: tally(17)?,
            fresh: tally(19)?,
            conflicted: tally(21)?,
            added: tally(23)?,
            unseen: tally(25)?,
        },
        txs: Vec::new(),
    })
}

fn read_full(r: &rusqlite::Row) -> Result<Audit, SqlError> {
    let mut a = read_totals(r)?;
    let bytes: Vec<u8> = r.get(27)?;
    a.txs = decode(&bytes).ok_or_else(|| {
        SqlError::FromSqlConversionFailure(27, Type::Blob, "malformed audit records".into())
    })?;
    Ok(a)
}

/// Stores an audit unless the block has one; whether it was written. An audit is computed once,
/// when the block arrives, so the first one stands.
pub async fn write(store: &Store, a: Audit) -> Result<bool, Error> {
    store
        .write(move |sql| {
            let t = &a.totals;
            let n = sql
                .prepare(
                    "INSERT INTO audits VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, \
                     ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, \
                     ?28) ON CONFLICT DO NOTHING",
                )?
                .execute(params![
                    &a.id.as_byte_array()[..],
                    a.height,
                    a.time,
                    a.seen_at,
                    t.match_rate,
                    t.expected.txs,
                    int(t.expected.vsize),
                    int(t.expected.weight),
                    int(t.expected.fees),
                    t.actual.txs,
                    int(t.actual.vsize),
                    int(t.actual.weight),
                    int(t.actual.fees),
                    t.matched.txs,
                    int(t.matched.vsize),
                    t.missing.txs,
                    int(t.missing.vsize),
                    t.displaced.txs,
                    int(t.displaced.vsize),
                    t.fresh.txs,
                    int(t.fresh.vsize),
                    t.conflicted.txs,
                    int(t.conflicted.vsize),
                    t.added.txs,
                    int(t.added.vsize),
                    t.unseen.txs,
                    int(t.unseen.vsize),
                    encode(&a.txs),
                ])?;
            Ok(n == 1)
        })
        .await
}

/// One block's audit with its listed transactions.
pub async fn by_hash(store: &Store, hash: BlockHash) -> Result<Option<Audit>, Error> {
    store
        .read(Read::Audit, move |sql| {
            Ok(sql
                .prepare(select!(", txs", "WHERE hash = ?1"))?
                .query_row(params![&hash.as_byte_array()[..]], read_full)
                .optional()?)
        })
        .await
}

/// The totals of each block's audit, in order, without the lists.
pub async fn by_hashes(store: &Store, hashes: Vec<BlockHash>) -> Result<Vec<Option<Audit>>, Error> {
    store
        .read(Read::Audit, move |sql| {
            let mut stmt = sql.prepare(select!("", "WHERE hash = ?1"))?;
            let mut out = Vec::with_capacity(hashes.len());
            for h in &hashes {
                out.push(
                    stmt.query_row(params![&h.as_byte_array()[..]], read_totals)
                        .optional()?,
                );
            }
            Ok(out)
        })
        .await
}

fn read_scored(r: &rusqlite::Row) -> Result<Scored, SqlError> {
    let hash: [u8; 32] = r.get(0)?;
    Ok(Scored {
        hash: BlockHash::from_byte_array(hash),
        height: get(r, 1)?,
        time: get(r, 2)?,
        match_rate: r.get(3)?,
        expected_fees: get(r, 4)?,
        expected_weight: get(r, 5)?,
    })
}

/// Scores of the best chain's blocks at heights `from..=to`, lowest first: an audit whose block
/// is no longer the table's block at its height is left out.
pub async fn scored_between(store: &Store, from: u32, to: u32) -> Result<Vec<Scored>, Error> {
    store
        .read(Read::Audit, move |sql| {
            let mut stmt = sql.prepare(
                "SELECT a.hash, a.height, a.time, a.match_rate, a.expected_fees, \
                 a.expected_weight FROM audits a JOIN blocks b ON b.height = a.height \
                 AND b.hash = a.hash WHERE a.height BETWEEN ?1 AND ?2 ORDER BY a.height",
            )?;
            let rows = stmt.query_map(params![from, to], read_scored)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await
}

/// One pool's audited blocks over a height range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PoolHealth {
    pub pool: u16,
    pub audited: u32,
    /// The mean of their match rates, percent.
    pub match_rate: f64,
}

/// Each pool's audited best-chain blocks at heights `from..=to` and their mean match rate, for
/// pools with at least one. One grouped read over the audits, joined to the block table as the
/// pages are, so a block that left the best chain counts for nobody.
pub async fn pool_health(store: &Store, from: u32, to: u32) -> Result<Vec<PoolHealth>, Error> {
    store
        .read(Read::Audit, move |sql| {
            let mut stmt = sql.prepare(
                "SELECT b.pool, count(*), avg(a.match_rate) FROM audits a JOIN blocks b                  ON b.height = a.height AND b.hash = a.hash WHERE a.height BETWEEN ?1 AND ?2                  GROUP BY b.pool ORDER BY b.pool",
            )?;
            let rows = stmt.query_map(params![from, to], |r| {
                Ok(PoolHealth {
                    pool: get(r, 0)?,
                    audited: get(r, 1)?,
                    match_rate: r.get(2)?,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await
}

/// Scores of the best chain's blocks stamped at or after `since` (Unix seconds), lowest first.
pub async fn scored_since(store: &Store, since: u64) -> Result<Vec<Scored>, Error> {
    store
        .read(Read::Audit, move |sql| {
            let mut stmt = sql.prepare(
                "SELECT a.hash, a.height, a.time, a.match_rate, a.expected_fees, \
                 a.expected_weight FROM audits a JOIN blocks b ON b.height = a.height \
                 AND b.hash = a.hash WHERE a.time >= ?1 ORDER BY a.height",
            )?;
            let rows = stmt.query_map(params![int(since)], read_scored)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await
}
