//! Block audits: each block we watched being mined, compared with the block we expected on its
//! parent. The method and its categories are in `docs/MEMPOOL.md`, "Block audits"; the table in
//! `docs/STORE.md`, "audits".
//!
//! [`compare`] is pure. The server feeds it the projector's [`crate::projected::Expected`], the
//! mined block from the `getblock` the block summary reads anyway, and the tracker's accept
//! times, and stores the result with [`write`].

mod db;

pub use db::{
    PoolHealth, Scored, by_hash, by_hashes, pool_health, scored_between, scored_since, write,
};

use std::collections::HashSet;

use bitcoin::{BlockHash, Txid};
use serde::Serialize;

use crate::projected::ProjectedBlock;

/// A transaction the node accepted less than this many seconds before we learnt of the block is
/// too recent to count against the miner: pools hand out new work about every 30 s, and the
/// transaction must also have reached the pool's node.
pub const RECENT_SECS: u32 = 60;
/// A block's room for transactions in weight units: 4,000,000 less the 8,000 a template keeps
/// for the coinbase, as the projection packs.
pub const BLOCK_WEIGHT: u64 = 3_992_000;
/// Most per-transaction records kept for one block, the most telling kinds first.
pub const MAX_TXS: usize = 1000;

/// What happened to one transaction that was expected or mined but not both. Matched
/// transactions (expected and mined) are counted, not listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Expected, not mined, and no innocent cause below explains it. Only these lower the score.
    Missing = 1,
    /// Expected, not mined, but the block used the room for transactions paying at least its
    /// rate that we placed later or never saw: ties and packing differences at the boundary.
    Displaced = 2,
    /// Expected, not mined, and accepted by our node less than [`RECENT_SECS`] before the block.
    Fresh = 3,
    /// Expected, not mined, and gone from our mempool once the block was applied: the block spent
    /// one of its inputs, or it was replaced or evicted in that moment.
    Conflicted = 4,
    /// Mined, not expected, but in our projection for a later block.
    Added = 5,
    /// Mined, and not in our projection at all: we had not seen it.
    Unseen = 6,
}

impl Kind {
    /// The order records are kept in when there are more than [`MAX_TXS`].
    const PRIORITY: [Kind; 6] = [
        Kind::Missing,
        Kind::Unseen,
        Kind::Added,
        Kind::Displaced,
        Kind::Conflicted,
        Kind::Fresh,
    ];

    fn from_u8(b: u8) -> Option<Self> {
        Self::PRIORITY.into_iter().find(|k| *k as u8 == b)
    }
}

/// One transaction of the mined block, the coinbase left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mined {
    pub txid: Txid,
    pub fee: u64,
    /// `ceil(weight / 4)`, as `getblock` gives it.
    pub vsize: u32,
    pub weight: u32,
}

/// Transactions and their virtual size.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Tally {
    pub txs: u32,
    pub vsize: u64,
}

impl Tally {
    fn add(&mut self, vsize: u32) {
        self.txs += 1;
        self.vsize += u64::from(vsize);
    }
}

/// One side's totals: the expected block's, or the mined block's without its coinbase.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Side {
    pub txs: u32,
    pub vsize: u64,
    pub weight: u64,
    pub fees: u64,
}

/// An audit's numbers. `match_rate` is in percent, rounded down to two decimals, so 100 means no
/// expected transaction was left out without an innocent cause.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Totals {
    pub match_rate: f64,
    pub expected: Side,
    pub actual: Side,
    pub matched: Tally,
    pub missing: Tally,
    pub displaced: Tally,
    pub fresh: Tally,
    pub conflicted: Tally,
    pub added: Tally,
    pub unseen: Tally,
}

impl Totals {
    /// Transactions in the lists, kept or not.
    pub fn listed(&self) -> u32 {
        [
            self.missing,
            self.displaced,
            self.fresh,
            self.conflicted,
            self.added,
            self.unseen,
        ]
        .iter()
        .map(|t| t.txs)
        .sum()
    }
}

/// One listed transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AuditTx {
    pub txid: Txid,
    pub kind: Kind,
    pub vsize: u32,
    /// Sat per 1,000 vB: the chunk rate we placed it at for the expected kinds, its own fee over
    /// its size for the mined kinds.
    pub rate: u32,
    /// When our node accepted it (Unix seconds), for the expected kinds still in our mempool.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_seen: Option<u32>,
}

/// One block's audit as stored.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Audit {
    pub id: BlockHash,
    pub height: u32,
    /// The block header's timestamp.
    pub time: u32,
    /// When we learnt of the block, Unix seconds: the reference for [`Kind::Fresh`].
    pub seen_at: u32,
    #[serde(flatten)]
    pub totals: Totals,
    /// At most [`MAX_TXS`], by [`Kind::PRIORITY`], then highest rate first. Empty when only
    /// the totals were read.
    pub txs: Vec<AuditTx>,
}

/// Compares `expected` (our first block on the parent) with `mined`.
///
/// - `left`: sorted, what left our projection when it moved onto this block, with the rate we
///   had placed it at ([`crate::projected::Expected::left`]).
/// - `room`: the weight a block holds for transactions ([`BLOCK_WEIGHT`] on a real chain).
/// - `seen_at`: when we learnt of the block, Unix seconds ([`crate::projected::Expected::at`]).
/// - `mempool`: the node's accept time and the raw weight of a transaction still in our mempool
///   after the block.
pub fn compare(
    expected: &ProjectedBlock,
    left: &[(Txid, u32)],
    mined: &[Mined],
    room: u64,
    seen_at: u32,
    mempool: impl Fn(&Txid) -> Option<(u32, u32)>,
) -> (Totals, Vec<AuditTx>) {
    let in_block: HashSet<&Txid> = mined.iter().map(|m| &m.txid).collect();
    let was_expected: HashSet<&Txid> = expected.txs.iter().map(|(t, _)| t).collect();
    let placed = |t: &Txid| {
        left.binary_search_by_key(t, |(t, _)| *t)
            .ok()
            .and_then(|i| left.get(i))
    };
    let mut t = Totals {
        match_rate: 100.0,
        expected: Side {
            txs: expected.summary.txs,
            vsize: expected.summary.vsize,
            weight: expected.summary.weight,
            fees: expected.summary.fees,
        },
        actual: Side::default(),
        matched: Tally::default(),
        missing: Tally::default(),
        displaced: Tally::default(),
        fresh: Tally::default(),
        conflicted: Tally::default(),
        added: Tally::default(),
        unseen: Tally::default(),
    };
    let mut txs = Vec::new();
    // Expected and not mined, with no reason yet: missing unless displaced.
    let mut left_out = Vec::new();
    let mut expected_vsize = 0u64;
    for (txid, p) in &expected.txs {
        expected_vsize += u64::from(p.vsize);
        if in_block.contains(txid) {
            t.matched.add(p.vsize);
            continue;
        }
        let mut tx = AuditTx {
            txid: *txid,
            kind: Kind::Missing,
            vsize: p.vsize,
            rate: p.rate,
            first_seen: None,
        };
        if placed(txid).is_some() {
            tx.kind = Kind::Conflicted;
            t.conflicted.add(p.vsize);
            txs.push(tx);
            continue;
        }
        let held = mempool(txid);
        tx.first_seen = held.map(|h| h.0);
        // Physical size, which the block limit counts; sigop-adjusted size can be far larger.
        // Unknown once the tracker no longer holds it: then it neither adds to the overflow
        // nor can be displaced.
        let weight = held.map(|h| u64::from(h.1));
        // A time after the block is a record newer than the one we expected (evicted and sent
        // again since): it says nothing about when the miner could have seen it.
        if tx
            .first_seen
            .is_some_and(|at| at <= seen_at && at.saturating_add(RECENT_SECS) > seen_at)
        {
            tx.kind = Kind::Fresh;
            t.fresh.add(p.vsize);
            txs.push(tx);
            continue;
        }
        left_out.push((tx, weight));
    }
    // Mined and not expected: the space a left-out transaction may have gone to.
    let mut taken = Vec::new();
    for m in mined {
        t.actual.txs += 1;
        t.actual.vsize += u64::from(m.vsize);
        t.actual.weight += u64::from(m.weight);
        t.actual.fees = t.actual.fees.saturating_add(m.fee);
        if was_expected.contains(&m.txid) {
            continue;
        }
        // Ours is the rate of its chunk, which a parent mined with its child shares; an unseen
        // one has only its own.
        let (kind, rate) = match placed(&m.txid) {
            Some(&(_, rate)) => {
                t.added.add(m.vsize);
                (Kind::Added, rate)
            }
            None => {
                t.unseen.add(m.vsize);
                let own = u128::from(m.fee) * 1000 / u128::from(m.vsize.max(1));
                (Kind::Unseen, u32::try_from(own).unwrap_or(u32::MAX))
            }
        };
        taken.push((rate, u64::from(m.weight)));
        txs.push(AuditTx {
            txid: m.txid,
            kind,
            vsize: m.vsize,
            rate,
            first_seen: None,
        });
    }
    // Only what could not have fitted is excused: had the miner added every left-out one, the
    // block would have overflowed by `forced`. Within that, highest rates first on both sides, a
    // left-out transaction is displaced when it fits whole in the space the block gave to
    // transactions we did not expect paying at least its rate, less what better-paying left-out
    // ones already took.
    // All of it in weight, the block limit's own unit.
    let forced =
        (t.actual.weight + left_out.iter().filter_map(|x| x.1).sum::<u64>()).saturating_sub(room);
    left_out.sort_by_key(|x| std::cmp::Reverse(x.0.rate));
    taken.sort_by_key(|r| std::cmp::Reverse(r.0));
    let (mut j, mut avail, mut used) = (0, 0u64, 0u64);
    for (mut tx, weight) in left_out {
        while let Some(&(rate, w)) = taken.get(j)
            && rate >= tx.rate
        {
            avail += w;
            j += 1;
        }
        if let Some(weight) = weight
            && used + weight <= avail.min(forced)
        {
            used += weight;
            tx.kind = Kind::Displaced;
            t.displaced.add(tx.vsize);
        } else {
            t.missing.add(tx.vsize);
        }
        txs.push(tx);
    }
    let eligible = expected_vsize.saturating_sub(t.fresh.vsize + t.conflicted.vsize);
    if eligible > 0 {
        let agreed = t.matched.vsize + t.displaced.vsize;
        // Rounded down, so 100 is only ever a block with nothing missing.
        t.match_rate = (agreed as f64 * 10_000.0 / eligible as f64).floor() / 100.0;
    }
    txs.sort_by_key(|x| {
        let rank = Kind::PRIORITY.iter().position(|k| *k == x.kind);
        (rank, std::cmp::Reverse(x.rate), x.txid)
    });
    txs.truncate(MAX_TXS);
    (t, txs)
}

/// Bytes per stored record: txid, kind, vsize, rate, first seen (0 for none).
const RECORD: usize = 32 + 1 + 4 + 4 + 4;

fn encode(txs: &[AuditTx]) -> Vec<u8> {
    use bitcoin::hashes::Hash;
    let mut out = Vec::with_capacity(txs.len() * RECORD);
    for tx in txs {
        out.extend_from_slice(tx.txid.as_byte_array());
        out.push(tx.kind as u8);
        out.extend_from_slice(&tx.vsize.to_le_bytes());
        out.extend_from_slice(&tx.rate.to_le_bytes());
        out.extend_from_slice(&tx.first_seen.unwrap_or(0).to_le_bytes());
    }
    out
}

/// `None` for bytes [`encode`] did not write.
fn decode(bytes: &[u8]) -> Option<Vec<AuditTx>> {
    use bitcoin::hashes::Hash;
    let (records, rest) = bytes.as_chunks::<RECORD>();
    if !rest.is_empty() {
        return None;
    }
    let u32_at = |r: &[u8], at: usize| -> Option<u32> {
        Some(u32::from_le_bytes(r.get(at..at + 4)?.try_into().ok()?))
    };
    records
        .iter()
        .map(|r| {
            let first_seen = u32_at(r, 41)?;
            Some(AuditTx {
                txid: Txid::from_byte_array(r.get(..32)?.try_into().ok()?),
                kind: Kind::from_u8(*r.get(32)?)?,
                vsize: u32_at(r, 33)?,
                rate: u32_at(r, 37)?,
                first_seen: (first_seen != 0).then_some(first_seen),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
