//! Fee facts and the per-transaction records of one block, from `getblock` verbosity 2. Pure.

use bitcoin::BlockHash;
use node::rpc::model::{Block, BlockTx};
use serde::Serialize;

use super::Coinbase;
use crate::mining::pools;

/// Upper bounds of the design's fee bands 1–9 in hundredths of a sat/vB (`FEE_BOUNDS` in
/// `web/src/lib/format.ts`); band 10 is everything above.
const BAND_BOUNDS: [u64; 9] = [20, 30, 50, 100, 150, 200, 300, 500, 1000];

/// Version of the [`encode_txs`] format, its first four bytes.
pub const TXS_FORMAT: u32 = 1;
/// Bytes before the first record: format version and record count, both `u32` little-endian.
pub const TXS_HEADER: usize = 8;
/// Bytes per transaction: vsize `u32`, fee `u64`, output value `u64`, all little-endian.
pub const TXS_RECORD: usize = 20;

/// The facts of one block, as `/app/block/{hash}/summary` answers them. Everything except
/// `tx_count` leaves the coinbase out, as `getblockstats` does.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Facts {
    pub id: BlockHash,
    pub height: u32,
    /// Every transaction, the coinbase included, as Esplora's `tx_count`.
    pub tx_count: usize,
    /// Sum of the fees, in sats.
    pub fees: u64,
    /// Sum of the virtual sizes; equals the sum of `fee_bands`.
    pub vsize: u64,
    pub weight: u64,
    /// `None` for a block holding only its coinbase.
    pub fee_rate: Option<Rates>,
    /// Virtual size paid at each of the ten design bands, cheapest first, banded on the rate as
    /// the frontend prints it (two decimals), so a stratum's color agrees with its number.
    pub fee_bands: [u64; 10],
    /// Who mined it, by the pool list's rules.
    pub miner: Miner,
}

/// Who mined a block: a pool from the shipped list, or Unknown. Never text from the block, whose
/// coinbase tag is whatever its miner wrote; a tag only selects one of the list's names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Miner {
    pub name: &'static str,
    /// The pool page's slug, `unknown` for Unknown.
    pub slug: &'static str,
}

impl Miner {
    pub const UNKNOWN: Self = Self {
        name: "Unknown",
        slug: "unknown",
    };

    /// The pool [`pools::Pools::attribute`] gives this coinbase: its payout address first, then
    /// its tag.
    pub fn of(coinbase: &Coinbase) -> Self {
        let list = pools::list();
        let (id, _) = list.attribute(&coinbase.tag, &coinbase.payouts);
        list.by_id(id).map_or(Self::UNKNOWN, |p| Self {
            name: &p.name,
            slug: &p.slug,
        })
    }
}

/// Fee rates in sat/vB, unrounded. Percentiles are weighted by virtual size: `p25` is the rate
/// of the transaction that the cheapest quarter of the block's vbytes reaches.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Rates {
    pub min: f64,
    pub p10: f64,
    pub p25: f64,
    pub median: f64,
    pub p75: f64,
    pub p90: f64,
    pub max: f64,
}

/// One block's facts. The first transaction is taken as the coinbase, whatever its fee says.
pub fn facts(block: &Block) -> Facts {
    let paying = block.tx.get(1..).unwrap_or_default();
    let mut fee_bands = [0u64; 10];
    let (mut fees, mut vsize, mut weight) = (0u64, 0u64, 0u64);
    for tx in paying {
        let (fee, size) = fee_and_size(tx);
        fees = fees.saturating_add(fee);
        vsize += u64::from(tx.vsize);
        weight += u64::from(tx.weight);
        if let Some(b) = fee_bands.get_mut(band(fee, size)) {
            *b += u64::from(tx.vsize);
        }
    }
    Facts {
        id: block.hash,
        height: block.height,
        tx_count: block.tx.len(),
        fees,
        vsize,
        weight,
        fee_rate: rates(paying),
        fee_bands,
        miner: Miner::of(&Coinbase::from_parts(
            block.coinbase.script.as_bytes(),
            &block.coinbase.outputs,
        )),
    }
}

/// Fee in sats and a size that is never zero, so a malformed reply cannot divide by zero.
fn fee_and_size(tx: &BlockTx) -> (u64, u64) {
    (tx.fee.map_or(0, |f| f.to_sat()), u64::from(tx.vsize.max(1)))
}

/// The band index (0–9) of `fee / size`, rounded half up to hundredths first, as printed.
fn band(fee: u64, size: u64) -> usize {
    let hundredths = (u128::from(fee) * 200 + u128::from(size)) / (2 * u128::from(size));
    BAND_BOUNDS
        .iter()
        .take_while(|&&b| u128::from(b) <= hundredths)
        .count()
}

fn rates(paying: &[BlockTx]) -> Option<Rates> {
    let mut txs: Vec<(u64, u64)> = paying.iter().map(fee_and_size).collect();
    // Exact: compares fee_a / size_a with fee_b / size_b by cross-multiplying.
    txs.sort_unstable_by(|a, b| {
        (u128::from(a.0) * u128::from(b.1)).cmp(&(u128::from(b.0) * u128::from(a.1)))
    });
    let rate = |t: &(u64, u64)| t.0 as f64 / t.1 as f64;
    let total: u64 = txs.iter().map(|t| t.1).sum();
    // The first transaction at which the running vsize reaches `pct` percent of the total.
    let at = |pct: u64| {
        let mut seen = 0u64;
        txs.iter()
            .find(|t| {
                seen += t.1;
                u128::from(seen) * 100 >= u128::from(total) * u128::from(pct)
            })
            .map(rate)
    };
    Some(Rates {
        min: txs.first().map(rate)?,
        p10: at(10)?,
        p25: at(25)?,
        median: at(50)?,
        p75: at(75)?,
        p90: at(90)?,
        max: txs.last().map(rate)?,
    })
}

/// Every transaction in block order, coinbase first (its fee is 0), as fixed-width records
/// after an 8-byte header ([`TXS_FORMAT`], count). A square needs no txid up front: the public
/// API resolves a block hash and index to one (`/api/block/:hash/txid/:index`).
pub fn encode_txs(block: &Block) -> Vec<u8> {
    let mut out = Vec::with_capacity(TXS_HEADER + TXS_RECORD * block.tx.len());
    out.extend_from_slice(&TXS_FORMAT.to_le_bytes());
    let count = u32::try_from(block.tx.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&count.to_le_bytes());
    for tx in block.tx.iter().take(count as usize) {
        out.extend_from_slice(&tx.vsize.to_le_bytes());
        out.extend_from_slice(&tx.fee.map_or(0, |f| f.to_sat()).to_le_bytes());
        out.extend_from_slice(&tx.value.to_sat().to_le_bytes());
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use bitcoin::hashes::Hash;
    use bitcoin::{Amount, Txid};

    use super::*;

    fn tx(fee: Option<u64>, vsize: u32) -> BlockTx {
        BlockTx {
            txid: Txid::all_zeros(),
            fee: fee.map(Amount::from_sat),
            vsize,
            weight: vsize * 4,
            size: vsize,
            value: Amount::from_sat(u64::from(vsize) * 1000),
        }
    }

    fn block(paying: &[(u64, u32)]) -> Block {
        let mut tx_list = vec![tx(None, 150)];
        tx_list.extend(paying.iter().map(|&(f, v)| tx(Some(f), v)));
        Block {
            hash: BlockHash::all_zeros(),
            confirmations: 1,
            height: 7,
            time: 0,
            mediantime: 0,
            size: 0,
            strippedsize: 0,
            weight: 0,
            previousblockhash: None,
            tx: tx_list,
            coinbase: Default::default(),
        }
    }

    #[test]
    fn weighted_percentiles_keep_sub_sat_rates() {
        // 0.11, 0.25, 0.5 and 2 sat/vB; the 0.25 transaction holds most of the vbytes.
        let b = block(&[(11, 100), (2500, 10_000), (50, 100), (400, 200)]);
        let f = facts(&b);
        let r = f.fee_rate.unwrap();
        assert_eq!((r.min, r.max), (0.11, 2.0));
        // Cumulative vsize: 100 (1%), 10 100 (97%), 10 200 (98%), 10 400 (100%).
        assert_eq!(
            (r.p10, r.p25, r.median, r.p75, r.p90),
            (0.25, 0.25, 0.25, 0.25, 0.25)
        );
        let b = block(&[(10, 100), (20, 100), (30, 100), (40, 100)]);
        let r = facts(&b).fee_rate.unwrap();
        assert_eq!(
            (r.p10, r.p25, r.median, r.p75, r.p90),
            (0.1, 0.1, 0.2, 0.3, 0.4)
        );
        assert_eq!(
            (f.tx_count, f.fees, f.vsize, f.weight),
            (5, 2961, 10_400, 41_600)
        );
    }

    fn mined(tag: &[u8], pays: &str) -> Miner {
        use std::str::FromStr;
        let script = bitcoin::Address::from_str(pays)
            .unwrap()
            .assume_checked()
            .script_pubkey();
        let mut b = block(&[]);
        b.coinbase = node::rpc::model::Coinbase {
            script: bitcoin::ScriptBuf::from_bytes(tag.to_vec()),
            outputs: vec![
                bitcoin::TxOut {
                    value: Amount::ZERO,
                    script_pubkey: bitcoin::ScriptBuf::new_op_return([0xaa; 4]),
                },
                bitcoin::TxOut {
                    value: Amount::from_sat(312_500_000),
                    script_pubkey: script,
                },
            ],
        };
        facts(&b).miner
    }

    #[test]
    fn the_miner_is_a_listed_name_payout_first() {
        const ELSEWHERE: &str = "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq";
        // Tagged ViaBTC but paying Foundry USA's listed address: Foundry USA's.
        let m = mined(
            b"\x03\x97\xe2\x0e/ViaBTC/",
            "12KKDt4Mj7N5UAkQMN7LtPZMayenXHa8KL",
        );
        assert_eq!((m.name, m.slug), ("Foundry USA", "foundryusa"));
        // A tag alone names its pool, by the list's spelling.
        let m = mined(b"\x03\x97\xe2\x0e/viabtc/", ELSEWHERE);
        assert_eq!((m.name, m.slug), ("ViaBTC", "viabtc"));
        // Anything else is Unknown, and the tag's text never becomes a name.
        for tag in [
            &b"<img src=x onerror=alert(1)>"[..],
            b"\x00\x1b[31m\xe2\x80\xaeMy Pool",
            b"",
        ] {
            assert_eq!(mined(tag, ELSEWHERE), Miner::UNKNOWN);
        }
        assert_eq!(facts(&block(&[])).miner, Miner::UNKNOWN, "no coinbase read");
        let json = serde_json::to_value(facts(&block(&[]))).unwrap();
        assert_eq!(
            json["miner"],
            serde_json::json!({"name": "Unknown", "slug": "unknown"})
        );
    }

    #[test]
    fn a_coinbase_only_block_has_no_rates() {
        let f = facts(&block(&[]));
        assert_eq!(f.tx_count, 1);
        assert_eq!((f.fees, f.vsize, f.fee_rate), (0, 0, None));
        assert_eq!(f.fee_bands, [0; 10]);
        let empty = Block {
            tx: Vec::new(),
            ..block(&[])
        };
        assert_eq!(facts(&empty).tx_count, 0);
        assert_eq!(encode_txs(&empty), [1, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn identical_rates_give_one_value_everywhere() {
        let f = facts(&block(&[(70, 140), (35, 70), (700, 1400)]));
        let r = f.fee_rate.unwrap();
        for v in [r.min, r.p10, r.p25, r.median, r.p75, r.p90, r.max] {
            assert_eq!(v, 0.5);
        }
        assert_eq!(f.fee_bands[3], 1610);
    }

    #[test]
    fn bands_follow_the_printed_rate_and_sum_to_the_whole() {
        // 0.1995 prints as 0.20 (band 2); 0.1949 as 0.19 (band 1); 9.996 as 10.0 (band 10).
        assert_eq!(band(1995, 10_000), 1);
        assert_eq!(band(1949, 10_000), 0);
        assert_eq!(band(9996, 1000), 9);
        assert_eq!(band(0, 1), 0);
        assert_eq!(band(u64::MAX, 1), 9);
        let edges = [
            (1, 10),
            (2, 10),
            (3, 10),
            (5, 10),
            (10, 10),
            (15, 10),
            (20, 10),
            (30, 10),
            (50, 10),
            (100, 10),
        ];
        for (i, (fee, size)) in edges.into_iter().enumerate() {
            assert_eq!(band(fee, size), i, "{fee}/{size}");
        }
        let b = block(&[
            (11, 100),
            (2500, 10_000),
            (50, 100),
            (400, 200),
            (5000, 333),
        ]);
        let f = facts(&b);
        assert_eq!(f.fee_bands.iter().sum::<u64>(), f.vsize);
        assert_eq!(f.fee_bands, [100, 10_000, 0, 100, 0, 0, 200, 0, 0, 333]);
    }

    #[test]
    fn zero_sizes_and_huge_fees_do_not_panic() {
        let f = facts(&block(&[(5, 0), (u64::MAX / 2, 1), (u64::MAX / 2, 1)]));
        assert_eq!(f.fees, u64::MAX, "saturates");
        assert!(f.fee_rate.unwrap().max > 1e18);
    }

    #[test]
    fn records_are_fixed_width_in_block_order() {
        let b = block(&[(11, 100), (u64::from(u32::MAX) + 1, 200)]);
        let out = encode_txs(&b);
        assert_eq!(out.len(), TXS_HEADER + 3 * TXS_RECORD);
        assert_eq!(out[..8], [1, 0, 0, 0, 3, 0, 0, 0]);
        let rec = |i: usize| &out[TXS_HEADER + i * TXS_RECORD..][..TXS_RECORD];
        let u32_at = |r: &[u8], o: usize| u32::from_le_bytes(r[o..o + 4].try_into().unwrap());
        let u64_at = |r: &[u8], o: usize| u64::from_le_bytes(r[o..o + 8].try_into().unwrap());
        // The coinbase: no fee.
        assert_eq!(
            (u32_at(rec(0), 0), u64_at(rec(0), 4), u64_at(rec(0), 12)),
            (150, 0, 150_000)
        );
        assert_eq!((u32_at(rec(1), 0), u64_at(rec(1), 4)), (100, 11));
        assert_eq!(u64_at(rec(2), 4), 1 << 32);
        assert_eq!(u64_at(rec(2), 12), 200_000);
    }
}
