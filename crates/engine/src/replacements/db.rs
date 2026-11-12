//! SQL for the `replacements` table. Every statement is a literal; values are bound
//! (SECURITY.md rule 12).

use bitcoin::Txid;
use bitcoin::hashes::Hash;

use super::{Facts, Link};
use crate::store::{Error, Sql, SqlError, Store, params};

const INSERT: &str = "INSERT OR REPLACE INTO replacements (txid, by, seen, fee, vsize, weight, \
    value, time, rbf, by_fee, by_vsize, by_weight, by_value, by_time, by_rbf, mined) \
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)";

const MINED: &str = "UPDATE replacements SET mined = 1 WHERE by = ?1";
const UNMINED: &str = "UPDATE replacements SET mined = 0 WHERE by = ?1";
const RETURNED: &str = "DELETE FROM replacements WHERE txid = ?1";

/// Keeps the newest ?1 rows by the order recorded. Rows replaced in place leave gaps, so it may
/// keep fewer, never more.
const PRUNE: &str = "DELETE FROM replacements WHERE n <= (SELECT max(n) FROM replacements) - ?1";

const LOAD: &str = "SELECT txid, by, seen, fee, vsize, weight, value, time, rbf, by_fee, \
    by_vsize, by_weight, by_value, by_time, by_rbf, mined FROM replacements \
    ORDER BY n DESC LIMIT ?1";

/// What one tracker delta changed, applied in this order, as the record applied it: new links,
/// replacements seen confirmed, then transactions back in the mempool.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Batch {
    pub links: Vec<Link>,
    pub mined: Vec<Txid>,
    pub returned: Vec<Txid>,
}

impl Batch {
    pub fn is_empty(&self) -> bool {
        self.links.is_empty() && self.mined.is_empty() && self.returned.is_empty()
    }
}

fn int(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

/// Stores `batches`, in order, in one transaction and keeps the newest `keep` rows.
pub async fn write(store: &Store, batches: Vec<Batch>, keep: usize) -> Result<(), Error> {
    store
        .write(move |sql| {
            for b in &batches {
                insert(sql, b)?;
            }
            prune(sql, keep)
        })
        .await
}

/// One batch's statements, inside a write's transaction.
pub(crate) fn insert(sql: &Sql, batch: &Batch) -> Result<(), Error> {
    let (mut insert, mut stale) = (sql.prepare(INSERT)?, sql.prepare(RETURNED)?);
    for l in &batch.links {
        // As `Replacements::restore`: the replacement is back, so its own old link is stale.
        stale.execute(params![l.by.to_byte_array()])?;
        let by = l.by_tx;
        insert.execute(params![
            l.txid.to_byte_array(),
            l.by.to_byte_array(),
            int(l.seen),
            int(l.tx.fee),
            l.tx.vsize,
            l.tx.weight,
            int(l.tx.value),
            l.tx.time,
            l.tx.rbf,
            by.map(|f| int(f.fee)),
            by.map(|f| f.vsize),
            by.map(|f| f.weight),
            by.map(|f| int(f.value)),
            by.map(|f| f.time),
            by.map(|f| f.rbf),
            l.mined,
        ])?;
    }
    let mut mined = sql.prepare(MINED)?;
    for t in &batch.mined {
        mined.execute(params![t.to_byte_array()])?;
    }
    let (mut unmined, mut returned) = (sql.prepare(UNMINED)?, sql.prepare(RETURNED)?);
    for t in &batch.returned {
        returned.execute(params![t.to_byte_array()])?;
        unmined.execute(params![t.to_byte_array()])?;
    }
    Ok(())
}

fn prune(sql: &Sql, keep: usize) -> Result<(), Error> {
    sql.prepare(PRUNE)?.execute(params![int(keep as u64)])?;
    Ok(())
}

fn get<T: TryFrom<i64>>(r: &rusqlite::Row, i: usize) -> Result<T, SqlError> {
    let v: i64 = r.get(i)?;
    T::try_from(v).map_err(|_| SqlError::IntegralValueOutOfRange(i, v))
}

fn read(r: &rusqlite::Row) -> Result<Link, SqlError> {
    let txid: [u8; 32] = r.get(0)?;
    let by: [u8; 32] = r.get(1)?;
    let by_fee: Option<i64> = r.get(9)?;
    let by_tx = match by_fee {
        None => None,
        Some(_) => Some(Facts {
            fee: get(r, 9)?,
            vsize: get(r, 10)?,
            weight: get(r, 11)?,
            value: get(r, 12)?,
            time: get(r, 13)?,
            rbf: r.get(14)?,
        }),
    };
    Ok(Link {
        txid: Txid::from_byte_array(txid),
        by: Txid::from_byte_array(by),
        seen: get(r, 2)?,
        tx: Facts {
            fee: get(r, 3)?,
            vsize: get(r, 4)?,
            weight: get(r, 5)?,
            value: get(r, 6)?,
            time: get(r, 7)?,
            rbf: r.get(8)?,
        },
        by_tx,
        mined: r.get(15)?,
    })
}

/// The newest `keep` links, oldest first, to put back into a record.
pub async fn load(store: &Store, keep: usize) -> Result<Vec<Link>, Error> {
    store
        .read(move |sql| {
            let mut out = sql
                .prepare(LOAD)?
                .query_map(params![int(keep as u64)], read)?
                .collect::<Result<Vec<_>, _>>()?;
            out.reverse();
            Ok(out)
        })
        .await
}
