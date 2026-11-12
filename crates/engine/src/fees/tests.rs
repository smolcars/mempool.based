#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;

use bitcoin::BlockHash;
use bitcoin::hashes::Hash;

use super::*;
use crate::projected::Projection;

const B: u64 = BLOCK_VSIZE;

/// A mempool of bands: `(rate, vsize)`, as many 250 vB transactions as fill each.
fn depth(bands: &[(u32, u64)]) -> Depth {
    Depth::new(bands.iter().flat_map(|&(rate, vsize)| {
        std::iter::repeat_n((rate, 250), usize::try_from(vsize / 250).unwrap())
    }))
}

/// Arrivals: one list of `(rate, vsize)` per whole minute, oldest first.
fn inflow(minutes: &[Vec<(u32, u32)>]) -> Inflow {
    let mut i = Inflow::default();
    i.roll(0);
    for (m, arrivals) in minutes.iter().enumerate() {
        let t = m as u64 * 60;
        for &(rate, vsize) in arrivals {
            i.record(t, rate, vsize);
        }
    }
    i.roll(minutes.len() as u64 * 60);
    i
}

fn levels(r: Rates) -> [u32; 4] {
    r.levels
}

#[test]
fn a_mempool_many_blocks_deep_asks_one_step_above_each_boundary() {
    // Twenty bands just under a block each, 20, 19.5, 19 … sat/vB.
    let bands: Vec<(u32, u64)> = (0..20).map(|k| (20_000 - k * 500, 990_000)).collect();
    let r = recommend(&depth(&bands), &Inflow::default(), 100);
    // Block N ends inside band N: pay one step above that band.
    assert_eq!(levels(r), [19_501, 18_501, 17_001, 100]);
    assert_eq!(r.minimum, 100);
}

#[test]
fn exactly_one_block_must_outbid_its_ties_and_the_rest_is_the_minimum() {
    let r = recommend(&depth(&[(5_000, B)]), &Inflow::default(), 100);
    assert_eq!(levels(r), [5_001, 100, 100, 100]);
}

#[test]
fn less_than_a_block_or_nothing_at_all_is_the_minimum() {
    for d in [depth(&[(3_000, B / 2), (150, B / 4)]), Depth::default()] {
        let r = recommend(&d, &Inflow::default(), 100);
        assert_eq!(levels(r), [100; 4]);
    }
}

#[test]
fn the_floor_is_the_nodes_minimum_and_never_under_the_relay_floor() {
    let r = recommend(&Depth::default(), &Inflow::default(), 1_500);
    assert_eq!((levels(r), r.minimum), ([1_500; 4], 1_500));
    let r = recommend(&Depth::default(), &Inflow::default(), 0);
    assert_eq!((levels(r), r.minimum), ([RELAY_FLOOR; 4], RELAY_FLOOR));
}

#[test]
fn a_boundary_full_of_equal_rates_is_outbid_by_one_step() {
    // Half a block at 2 sat/vB, then three blocks of 0.124 sat/vB ties.
    let r = recommend(
        &depth(&[(2_000, B / 2), (124, 3 * B)]),
        &Inflow::default(),
        100,
    );
    assert_eq!(levels(r), [125, 125, 100, 100]);
}

#[test]
fn one_absurd_transaction_moves_nothing() {
    let base = depth(&[(1_000, 3 * B)]);
    let calm = recommend(&base, &Inflow::default(), 100);
    // A 100 kvB transaction at 10,000 sat/vB in the mempool…
    let mut pairs: Vec<(u32, u64)> = (0..(3 * B / 250)).map(|_| (1_000, 250)).collect();
    pairs.push((10_000_000, 100_000));
    let r = recommend(&Depth::new(pairs), &Inflow::default(), 100);
    assert_eq!(levels(r), levels(calm));
    assert_eq!(levels(r), [1_001, 1_001, 100, 100]);
    // …or arriving once among quiet minutes.
    let mut minutes = vec![vec![(1_000, 250)]; 10];
    minutes[4].push((10_000_000, 100_000));
    let r = recommend(&base, &inflow(&minutes), 100);
    assert_eq!(levels(r), [1_001, 1_001, 100, 100]);
}

#[test]
fn steady_arrivals_raise_a_level_that_would_otherwise_be_the_minimum() {
    let d = depth(&[(1_000, B / 2)]);
    assert_eq!(levels(recommend(&d, &Inflow::default(), 100)), [100; 4]);
    // 50 kvB a minute at 2 sat/vB: 805 kvB before the next block at 80%, on top of half a
    // block at 1 sat/vB, does not fit: outbid the 1 sat/vB half. Three blocks have room.
    let minutes = vec![vec![(2_000, 50_000)]; 10];
    let r = recommend(&d, &inflow(&minutes), 100);
    assert_eq!(levels(r), [1_001, 100, 100, 100]);
}

#[test]
fn a_sudden_burst_counts_where_it_is_and_is_not_extrapolated() {
    let steady = vec![vec![(2_000, 10_000)]; 10];
    let base = depth(&[(2_000, B / 2), (500, 4 * B)]);
    let before = recommend(&base, &inflow(&steady), 100);
    // Five blocks at 50 sat/vB land in one minute.
    let mut minutes = steady;
    minutes[9].push((50_000, 5 * u32::try_from(B).unwrap()));
    let with_burst = depth(&[(50_000, 5 * B), (2_000, B / 2), (500, 4 * B)]);
    let r = recommend(&with_burst, &inflow(&minutes), 100);
    // The next three blocks are the burst's: outbid it. Six blocks hold it and the 2 sat/vB
    // half block, but not the arrivals at 2 sat/vB as well: outbid those too.
    assert_eq!(levels(before), [501, 501, 100, 100]);
    assert_eq!(levels(r), [50_001, 50_001, 2_001, 100]);
    // The burst's minute is among the busiest quarter, so it is left out of the arrivals.
    let i = inflow(&minutes);
    assert!((i.per_minute_at_or_above(2_000) - 10_000.0).abs() < 1e-9);
}

#[test]
fn bursts_on_a_quiet_network_are_not_taken_for_a_trend() {
    // Eight minutes, two of them bursts at 3 sat/vB, the rest silent: as on regtest or signet.
    let mut minutes = vec![Vec::new(); 8];
    minutes[2] = vec![(10_000, 720_000), (5_000, 720_000), (2_000, 720_000)];
    minutes[6] = vec![(3_000, 900_000)];
    let i = inflow(&minutes);
    assert_eq!(i.per_minute_at_or_above(100), 0.0);
    let r = recommend(&depth(&[(2_000, B), (3_000, B / 4)]), &i, 100);
    assert_eq!(levels(r), [2_001, 100, 100, 100]);
}

#[test]
fn whole_numbers_round_up_and_never_go_under_one() {
    assert_eq!(whole(0), 1);
    assert_eq!(whole(100), 1);
    assert_eq!(whole(1_000), 1);
    assert_eq!(whole(1_001), 2);
    assert_eq!(whole(2_633), 3);
    assert_eq!(whole(u32::MAX), 4_294_968);
}

#[test]
fn the_nodes_estimates_map_to_levels_in_order_and_above_the_floor() {
    let est: BTreeMap<u16, f64> = [(1, 4.299), (2, 2.453), (3, 1.799), (6, 1.597), (144, 0.128)]
        .into_iter()
        .collect();
    let r = from_node(&est, 100).unwrap();
    assert_eq!(levels(r), [4_299, 1_799, 1_597, 128]);
    // Missing targets take the next longer one; out-of-order answers are lifted; the floor wins.
    let est: BTreeMap<u16, f64> = [(2, 1.0), (5, 3.0), (1008, 0.05)].into_iter().collect();
    let r = from_node(&est, 100).unwrap();
    assert_eq!(levels(r), [3_000, 3_000, 100, 100]);
    assert_eq!(from_node(&BTreeMap::new(), 100), None);
    let nan: BTreeMap<u16, f64> = [(1, f64::NAN)].into_iter().collect();
    assert_eq!(from_node(&nan, 100), None);
}

#[test]
fn a_higher_minimum_seen_later_lifts_every_level() {
    let r = Rates {
        levels: [3_000, 150, 100, 100],
        minimum: 100,
    };
    let lifted = r.at_least(2_000);
    assert_eq!(
        (levels(lifted), lifted.minimum),
        ([3_000, 2_000, 2_000, 2_000], 2_000)
    );
    assert_eq!(r.at_least(50), r);
}

#[test]
fn inflow_counts_whole_minutes_and_quiet_ones_as_zero() {
    let mut i = Inflow::default();
    i.record(1_000, 2_000, 400);
    assert_eq!(i.minutes(), 0);
    assert!(!i.ready());
    i.roll(1_059);
    assert_eq!(i.minutes(), 0);
    // Three minutes later: the busy one and two quiet ones.
    i.roll(1_000 + 180);
    assert_eq!(i.minutes(), 3);
    assert!((i.per_minute_at_or_above(1) - 400.0 / 3.0).abs() < 1e-9);
    i.roll(1_000 + 300);
    assert!(i.ready());
    // Never more than the window, however long the silence.
    i.roll(1_000_000);
    assert_eq!(i.minutes(), INFLOW_MINUTES);
    assert_eq!(i.per_minute_at_or_above(1), 0.0);
    // A gap drops the minute in progress.
    i.record(1_000_010, 2_000, 999);
    i.gap(1_000_020);
    i.roll(1_000_080);
    assert_eq!(i.per_minute_at_or_above(1), 0.0);
}

fn tip(n: u8) -> Option<ChainTip> {
    Some(ChainTip {
        hash: BlockHash::from_byte_array([n; 32]),
        height: u32::from(n),
    })
}

fn rates(l: [u32; 4]) -> Rates {
    Rates {
        levels: l,
        minimum: 100,
    }
}

#[test]
fn smoothing_rises_at_once_falls_after_a_minute_and_starts_over_on_a_block() {
    let mut s = Smoother::default();
    assert_eq!(levels(s.push(0, tip(1), rates([500; 4]))), [500; 4]);
    assert_eq!(
        levels(s.push(2, tip(1), rates([900, 400, 400, 400]))),
        [900, 500, 500, 500]
    );
    assert_eq!(
        levels(s.push(30, tip(1), rates([300; 4]))),
        [900, 500, 500, 500]
    );
    assert_eq!(
        levels(s.push(61, tip(1), rates([300; 4]))),
        [900, 400, 400, 400]
    );
    assert_eq!(levels(s.push(62, tip(1), rates([300; 4]))), [300; 4]);
    s.push(63, tip(1), rates([800; 4]));
    // A new block: the old highs are gone.
    assert_eq!(levels(s.push(64, tip(2), rates([200; 4]))), [200; 4]);
}

#[test]
fn fee_range_is_seven_rates_by_vsize() {
    assert!(fee_range([]).is_empty());
    let txs = (1..=100u32).map(|r| (r * 10, 100));
    assert_eq!(fee_range(txs), vec![10, 100, 250, 500, 750, 900, 1_000]);
    assert_eq!(fee_range([(7, 1)]), vec![7; 7]);
}

#[test]
fn time_without_a_projection_is_not_counted_as_quiet_minutes() {
    let current = Projection {
        tip: tip(1),
        ..Projection::default()
    };
    let stale = Projection {
        stale: true,
        ..current.clone()
    };
    let mut r = Recommender::default();
    // Down from the start for ten minutes: nothing was watched, so no warm-up passed.
    for t in (0..600).step_by(2) {
        assert_eq!(r.update(t, &Projection::default(), 100), None);
    }
    assert_eq!(r.update(600, &current, 100), None);
    assert_eq!(r.update(600 + 299, &current, 100), None);
    assert!(r.update(600 + 300, &current, 100).is_some());
    // A blip under a minute keeps what was watched…
    assert_eq!(r.update(1_000, &stale, 100), None);
    assert!(r.update(1_030, &current, 100).is_some());
    // …a longer outage starts the warm-up over.
    assert_eq!(r.update(1_100, &stale, 100), None);
    assert_eq!(r.update(1_200, &current, 100), None);
    assert_eq!(r.update(1_499, &current, 100), None);
    assert!(r.update(1_500, &current, 100).is_some());
}

/// A mainnet moment recorded by `examples/fee_watch.rs`: every effective rate in the projected
/// blocks with its vsize, 30 minutes of arrivals, the node's minimum, the projected blocks'
/// lowest rates, and the answer the method gave.
#[test]
fn a_recorded_mainnet_mempool_gives_the_recorded_answer() {
    let f: serde_json::Value =
        serde_json::from_str(include_str!("../../../../fixtures/fees/mainnet.json")).unwrap();
    let pairs = |v: &serde_json::Value| -> Vec<(u32, u64)> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|p| (p[0].as_u64().unwrap() as u32, p[1].as_u64().unwrap()))
            .collect()
    };
    let d = Depth::new(pairs(&f["depth"]));
    let minutes: Vec<Vec<(u32, u32)>> = f["inflow"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| pairs(m).into_iter().map(|(r, v)| (r, v as u32)).collect())
        .collect();
    assert_eq!(minutes.len(), INFLOW_MINUTES);
    let i = inflow(&minutes);
    let floor = f["floor"].as_u64().unwrap() as u32;
    let r = recommend(&d, &i, floor);
    let expected: Vec<u32> = f["expected"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u32)
        .collect();
    assert_eq!([&r.levels[..], &[r.minimum]].concat(), expected);
    // Each level outbids the bottom of the projected block after its horizon: what pays the
    // bottom of block N or less does not all fit in N blocks.
    let bottoms: Vec<u32> = f["projected_min_rates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u32)
        .collect();
    assert!(r.levels[0] > bottoms[1], "{:?} {bottoms:?}", r.levels);
    assert!(r.levels[1] > bottoms[3].min(bottoms[2]), "{:?} {bottoms:?}", r.levels);
    // Without arrivals, the next block is the one-step-above-the-boundary answer.
    let still = recommend(&d, &Inflow::default(), floor);
    assert!(still.levels[0] <= r.levels[0]);
    assert!(d.at_or_above(still.levels[0]) < BLOCK_VSIZE);
    assert!(d.at_or_above(still.levels[0] - 1) >= BLOCK_VSIZE || still.levels[0] == floor.max(RELAY_FLOOR));
}

// Properties over random mempools from a deterministic generator.

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    /// Mostly realistic rates, sometimes absurd ones.
    fn rate(&mut self) -> u32 {
        let r = match self.below(10) {
            0 => self.below(u64::from(u32::MAX)),
            1..=5 => 100 + self.below(200),
            _ => 100 + self.below(50_000),
        };
        u32::try_from(r).unwrap()
    }
}

#[test]
fn random_mempools_are_ordered_floored_bounded_and_exactly_at_the_boundary() {
    let mut rng = Rng(0x5eed_f00d);
    for case in 0..150 {
        let txs = rng.below(if case % 4 == 0 { 50 } else { 3_000 });
        let d = Depth::new((0..txs).map(|_| (rng.rate(), 60 + rng.below(100_000))));
        let mut arrivals = Inflow::default();
        let minutes = rng.below(40);
        for m in 0..minutes {
            for _ in 0..rng.below(200) {
                let v = u32::try_from(60 + rng.below(20_000)).unwrap();
                arrivals.record(m * 60 + rng.below(60), rng.rate(), v);
            }
        }
        arrivals.roll(minutes * 60);
        let floor = u32::try_from(rng.below(3_000)).unwrap();
        let r = recommend(&d, &arrivals, floor);
        let min = floor.max(RELAY_FLOOR);
        assert_eq!(r.minimum, min);
        let top = d
            .max_rate()
            .max(arrivals.max_rate())
            .saturating_add(1)
            .max(min);
        for w in r.levels.windows(2) {
            assert!(w[0] >= w[1], "case {case}: {:?}", r.levels);
        }
        for (rate, level) in r.levels.into_iter().zip(LEVELS) {
            assert!(
                rate >= min && rate <= top,
                "case {case}: {rate} outside [{min}, {top}]"
            );
            let room = u64::from(level.blocks) * B;
            let fits = |x: u32| {
                d.at_or_above(x) as f64
                    + arrivals.per_minute_at_or_above(x) * level.arrival_secs as f64 / 60.0
                    < room as f64
            };
            // Monotonic lifting can only raise a slower level's answer to a faster one's, which
            // fits too; and the level's own answer is the lowest that fits.
            assert!(fits(rate), "case {case}: {} pays too little", level.name);
            let own = recommend_one(&d, &arrivals, min, level);
            assert!(own <= rate);
            if level.blocks == 1 {
                assert_eq!(own, rate, "case {case}");
            }
            if own > min {
                assert!(!fits(own - 1), "case {case}: {} not the lowest", level.name);
            }
        }
        for rate in r.levels {
            assert!(whole(rate) * 1000 >= u64::from(rate));
        }
    }
}

/// One level alone, by brute force over the candidate rates.
fn recommend_one(d: &Depth, i: &Inflow, floor: u32, level: Level) -> u32 {
    let room = u64::from(level.blocks) * B;
    let mut candidates: Vec<u32> = d.rates.iter().map(|r| r.saturating_add(1)).collect();
    for m in &i.closed {
        candidates.extend(m.rates.iter().map(|r| r.saturating_add(1)));
    }
    candidates.push(floor);
    candidates.retain(|c| *c >= floor);
    candidates.sort_unstable();
    candidates
        .into_iter()
        .find(|&x| {
            d.at_or_above(x) as f64 + i.per_minute_at_or_above(x) * level.arrival_secs as f64 / 60.0
                < room as f64
        })
        .unwrap_or(floor)
}
