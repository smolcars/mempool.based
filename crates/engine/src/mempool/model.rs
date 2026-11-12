//! The in-memory mempool and the deltas that change it. Pure: no I/O, no clock.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, RandomState};

use bitcoin::{Amount, BlockHash, OutPoint, Transaction, Txid};
use node::esplora::ChainTip;
use node::rpc::model::MempoolEntry;

/// What we keep for every mempool transaction: 48 bytes plus 8 per input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxInfo {
    /// Base fee, without `prioritisetransaction` deltas.
    pub fee: Amount,
    /// Total output value, for the live feed.
    pub value: Amount,
    /// Sigop-adjusted virtual size, the size mining uses.
    pub vsize: u32,
    pub weight: u32,
    /// Unix time the node accepted it, from `getmempoolentry`. Survives our own restarts.
    pub time: u32,
    /// Output count, to find children of a removed transaction.
    pub(crate) outputs: u32,
    /// Keyed 64-bit fingerprints of the outpoints it spends, to find what replaced it.
    pub(crate) spends: Box<[u64]>,
}

impl TxInfo {
    /// A record with no inputs, for building deltas in other crates' tests.
    pub fn new(fee: Amount, value: Amount, vsize: u32, weight: u32, time: u32) -> Self {
        Self {
            fee,
            value,
            vsize,
            weight,
            time,
            outputs: 0,
            spends: Box::new([]),
        }
    }
}

/// Why a transaction left the mempool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Removal {
    /// It is in a block on the active chain.
    Confirmed { height: u32, block: BlockHash },
    /// `by` spends one of its inputs: a transaction that entered the mempool in the same cycle,
    /// or one confirmed in a recent block. A child of a replaced transaction carries its
    /// parent's replacement.
    Replaced { by: Txid },
    /// Neither: evicted for size, expired, or dropped for another reason the node does not
    /// report. Also the fallback where a replacement could not be seen (see `docs/MEMPOOL.md`).
    Evicted,
}

/// One change to the mempool. Deltas are numbered without gaps from 1; applying them in order
/// to the snapshot they follow reproduces the tracker's state.
#[derive(Debug, Clone, PartialEq)]
pub struct Delta {
    pub seq: u64,
    /// The node's `mempool_sequence` for the listing this delta came from. Informational: it
    /// resets when the node restarts, and the diff never relies on it.
    pub mempool_sequence: u64,
    /// The chain tip read just after that listing.
    pub tip: ChainTip,
    /// Sorted by txid.
    pub added: Vec<(Txid, TxInfo)>,
    /// Sorted by txid.
    pub removed: Vec<(Txid, Removal)>,
}

/// The tracked mempool. Read it through [`super::Tracker`].
#[derive(Debug)]
pub struct Mempool {
    txs: HashMap<Txid, TxInfo>,
    seq: u64,
    mempool_sequence: u64,
    tip: Option<ChainTip>,
    vsize: u64,
    fees: Amount,
    keys: Keys,
}

/// The secret key for outpoint fingerprints, so nobody can grind a collision.
#[derive(Debug, Clone, Default)]
pub(crate) struct Keys(RandomState);

impl Default for Mempool {
    fn default() -> Self {
        Self {
            txs: HashMap::new(),
            seq: 0,
            mempool_sequence: 0,
            tip: None,
            vsize: 0,
            fees: Amount::ZERO,
            keys: Keys::default(),
        }
    }
}

impl Keys {
    pub(crate) fn key(&self, o: &OutPoint) -> u64 {
        self.0.hash_one(o)
    }

    /// The record for a transaction from its mempool entry and its bytes.
    pub(crate) fn record(&self, entry: &MempoolEntry, tx: &Transaction) -> TxInfo {
        let sat = |v: u64| u32::try_from(v).unwrap_or(u32::MAX);
        let value = tx.output.iter().fold(Amount::ZERO, |sum, o| {
            sum.checked_add(o.value).unwrap_or(Amount::MAX)
        });
        TxInfo {
            fee: entry.fees.base,
            value,
            vsize: sat(entry.vsize),
            weight: sat(entry.weight),
            time: sat(entry.time),
            outputs: sat(tx.output.len() as u64),
            spends: tx
                .input
                .iter()
                .map(|i| self.key(&i.previous_output))
                .collect(),
        }
    }

    pub(crate) fn block(&self, height: u32, block: &bitcoin::Block) -> BlockSpends {
        let mut spends = HashMap::new();
        let mut txids = HashSet::with_capacity(block.txdata.len());
        for tx in &block.txdata {
            let txid = tx.compute_txid();
            txids.insert(txid);
            if !tx.is_coinbase() {
                for i in &tx.input {
                    spends.insert(self.key(&i.previous_output), txid);
                }
            }
        }
        BlockSpends {
            hash: block.block_hash(),
            prev: block.header.prev_blockhash,
            height,
            txids,
            spends,
        }
    }
}

/// What a listing adds and removes, each sorted by txid.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Diff {
    pub added: Vec<Txid>,
    pub removed: Vec<Txid>,
}

/// One block's transactions and the outpoints they spend.
#[derive(Debug)]
pub(crate) struct BlockSpends {
    pub hash: BlockHash,
    pub prev: BlockHash,
    pub height: u32,
    txids: HashSet<Txid>,
    spends: HashMap<u64, Txid>,
}

impl Mempool {
    pub fn get(&self, txid: &Txid) -> Option<&TxInfo> {
        self.txs.get(txid)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Txid, &TxInfo)> {
        self.txs.iter()
    }

    pub fn len(&self) -> usize {
        self.txs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.txs.is_empty()
    }

    /// The last delta applied; 0 before the first.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    pub fn mempool_sequence(&self) -> u64 {
        self.mempool_sequence
    }

    pub fn tip(&self) -> Option<ChainTip> {
        self.tip
    }

    /// Summed sigop-adjusted vsize.
    pub fn vsize(&self) -> u64 {
        self.vsize
    }

    pub fn fees(&self) -> Amount {
        self.fees
    }

    pub(crate) fn keys(&self) -> &Keys {
        &self.keys
    }

    pub(crate) fn diff(&self, listing: &[Txid]) -> Diff {
        let now: HashSet<&Txid> = listing.iter().collect();
        let mut added: Vec<Txid> = now
            .iter()
            .filter(|t| !self.txs.contains_key(**t))
            .map(|t| **t)
            .collect();
        let mut removed: Vec<Txid> = self
            .txs
            .keys()
            .filter(|t| !now.contains(t))
            .copied()
            .collect();
        added.sort_unstable();
        removed.sort_unstable();
        Diff { added, removed }
    }

    /// Classifies every removal. `blocks` are recent blocks on the active chain; `added` are
    /// the transactions that entered in the same cycle.
    pub(crate) fn classify(
        &self,
        removed: &[Txid],
        added: &[(Txid, TxInfo)],
        blocks: &[BlockSpends],
    ) -> Vec<(Txid, Removal)> {
        let new_spends: HashMap<u64, Txid> = added
            .iter()
            .flat_map(|(txid, info)| info.spends.iter().map(move |k| (*k, *txid)))
            .collect();
        let mut out: Vec<(Txid, Option<Removal>)> = removed
            .iter()
            .map(|txid| {
                let spends = self.txs.get(txid).map_or(&[][..], |i| &i.spends[..]);
                let reason = blocks
                    .iter()
                    .find(|b| b.txids.contains(txid))
                    .map(|b| Removal::Confirmed {
                        height: b.height,
                        block: b.hash,
                    })
                    .or_else(|| {
                        spends.iter().find_map(|k| {
                            let by = blocks
                                .iter()
                                .find_map(|b| b.spends.get(k))
                                .or_else(|| new_spends.get(k))?;
                            Some(Removal::Replaced { by: *by })
                        })
                    });
                (*txid, reason)
            })
            .collect();

        // A child leaves with its replaced parent. Each pass settles one more generation, and
        // a pass that settles nothing ends it.
        loop {
            let replaced: HashMap<u64, Txid> = out
                .iter()
                .filter_map(|(txid, r)| match r {
                    Some(Removal::Replaced { by }) => Some((txid, *by)),
                    _ => None,
                })
                .flat_map(|(txid, by)| {
                    let outputs = self.txs.get(txid).map_or(0, |i| i.outputs);
                    (0..outputs).map(move |vout| (self.keys.key(&OutPoint::new(*txid, vout)), by))
                })
                .collect();
            let mut settled = false;
            for (txid, reason) in out.iter_mut().filter(|(_, r)| r.is_none()) {
                let spends = self.txs.get(txid).map_or(&[][..], |i| &i.spends[..]);
                if let Some(by) = spends.iter().find_map(|k| replaced.get(k)) {
                    *reason = Some(Removal::Replaced { by: *by });
                    settled = true;
                }
            }
            if !settled {
                break;
            }
        }
        out.into_iter()
            .map(|(t, r)| (t, r.unwrap_or(Removal::Evicted)))
            .collect()
    }

    /// Applies the next delta. A delta out of order is refused and changes nothing.
    pub fn apply(&mut self, d: &Delta) -> bool {
        if d.seq != self.seq + 1 {
            return false;
        }
        for (txid, _) in &d.removed {
            if let Some(info) = self.txs.remove(txid) {
                self.vsize = self.vsize.saturating_sub(u64::from(info.vsize));
                self.fees = self.fees.checked_sub(info.fee).unwrap_or(Amount::ZERO);
            }
        }
        for (txid, info) in &d.added {
            if let Some(old) = self.txs.insert(*txid, info.clone()) {
                self.vsize = self.vsize.saturating_sub(u64::from(old.vsize));
                self.fees = self.fees.checked_sub(old.fee).unwrap_or(Amount::ZERO);
            }
            self.vsize += u64::from(info.vsize);
            self.fees = self.fees.checked_add(info.fee).unwrap_or(Amount::MAX);
        }
        self.seq = d.seq;
        self.mempool_sequence = d.mempool_sequence;
        self.tip = Some(d.tip);
        true
    }
}
