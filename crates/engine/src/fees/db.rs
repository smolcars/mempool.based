//! SQL for the fee score's tables, `fee_samples` and `fee_blocks` (`docs/STORE.md`). Every
//! statement is a literal; values are bound (SECURITY.md rule 12).

use super::score::{Block, Sample, Served};
use crate::store::{Error, Read, SqlError, Store, params};

/// Samples are kept this long.
pub const KEEP_SAMPLES: u64 = 30 * 86_400;
/// Blocks a day longer: the oldest sample's day-long horizon is still scored.
pub const KEEP_BLOCKS: u64 = KEEP_SAMPLES + 86_400;

/// What one write stores.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Batch {
    pub samples: Vec<Sample>,
    pub blocks: Vec<Block>,
}

/// SQLite integers are signed; nothing stored here comes near 2^63.
fn int(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

/// Stores `batch` and deletes what has aged as of `now`, in one write at the store's
/// `generation` ([`Store::write_from`]): data read before the node was found on another chain is
/// not written. A sample at a time already stored is kept as it was.
pub async fn write(store: &Store, generation: u64, batch: Batch, now: u64) -> Result<(), Error> {
    store
        .write_from(generation, move |sql| {
            let mut sample = sql.prepare(
                "INSERT OR IGNORE INTO fee_samples (time, served, high, medium, low, no_rush) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for s in &batch.samples {
                let (served, r) = match s.served {
                    Served::Ours(r) => (0, r.map(Some)),
                    Served::Node => (1, [None; 4]),
                    Served::Nothing => (2, [None; 4]),
                };
                sample.execute(params![int(s.t), served, r[0], r[1], r[2], r[3]])?;
            }
            let mut block = sql.prepare(
                "INSERT OR REPLACE INTO fee_blocks (height, time, clearing) VALUES (?1, ?2, ?3)",
            )?;
            for b in &batch.blocks {
                block.execute(params![b.height, int(b.t), b.clearing])?;
            }
            sql.prepare("DELETE FROM fee_samples WHERE time < ?1")?
                .execute(params![int(now.saturating_sub(KEEP_SAMPLES))])?;
            if !batch.blocks.is_empty() {
                sql.prepare("DELETE FROM fee_blocks WHERE time < ?1")?
                    .execute(params![int(now.saturating_sub(KEEP_BLOCKS))])?;
            }
            Ok(())
        })
        .await
}

fn get(r: &rusqlite::Row, i: usize) -> Result<u64, SqlError> {
    let v: i64 = r.get(i)?;
    u64::try_from(v).map_err(|_| SqlError::IntegralValueOutOfRange(i, v))
}

/// Every sample and block kept as of `now`, in one read.
pub async fn load(store: &Store, now: u64) -> Result<(Vec<Sample>, Vec<Block>), Error> {
    store
        .read(Read::FeeScore, move |sql| {
            let samples = sql
                .prepare(
                    "SELECT time, served, high, medium, low, no_rush FROM fee_samples \
                     WHERE time >= ?1 ORDER BY time",
                )?
                .query_map(params![int(now.saturating_sub(KEEP_SAMPLES))], |r| {
                    let rates: [Option<u32>; 4] = [r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?];
                    let served = match (r.get::<_, i64>(1)?, rates) {
                        (0, [Some(a), Some(b), Some(c), Some(d)]) => Served::Ours([a, b, c, d]),
                        (1, _) => Served::Node,
                        _ => Served::Nothing,
                    };
                    Ok(Sample {
                        t: get(r, 0)?,
                        served,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let blocks = sql
                .prepare(
                    "SELECT height, time, clearing FROM fee_blocks WHERE time >= ?1 ORDER BY time",
                )?
                .query_map(params![int(now.saturating_sub(KEEP_BLOCKS))], |r| {
                    Ok(Block {
                        height: r.get(0)?,
                        t: get(r, 1)?,
                        clearing: r.get(2)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok((samples, blocks))
        })
        .await
}
