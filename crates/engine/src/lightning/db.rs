//! SQL for `ln_channels` and `ln_statistics`. Every statement is a literal; values are bound
//! (SECURITY.md rule 12).

use bitcoin::hashes::Hash;
use bitcoin::{BlockHash, Txid};

use super::{
    Block, Change, Channel, Close, DAY, EVERY, HOUR, Interval, KEEP_HOURS, KEEP_MINUTES, Kind,
    Level, Open, Penalty, Point, Series, Stats,
};
use crate::history::Rolled;
use crate::store::{Error, Sql, SqlError, Store, params};

/// Opens a channel: inserts it, or makes the stored row match and clears any close, which the
/// chain no longer holds. A row that already matches is not written again.
const OPEN: &str = "INSERT INTO ln_channels (scid, node_one, node_two, capacity, open_hash, open_time) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT (scid) DO UPDATE SET \
     node_one = excluded.node_one, node_two = excluded.node_two, capacity = excluded.capacity, \
     open_hash = excluded.open_hash, open_time = excluded.open_time, close_height = NULL, \
     close_hash = NULL, close_time = NULL, close_txid = NULL, close_kind = NULL, close_fee = NULL, \
     penalty_txid = NULL, penalty_height = NULL, penalty_hash = NULL, penalty_time = NULL \
     WHERE (node_one, node_two, capacity, open_hash, open_time, close_height, penalty_txid) \
     IS NOT (excluded.node_one, excluded.node_two, excluded.capacity, excluded.open_hash, \
     excluded.open_time, NULL, NULL)";

/// Closes a channel, inserting it if it was never stored open. A penalty already stored stays
/// when the close is the same transaction (the close found again after a restart).
const CLOSE: &str = "INSERT INTO ln_channels (scid, node_one, node_two, capacity, open_hash, open_time, \
     close_height, close_hash, close_time, close_txid, close_kind, close_fee) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) ON CONFLICT (scid) DO UPDATE SET \
     node_one = excluded.node_one, node_two = excluded.node_two, capacity = excluded.capacity, \
     open_hash = excluded.open_hash, open_time = excluded.open_time, \
     close_height = excluded.close_height, close_hash = excluded.close_hash, \
     close_time = excluded.close_time, close_txid = excluded.close_txid, \
     close_kind = excluded.close_kind, close_fee = excluded.close_fee, \
     penalty_txid = CASE WHEN close_txid IS excluded.close_txid THEN penalty_txid END, \
     penalty_height = CASE WHEN close_txid IS excluded.close_txid THEN penalty_height END, \
     penalty_hash = CASE WHEN close_txid IS excluded.close_txid THEN penalty_hash END, \
     penalty_time = CASE WHEN close_txid IS excluded.close_txid THEN penalty_time END";

/// SQLite integers are signed; nothing stored here comes near 2^63.
fn int(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn open_params(o: &Open) -> [rusqlite::types::Value; 6] {
    [
        int(o.scid).into(),
        o.node_one.to_vec().into(),
        o.node_two.to_vec().into(),
        int(o.capacity).into(),
        o.block.hash.to_byte_array().to_vec().into(),
        int(o.block.time).into(),
    ]
}

fn change(sql: &Sql, c: &Change) -> Result<(), Error> {
    match c {
        Change::Opened(o) => {
            sql.prepare(OPEN)?.execute(open_params(o))?;
        }
        Change::Closed(o, c) => {
            let close: [rusqlite::types::Value; 6] = [
                i64::from(c.block.height).into(),
                c.block.hash.to_byte_array().to_vec().into(),
                int(c.block.time).into(),
                c.txid.to_byte_array().to_vec().into(),
                (c.kind as i64).into(),
                c.fee.map(int).into(),
            ];
            sql.prepare(CLOSE)?.execute(rusqlite::params_from_iter(
                open_params(o).into_iter().chain(close),
            ))?;
        }
        Change::Penalty(scid, p) => {
            sql.prepare(
                "UPDATE ln_channels SET penalty_txid = ?2, penalty_height = ?3, penalty_hash = ?4, \
                 penalty_time = ?5 WHERE scid = ?1 AND close_height IS NOT NULL",
            )?
            .execute(params![
                int(*scid),
                p.txid.to_byte_array().to_vec(),
                p.block.height,
                p.block.hash.to_byte_array().to_vec(),
                int(p.block.time)
            ])?;
        }
        Change::OpenUndone(scid) => {
            sql.prepare("DELETE FROM ln_channels WHERE scid = ?1")?
                .execute(params![int(*scid)])?;
        }
        Change::CloseUndone(scid) => {
            sql.prepare(
                "UPDATE ln_channels SET close_height = NULL, close_hash = NULL, close_time = NULL, \
                 close_txid = NULL, close_kind = NULL, close_fee = NULL, penalty_txid = NULL, \
                 penalty_height = NULL, penalty_hash = NULL, penalty_time = NULL WHERE scid = ?1",
            )?
            .execute(params![int(*scid)])?;
        }
        Change::PenaltyUndone(scid) => {
            sql.prepare(
                "UPDATE ln_channels SET penalty_txid = NULL, penalty_height = NULL, \
                 penalty_hash = NULL, penalty_time = NULL WHERE scid = ?1",
            )?
            .execute(params![int(*scid)])?;
        }
    }
    Ok(())
}

/// Applies changes in order, in one write. Fails with [`Error::Full`] rather than wait behind a
/// full write queue, so the block table's and the mempool history's writes never wait for it.
pub async fn apply(store: &Store, changes: Vec<Change>) -> Result<(), Error> {
    store
        .try_write(move |sql| changes.iter().try_for_each(|c| change(sql, c)))
        .await
}

fn get(r: &rusqlite::Row, i: usize) -> Result<u64, SqlError> {
    let v: i64 = r.get(i)?;
    u64::try_from(v).map_err(|_| SqlError::IntegralValueOutOfRange(i, v))
}

fn bytes<const N: usize>(r: &rusqlite::Row, i: usize) -> Result<[u8; N], SqlError> {
    let v: Vec<u8> = r.get(i)?;
    v.try_into()
        .map_err(|_| SqlError::InvalidColumnType(i, "blob".into(), rusqlite::types::Type::Blob))
}

fn height(r: &rusqlite::Row, i: usize) -> Result<u32, SqlError> {
    let v: i64 = r.get(i)?;
    u32::try_from(v).map_err(|_| SqlError::IntegralValueOutOfRange(i, v))
}

/// A row of the channel columns, in the table's order.
fn channel(r: &rusqlite::Row) -> Result<Channel, SqlError> {
    let scid = get(r, 0)?;
    let open = Open {
        scid,
        node_one: bytes(r, 1)?,
        node_two: bytes(r, 2)?,
        capacity: get(r, 3)?,
        block: Block {
            height: u32::try_from(scid >> 40).unwrap_or(u32::MAX),
            hash: BlockHash::from_byte_array(bytes(r, 4)?),
            time: get(r, 5)?,
        },
    };
    let close = match r.get::<_, Option<i64>>(6)? {
        None => None,
        Some(_) => Some(Close {
            block: Block {
                height: height(r, 6)?,
                hash: BlockHash::from_byte_array(bytes(r, 7)?),
                time: get(r, 8)?,
            },
            txid: Txid::from_byte_array(bytes(r, 9)?),
            kind: Kind::from_code(r.get(10)?).unwrap_or(Kind::Other),
            fee: r
                .get::<_, Option<i64>>(11)?
                .and_then(|f| u64::try_from(f).ok()),
        }),
    };
    let penalty = match r.get::<_, Option<Vec<u8>>>(12)? {
        None => None,
        Some(_) => Some(Penalty {
            txid: Txid::from_byte_array(bytes(r, 12)?),
            block: Block {
                height: height(r, 13)?,
                hash: BlockHash::from_byte_array(bytes(r, 14)?),
                time: get(r, 15)?,
            },
        }),
    };
    Ok(Channel {
        open,
        close,
        penalty,
    })
}

/// Up to `limit` channels, newest funding first.
pub async fn opened(store: &Store, limit: usize) -> Result<Vec<Channel>, Error> {
    store
        .read(move |sql| {
            Ok(sql
                .prepare(concat!(
                    "SELECT scid, node_one, node_two, capacity, open_hash, open_time, ",
                    "close_height, close_hash, close_time, close_txid, close_kind, close_fee, ",
                    "penalty_txid, penalty_height, penalty_hash, penalty_time FROM ln_channels ",
                    "ORDER BY scid DESC LIMIT ?1"
                ))?
                .query_map(params![int(limit as u64)], channel)?
                .collect::<Result<Vec<_>, _>>()?)
        })
        .await
}

/// Up to `limit` closed channels, newest close first.
pub async fn closed(store: &Store, limit: usize) -> Result<Vec<Channel>, Error> {
    store
        .read(move |sql| {
            Ok(sql
                .prepare(concat!(
                    "SELECT scid, node_one, node_two, capacity, open_hash, open_time, ",
                    "close_height, close_hash, close_time, close_txid, close_kind, close_fee, ",
                    "penalty_txid, penalty_height, penalty_hash, penalty_time FROM ln_channels ",
                    "WHERE close_height IS NOT NULL ORDER BY close_height DESC, scid DESC LIMIT ?1"
                ))?
                .query_map(params![int(limit as u64)], channel)?
                .collect::<Result<Vec<_>, _>>()?)
        })
        .await
}

// Statistics: the same shape and roll-up as `mempool_history` (crate::history::db).

macro_rules! stat_columns {
    () => {
        "samples, channels, nodes, capacity, tor_nodes, clearnet_nodes, clearnet_tor_nodes, \
         unannounced_nodes, med_capacity, policies, fee_rate, base_fee, med_fee_rate, med_base_fee"
    };
}

macro_rules! stat_sums {
    () => {
        "sum(samples), sum(channels), sum(nodes), sum(capacity), sum(tor_nodes), \
         sum(clearnet_nodes), sum(clearnet_tor_nodes), sum(unannounced_nodes), sum(med_capacity), \
         sum(policies), sum(fee_rate), sum(base_fee), sum(med_fee_rate), sum(med_base_fee)"
    };
}

const INSERT: &str = concat!(
    "INSERT OR IGNORE INTO ln_statistics (level, time, ",
    stat_columns!(),
    ") VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)"
);

/// Adds the rows of level ?1 older than ?4 into level ?2 rows of ?3 seconds, merging with any
/// already there.
const ROLL: &str = concat!(
    "INSERT INTO ln_statistics (level, time, ",
    stat_columns!(),
    ") SELECT ?2, time / ?3 * ?3, ",
    stat_sums!(),
    " FROM ln_statistics WHERE level = ?1 AND time < ?4 GROUP BY time / ?3 ",
    "ON CONFLICT (level, time) DO UPDATE SET samples = samples + excluded.samples, ",
    "channels = channels + excluded.channels, nodes = nodes + excluded.nodes, ",
    "capacity = capacity + excluded.capacity, tor_nodes = tor_nodes + excluded.tor_nodes, ",
    "clearnet_nodes = clearnet_nodes + excluded.clearnet_nodes, ",
    "clearnet_tor_nodes = clearnet_tor_nodes + excluded.clearnet_tor_nodes, ",
    "unannounced_nodes = unannounced_nodes + excluded.unannounced_nodes, ",
    "med_capacity = med_capacity + excluded.med_capacity, ",
    "policies = policies + excluded.policies, fee_rate = fee_rate + excluded.fee_rate, ",
    "base_fee = base_fee + excluded.base_fee, ",
    "med_fee_rate = med_fee_rate + excluded.med_fee_rate, ",
    "med_base_fee = med_base_fee + excluded.med_base_fee"
);

fn insert_row(sql: &Sql, level: Level, p: &Point) -> Result<usize, Error> {
    let values = [p.time, p.samples]
        .into_iter()
        .chain(p.sums.values())
        .map(int);
    Ok(sql.prepare(INSERT)?.execute(rusqlite::params_from_iter(
        std::iter::once(level as i64).chain(values),
    ))?)
}

/// Rolls every row past its level's retention, as of `now`, into the next level, in whole
/// periods: ten-minute rows into hours after 8 days, hours into days after 400.
fn roll(sql: &Sql, now: u64) -> Result<Rolled, Error> {
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
            .prepare("DELETE FROM ln_statistics WHERE level = ?1 AND time < ?2")?
            .execute(params![from as i64, int(cut)])?;
    }
    Ok(rolled)
}

/// Stores one snapshot and rolls up what has aged, in one write. Fails with [`Error::Full`]
/// rather than wait behind a full write queue: a missed snapshot is a gap, not a delay. A
/// snapshot already stored is left as it is, and so is one whose hour or day was rolled up
/// already (the clock was ahead and came back), which it would otherwise count twice.
pub async fn record(store: &Store, snapshot: Point, now: u64) -> Result<Rolled, Error> {
    store
        .try_write(move |sql| {
            let rolled: bool = sql
                .prepare(
                    "SELECT EXISTS (SELECT 1 FROM ln_statistics WHERE \
                     (level = 1 AND time = ?1 / 3600 * 3600) OR (level = 2 AND time = ?1 / 86400 * 86400))",
                )?
                .query_row(params![int(snapshot.time)], |r| r.get(0))?;
            if !rolled {
                insert_row(sql, Level::Minute, &snapshot)?;
            }
            roll(sql, now)
        })
        .await
}

/// Stores rows at `level` as they are, keeping any already stored at the same time. For tests
/// and benchmarks.
pub async fn insert(store: &Store, level: Level, rows: Vec<Point>) -> Result<(), Error> {
    store
        .write(move |sql| {
            rows.iter()
                .try_for_each(|p| insert_row(sql, level, p).map(drop))
        })
        .await
}

/// A row of `time, ` followed by [`stat_columns!`] or [`stat_sums!`].
fn point(r: &rusqlite::Row) -> Result<Point, SqlError> {
    let mut v = [0; 13];
    for (i, x) in v.iter_mut().enumerate() {
        *x = get(r, i + 2)?;
    }
    Ok(Point {
        time: get(r, 0)?,
        samples: get(r, 1)?,
        sums: Stats::from_values(v),
    })
}

/// The newest stored row: a snapshot, or after a long stop the newest hour or day.
pub async fn latest(store: &Store) -> Result<Option<Point>, Error> {
    store
        .read(move |sql| {
            let mut newest = sql.prepare(concat!(
                "SELECT time, ",
                stat_columns!(),
                " FROM ln_statistics WHERE level = ?1 ORDER BY time DESC LIMIT 1"
            ))?;
            for level in [Level::Minute, Level::Hour, Level::Day] {
                let mut rows = newest.query_map(params![level as i64], point)?;
                if let Some(p) = rows.next() {
                    return Ok(Some(p?));
                }
            }
            Ok(None)
        })
        .await
}

/// The series for `interval` as of `now`, in one read. Steps are never finer than a snapshot.
pub async fn series(store: &Store, interval: Interval, now: u64) -> Result<Series, Error> {
    store
        .read(move |sql| {
            let mut first = None;
            let mut oldest = sql.prepare("SELECT min(time) FROM ln_statistics WHERE level = ?1")?;
            for level in [Level::Day, Level::Hour, Level::Minute] {
                let t: Option<i64> = oldest.query_row(params![level as i64], |r| r.get(0))?;
                if let Some(t) = t {
                    first = u64::try_from(t).ok();
                    break;
                }
            }
            let (from, step, since) = match interval.span() {
                Some((span, step)) => {
                    let step = step.max(EVERY);
                    let from = now.saturating_sub(span).div_ceil(step) * step;
                    (from, step, from)
                }
                None => {
                    let first = first.unwrap_or(now);
                    let step = now.saturating_sub(first).div_ceil(1_000 * DAY).max(1) * DAY;
                    (first / step * step, step, first)
                }
            };
            let points = sql
                .prepare(concat!(
                    "SELECT time / ?2 * ?2 AS t, ",
                    stat_sums!(),
                    " FROM ln_statistics WHERE level IN (0, 1, 2) AND time BETWEEN ?1 AND ?3 ",
                    "GROUP BY t ORDER BY t"
                ))?
                .query_map(params![int(from), int(step), int(now)], point)?
                .collect::<Result<Vec<_>, _>>()?;
            // The snapshot in progress may not be taken yet; the one before must be.
            let last = (now / EVERY * EVERY).saturating_sub(EVERY);
            let start = since.div_ceil(EVERY) * EVERY;
            let slots = if last >= start {
                (last - start) / EVERY + 1
            } else {
                0
            };
            let sampled: i64 = sql
                .prepare(
                    "SELECT coalesce(sum(samples), 0) FROM ln_statistics \
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
                slots,
            })
        })
        .await
}
