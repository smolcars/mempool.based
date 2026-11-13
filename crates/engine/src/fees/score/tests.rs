#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::fees::db::{self, Batch};
use crate::fees::{Depth, Inflow, recommend};
use crate::store::{Options, Store};

fn store() -> (std::path::PathBuf, Store) {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "rbitcoin-space-fee-score-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let store = Store::open(&dir, "regtest", Options::default()).unwrap();
    (dir, store)
}

/// `fixtures/fees/replay.json` replayed as its own test does, every sample's answer and every
/// block recorded through the store as the server records them, read back and scored: the
/// same met shares as PR #97's scorer gave for that file (14 of 17, 15 of 15, 8 of 9).
#[tokio::test]
async fn the_recorded_fixture_scores_as_the_recorder_scored_it() {
    let f: serde_json::Value =
        serde_json::from_str(include_str!("../../../../../fixtures/fees/replay.json")).unwrap();
    let pairs = |v: &serde_json::Value| -> Vec<(u32, u64)> {
        v.as_array()
            .unwrap()
            .chunks(2)
            .map(|p| (p[0].as_u64().unwrap() as u32, p[1].as_u64().unwrap()))
            .collect()
    };
    let start = f["start"].as_u64().unwrap();
    let minutes = f["minutes"].as_array().unwrap();
    let mut inflow = Inflow::default();
    inflow.roll(start);
    let mut next = 0;
    let mut samples = Vec::new();
    for s in f["samples"].as_array().unwrap() {
        let t = s[0].as_u64().unwrap();
        while let Some(m) = minutes.get(next).filter(|_| start + next as u64 * 60 <= t) {
            for (rate, vsize) in pairs(m) {
                inflow.record(
                    start + next as u64 * 60,
                    rate,
                    u32::try_from(vsize).unwrap(),
                );
            }
            next += 1;
        }
        inflow.roll(t);
        let r = recommend(
            &Depth::new(pairs(&s[2])),
            &inflow,
            s[1].as_u64().unwrap() as u32,
        );
        samples.push(Sample {
            t,
            served: Served::Ours(r.levels),
        });
    }
    let blocks: Vec<Block> = f["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .zip(975_661..)
        .map(|(b, height)| Block {
            height,
            t: b[0].as_u64().unwrap(),
            clearing: b[1].as_u64().map(|c| c as u32),
        })
        .collect();
    let now = blocks.last().unwrap().t;

    let (dir, store) = store();
    store.set_gate(crate::store::Gate::Open);
    // One write per sample and per block, as the server sends them.
    for s in &samples {
        let batch = Batch {
            samples: vec![*s],
            blocks: Vec::new(),
        };
        db::write(&store, store.generation(), batch, now)
            .await
            .unwrap();
    }
    for b in &blocks {
        let batch = Batch {
            samples: Vec::new(),
            blocks: vec![*b],
        };
        db::write(&store, store.generation(), batch, now)
            .await
            .unwrap();
    }
    let (s, b) = db::load(&store, now).await.unwrap();
    assert_eq!((s.len(), b.len()), (samples.len(), blocks.len()));
    let record = Record::new(s, b);
    let report = record.report(&[(0, u64::MAX)]).pop().unwrap();
    let met: Vec<(usize, usize)> = report.levels[..3]
        .iter()
        .map(|t| (t.met, t.total))
        .collect();
    assert_eq!(met, [(14, 17), (15, 15), (8, 9)]);
    // The day-long horizons have not finished.
    assert_eq!(report.levels[3].unfinished, 17);

    // A month and a day on, the next block's write deletes every row that aged.
    let later = now + db::KEEP_BLOCKS + 1;
    let batch = Batch {
        samples: Vec::new(),
        blocks: vec![block(975_700, later, None)],
    };
    db::write(&store, store.generation(), batch, later).await.unwrap();
    let (s, b) = db::load(&store, now).await.unwrap();
    assert!(s.is_empty());
    assert_eq!(b, [block(975_700, later, None)]);
    drop(store);
    let _ = std::fs::remove_dir_all(dir);
}

fn block(height: u32, t: u64, clearing: Option<u32>) -> Block {
    Block {
        height,
        t,
        clearing,
    }
}

#[test]
fn outages_reorgs_and_age_are_handled() {
    let ours = |t, r| Sample {
        t,
        served: Served::Ours([r; 4]),
    };
    let mut rec = Record::default();
    for t in [0, 100, 200, 9_000, 9_100] {
        assert!(rec.sample(ours(t, 1_000)));
    }
    // A clock that went back is not a sample.
    assert!(!rec.sample(ours(50, 1_000)));
    assert!(rec.sample(Sample {
        t: 9_200,
        served: Served::Node,
    }));
    rec.block(block(1, 300, Some(500)));
    // Over two hours later: the node had lost the network.
    rec.block(block(2, 8_000, Some(2_000)));
    rec.block(block(3, 9_500, None));
    // A reorg replaces the block at its height.
    rec.block(block(3, 9_600, Some(999)));
    assert_eq!(rec.blocks().len(), 3);
    let r = rec.report(&[(0, 10_000)]).pop().unwrap();
    let high = &r.levels[0];
    // 0, 100 and 200 waited for block 1 and met it; 9,000 and 9,100 waited for block 3, inside
    // the outage's aftermath.
    assert_eq!((high.met, high.total, high.left_out), (3, 3, 2));
    assert_eq!(r.node, 1);
    assert!(high.interval().0.is_nan(), "one block is no interval");
    rec.prune(150, 500);
    assert_eq!(rec.samples().first().unwrap().t, 200);
    assert_eq!(rec.last_height(), Some(3));
    assert_eq!(rec.blocks().len(), 2);
}

#[test]
fn the_clearing_rate_is_the_first_percentile_by_size() {
    // One cheap outlier under 1% of the block does not set it.
    let mut rates = vec![(10, 5), (1_000, 600), (2_000, 400)];
    assert_eq!(clearing(&mut rates), Some(1_000));
    assert_eq!(clearing(&mut []), None);
}
