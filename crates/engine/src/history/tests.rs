#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use bitcoin::hashes::Hash;
use bitcoin::{Amount, BlockHash, Txid};
use node::esplora::ChainTip;
use tokio::sync::mpsc;

use super::db::roll;
use super::*;
use crate::mempool::{self, Delta, Removal, Tracker, TxInfo};
use crate::projected::tests::{Mock, block, tx, vsize};
use crate::projected::{BlockSummary, ProjectedBlock, Projector};
use crate::store::{Error, Options, Store, params};

struct Temp {
    dir: PathBuf,
    store: Arc<Store>,
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn temp() -> Temp {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "mempool-based-history-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let store = Arc::new(Store::open(&dir, "regtest", Options::default()).unwrap());
    Temp { dir, store }
}

/// A minute's sample whose every field is derived from `n`, so sums are checkable.
fn minute(time: u64, n: u64) -> Point {
    let mut bands = [0; BANDS];
    for (i, b) in bands.iter_mut().enumerate() {
        *b = n * (i as u64 + 1);
    }
    Point {
        time,
        samples: 1,
        sums: Totals {
            count: n,
            vsize: 55 * n,
            fees: 7 * n,
            min_fee: 100,
            bands,
            flow: Flow {
                in_count: n % 5,
                in_vsize: 3 * n,
                out_count: n % 3,
                out_vsize: 2 * n,
                mined_count: n % 2,
                mined_vsize: n,
            },
        },
    }
}

/// Every stored row, by level then time.
async fn rows(store: &Store) -> Vec<(i64, Point)> {
    store
        .read(|sql| {
            let mut stmt = sql.prepare(
                "SELECT level, time, samples, count, vsize, fees, min_fee, band0, band1, band2, \
                 band3, band4, band5, band6, band7, band8, band9, in_count, in_vsize, out_count, \
                 out_vsize, mined_count, mined_vsize FROM mempool_history ORDER BY level, time",
            )?;
            let rows = stmt.query_map(params![], |r| {
                let v = |i: usize| -> u64 { u64::try_from(r.get::<_, i64>(i).unwrap()).unwrap() };
                let mut bands = [0; BANDS];
                for (i, b) in bands.iter_mut().enumerate() {
                    *b = v(7 + i);
                }
                Ok((
                    r.get(0)?,
                    Point {
                        time: v(1),
                        samples: v(2),
                        sums: Totals {
                            count: v(3),
                            vsize: v(4),
                            fees: v(5),
                            min_fee: v(6),
                            bands,
                            flow: Flow {
                                in_count: v(17),
                                in_vsize: v(18),
                                out_count: v(19),
                                out_vsize: v(20),
                                mined_count: v(21),
                                mined_vsize: v(22),
                            },
                        },
                    },
                ))
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await
        .unwrap()
}

fn total(rows: &[(i64, Point)]) -> (u64, Totals) {
    let mut t = Totals::default();
    let mut samples = 0;
    for (_, p) in rows {
        t.add(&p.sums);
        samples += p.samples;
    }
    (samples, t)
}

// Sampling.

#[test]
fn a_sample_is_the_projections_totals() {
    let summary = |txs, vsize, fees, band: usize| {
        let mut bands = [0; BANDS];
        bands[band] = vsize;
        Arc::new(ProjectedBlock {
            summary: BlockSummary {
                txs,
                vsize,
                fees,
                bands,
                ..BlockSummary::default()
            },
            txs: Vec::new(),
        })
    };
    let p = Projection {
        blocks: vec![summary(3, 900, 5_000, 9), summary(2, 400, 80, 0)],
        ..Projection::default()
    };
    let flow = Flow {
        in_count: 4,
        ..Flow::default()
    };
    let s = sample(1_800_000_060, &p, Amount::from_sat(1_000), flow);
    assert_eq!((s.time, s.samples), (1_800_000_060, 1));
    let t = s.sums;
    assert_eq!(
        (t.count, t.vsize, t.fees, t.min_fee),
        (5, 1_300, 5_080, 1_000)
    );
    assert_eq!(t.bands, [400, 0, 0, 0, 0, 0, 0, 0, 0, 900]);
    assert_eq!(t.bands.iter().sum::<u64>(), t.vsize);
    assert_eq!(t.flow, flow);
}

fn id(n: u8) -> Txid {
    Txid::from_byte_array([n; 32])
}

fn delta(seq: u64, added: &[(u8, u32)], removed: &[(u8, Removal)]) -> Delta {
    Delta {
        seq,
        mempool_sequence: seq,
        tip: ChainTip {
            hash: BlockHash::all_zeros(),
            height: 1,
        },
        added: added
            .iter()
            .map(|(n, v)| {
                let info = TxInfo::new(Amount::from_sat(100), Amount::ZERO, *v, v * 4, 0);
                (id(*n), info)
            })
            .collect(),
        removed: removed.iter().map(|(n, r)| (id(*n), *r)).collect(),
        replaced: Vec::new(),
    }
}

#[test]
fn flow_counts_arrivals_departures_and_what_was_mined() {
    // Two transactions held before the snapshot.
    let mut w = Watch::new([(id(1), 200), (id(2), 300)].into_iter().collect());
    let mined = Removal::Confirmed {
        height: 2,
        block: BlockHash::all_zeros(),
    };
    w.apply(&delta(1, &[(3, 150), (4, 250)], &[]));
    w.apply(&delta(2, &[(5, 110)], &[(1, mined), (3, Removal::Evicted)]));
    w.apply(&delta(
        3,
        &[],
        &[
            (4, Removal::Replaced { by: id(5) }),
            (2, mined),
            (9, Removal::Evicted), // never seen: not counted
        ],
    ));
    let (f, whole) = w.take();
    assert!(whole);
    assert_eq!(
        f,
        Flow {
            in_count: 3,
            in_vsize: 510,
            out_count: 4,
            out_vsize: 900,
            mined_count: 2,
            mined_vsize: 500,
        }
    );
    // The mempool's size moved by exactly arrivals less departures: 500 + 510 - 900 = 110.
    assert_eq!(w.sizes.values().map(|v| u64::from(*v)).sum::<u64>(), 110);
    // The next minute starts from zero; a lag makes it incomplete once.
    assert_eq!(w.take(), (Flow::default(), true));
    w.resync([(id(5), 110), (id(6), 90)].into_iter().collect());
    w.apply(&delta(4, &[(7, 10)], &[(6, Removal::Evicted)]));
    let (f, whole) = w.take();
    assert!(!whole);
    assert_eq!((f.in_vsize, f.out_vsize), (10, 90));
    assert!(w.take().1);
}

// Roll-up.

const NOW: u64 = 1_800_000_000; // a Unix time; 2027-01-15 08:00 UTC

#[tokio::test]
async fn roll_up_keeps_totals_cuts_at_whole_periods_and_is_idempotent() {
    let t = temp();
    let cut = (NOW - KEEP_MINUTES) / HOUR * HOUR;
    // Minutes from three hours before the cut to an hour after it, with a gap, and a few
    // minutes of the present.
    let mut minutes: Vec<Point> = (0..240)
        .filter(|i| !(70..95).contains(i))
        .map(|i| minute(cut - 3 * HOUR + i * MINUTE, i + 1))
        .collect();
    minutes.extend((0..5).map(|i| minute(NOW - i * MINUTE, 1_000 + i)));
    insert(&t.store, Level::Minute, minutes.clone())
        .await
        .unwrap();
    let before = rows(&t.store).await;
    let (samples, totals) = total(&before);
    assert_eq!(samples, 215 + 5);

    let rolled = roll_up(&t.store, NOW).await.unwrap();
    assert_eq!(
        rolled,
        Rolled {
            minutes: 155,
            hours: 0
        }
    ); // 180 minutes less the gap of 25
    let after = rows(&t.store).await;
    assert_eq!(
        total(&after),
        (samples, totals),
        "nothing lost, nothing added"
    );
    // Three hour rows, each the sum of its own minutes; the gap shows as fewer samples.
    let hours: Vec<&Point> = after
        .iter()
        .filter(|(l, _)| *l == 1)
        .map(|(_, p)| p)
        .collect();
    assert_eq!(
        hours
            .iter()
            .map(|p| (p.time, p.samples))
            .collect::<Vec<_>>(),
        vec![(cut - 3 * HOUR, 60), (cut - 2 * HOUR, 35), (cut - HOUR, 60)]
    );
    for h in &hours {
        let mut sum = Totals::default();
        for m in minutes.iter().filter(|m| m.time / HOUR * HOUR == h.time) {
            sum.add(&m.sums);
        }
        assert_eq!(h.sums, sum);
    }
    // Every minute at or past the cut is still a minute.
    let left: Vec<u64> = after
        .iter()
        .filter(|(l, _)| *l == 0)
        .map(|(_, p)| p.time)
        .collect();
    assert_eq!(left.len(), 60 + 5);
    assert!(left.iter().all(|t| *t >= cut));

    // Again: nothing to do, nothing changes.
    assert_eq!(roll_up(&t.store, NOW).await.unwrap(), Rolled::default());
    assert_eq!(rows(&t.store).await, after);

    // A minute later still nothing; an hour later the next hour goes, merging into nothing.
    assert_eq!(
        roll_up(&t.store, NOW + 59).await.unwrap(),
        Rolled::default()
    );
    let rolled = roll_up(&t.store, NOW + HOUR).await.unwrap();
    assert_eq!(
        rolled,
        Rolled {
            minutes: 60,
            hours: 0
        }
    );
    assert_eq!(total(&rows(&t.store).await), (samples, totals));

    // Four hundred days on, the rest of the minutes become hours (two: they straddle an hour)
    // and every hour a day, in one write; the days keep every sample.
    let later = NOW + KEEP_HOURS + DAY;
    let rolled = roll_up(&t.store, later).await.unwrap();
    assert_eq!(
        rolled,
        Rolled {
            minutes: 5,
            hours: 4 + 2
        }
    );
    let after = rows(&t.store).await;
    assert_eq!(total(&after), (samples, totals));
    assert!(after.iter().all(|(l, _)| *l == 2));
    assert_eq!(
        after.len(),
        2,
        "one day for the old minutes, one for today's"
    );
}

#[tokio::test]
async fn a_roll_up_merges_into_an_existing_row_and_an_interrupted_one_leaves_nothing() {
    let t = temp();
    let cut = (NOW - KEEP_MINUTES) / HOUR * HOUR;
    let hour = cut - HOUR;
    // An hour row already holds half the hour (rolled by an earlier run); the rest is minutes.
    let mut early = Totals::default();
    for i in 0..30 {
        early.add(&minute(hour + i * MINUTE, i).sums);
    }
    let row = Point {
        time: hour,
        samples: 30,
        sums: early,
    };
    insert(&t.store, Level::Hour, vec![row]).await.unwrap();
    let late: Vec<Point> = (30..60).map(|i| minute(hour + i * MINUTE, i)).collect();
    insert(&t.store, Level::Minute, late).await.unwrap();
    let before = rows(&t.store).await;

    // A write that rolls and then fails is rolled back whole: the store owns the transaction.
    let failed = t
        .store
        .write(move |sql| {
            assert_eq!(roll(sql, NOW)?.minutes, 30);
            Err::<(), _>(Error::Sql(crate::store::SqlError::QueryReturnedNoRows))
        })
        .await;
    assert!(failed.is_err());
    assert_eq!(rows(&t.store).await, before);

    roll_up(&t.store, NOW).await.unwrap();
    let after = rows(&t.store).await;
    assert_eq!(after.len(), 1);
    let mut whole = Totals::default();
    for i in 0..60 {
        whole.add(&minute(hour + i * MINUTE, i).sums);
    }
    assert_eq!(
        after[0].1,
        Point {
            time: hour,
            samples: 60,
            sums: whole
        }
    );
}

#[tokio::test]
async fn a_minute_whose_hour_or_day_was_rolled_already_is_not_counted_again() {
    let t = temp();
    let hour = NOW - 10 * DAY / HOUR * HOUR;
    let day = (NOW - 500 * DAY) / DAY * DAY;
    let rolled = |time, samples| Point {
        time,
        samples,
        sums: minute(0, 1).sums,
    };
    insert(&t.store, Level::Hour, vec![rolled(hour, 60)])
        .await
        .unwrap();
    insert(&t.store, Level::Day, vec![rolled(day, 1_440)])
        .await
        .unwrap();
    let before = rows(&t.store).await;
    // As after a clock that ran ahead (and rolled them) came back and the server restarted.
    record(&t.store, minute(hour + 5 * MINUTE, 7), hour + 6 * MINUTE)
        .await
        .unwrap();
    record(&t.store, minute(day + 3 * HOUR, 7), day + 4 * HOUR)
        .await
        .unwrap();
    assert_eq!(rows(&t.store).await, before);
    roll_up(&t.store, NOW).await.unwrap();
    assert_eq!(total(&rows(&t.store).await).0, 60 + 1_440);
    // A minute of an hour not rolled is stored.
    record(&t.store, minute(NOW, 7), NOW).await.unwrap();
    assert_eq!(rows(&t.store).await.len(), 3);
}

// Reading.

#[tokio::test]
async fn a_series_buckets_by_step_leaves_gaps_out_and_says_what_was_sampled() {
    let t = temp();
    let now = NOW + 25; // 25 s into a minute
    // Two hours of minutes but for a gap of ten, the minute in progress included.
    let rows: Vec<Point> = (0..=120)
        .filter(|i| !(40..50).contains(i))
        .map(|i| minute(NOW - i * MINUTE, i + 1))
        .collect();
    insert(&t.store, Level::Minute, rows).await.unwrap();

    let s = series(&t.store, Interval::TwoHours, now).await.unwrap();
    assert_eq!(
        (s.from, s.step, s.to),
        (NOW - 2 * HOUR + MINUTE, MINUTE, now)
    );
    assert_eq!(s.first, Some(NOW - 2 * HOUR));
    // One point a minute from `from`, the gap left out: 120 minutes, 10 of them missing.
    assert_eq!(s.points.len(), 110);
    assert!(s.points.windows(2).all(|w| w[0].time < w[1].time));
    assert!(!s.points.iter().any(|p| p.time == NOW - 45 * MINUTE));
    assert_eq!(s.points.last().unwrap().sums, minute(NOW, 1).sums);
    // The minute in progress is not owed; 119 minutes are, 109 were sampled.
    assert_eq!((s.minutes, s.sampled), (119, 109));
    assert!(!s.complete());
    assert_eq!(s.earliest(), Some(s.from));

    // Rows stamped ahead of the time asked (a clock that ran ahead) are left out.
    insert(
        &t.store,
        Level::Minute,
        (1..=3).map(|i| minute(NOW + i * HOUR, 1)).collect(),
    )
    .await
    .unwrap();
    let again = series(&t.store, Interval::TwoHours, now).await.unwrap();
    assert_eq!(again.points, s.points);

    // Ten-minute points over a week: each sums its minutes; the gap only thins one.
    let w = series(&t.store, Interval::Week, now).await.unwrap();
    assert_eq!(w.step, 10 * MINUTE);
    assert_eq!(w.from % w.step, 0);
    assert!(w.points.iter().all(|p| p.time % w.step == 0));
    assert_eq!(w.points.iter().map(|p| p.samples).sum::<u64>(), 111);
    let gap = w
        .points
        .iter()
        .find(|p| p.time == NOW - 50 * MINUTE)
        .unwrap();
    assert_eq!(gap.samples, 1); // only the minute 50 before now, of 10
    assert!(!w.complete());
}

#[tokio::test]
async fn a_series_over_rolled_rows_counts_their_samples_and_coverage_is_honest() {
    let t = temp();
    let now = NOW + 30;
    // Hours from 30 days ago to 9, then eight days of whole minutes, as a roll-up leaves them.
    let hours: Vec<Point> = (9 * 24..30 * 24)
        .map(|h| Point {
            time: (NOW / HOUR - h) * HOUR,
            samples: 60,
            sums: minute(0, h).sums,
        })
        .collect();
    let minutes: Vec<Point> = (1..=8 * 24 * 60)
        .map(|i| minute(NOW - i * MINUTE, 2))
        .chain([minute(NOW, 2)])
        .collect();
    insert(&t.store, Level::Hour, hours).await.unwrap();
    insert(&t.store, Level::Minute, minutes).await.unwrap();

    // A month at one point an hour: minute rows sum into hours; the gap between the hour rows
    // and the minutes (day 8 to 9) shows.
    let m = series(&t.store, Interval::Month, now).await.unwrap();
    assert_eq!(m.step, HOUR);
    assert!(m.points.iter().all(|p| p.samples <= 60));
    assert!(m.points.iter().all(|p| p.time % HOUR == 0));
    assert!(!m.complete());
    assert_eq!(m.first, Some((NOW / HOUR - 30 * 24 + 1) * HOUR));

    // The last day alone is whole.
    let d = series(&t.store, Interval::Day, now).await.unwrap();
    assert_eq!(d.points.len(), 1440);
    assert!(d.complete(), "{} of {}", d.sampled, d.minutes);

    // `all` starts at the oldest row, a day a point, and owes every minute since it.
    let a = series(&t.store, Interval::All, now).await.unwrap();
    assert_eq!(a.step, DAY);
    assert!(a.points.len() <= 31);
    assert_eq!(
        a.points.iter().map(|p| p.samples).sum::<u64>(),
        21 * 24 * 60 + 8 * 1440 + 1
    );
    assert!(!a.complete());

    // Nothing stored: no points, nothing complete.
    let e = temp();
    let s = series(&e.store, Interval::All, now).await.unwrap();
    assert!(s.points.is_empty() && s.first.is_none() && !s.complete());
}

// The sampler, on a real tracker and projector over a mock node, with time paused.

#[tokio::test(start_paused = true)]
async fn samples_each_minute_skips_stale_and_syncing_minutes_and_rolls_up() {
    let t = temp();
    // History already holds minutes from ten days ago: the first write rolls them up.
    let wall = Duration::from_secs(NOW + 20);
    let old: Vec<Point> = (0..3)
        .map(|i| minute(NOW - 10 * DAY + i * MINUTE, 1))
        .collect();
    insert(&t.store, Level::Minute, old).await.unwrap();

    let mock = Mock::default();
    let b0 = block(BlockHash::all_zeros(), &[&tx(9_000)]);
    let h0 = b0.block_hash();
    mock.mine(b0, 0);
    let (a, b, c) = (tx(1), tx(2), tx(3));
    mock.set(&[(&a, 500)]);
    let (tracker, tracker_task) = Tracker::start(mock.clone(), mempool::Config::default());
    let (projector, projector_task) = Projector::start(
        tracker.clone(),
        mock.clone(),
        crate::projected::Config::default(),
    );

    let start = tokio::time::Instant::now();
    // Seconds the wall clock is set back by, later in the test.
    let behind = Arc::new(AtomicU64::new(0));
    let (tx_events, mut events) = mpsc::unbounded_channel();
    let config = Config {
        clock: {
            let back = Arc::clone(&behind);
            Arc::new(move || {
                wall + start.elapsed() - Duration::from_secs(back.load(Ordering::SeqCst))
            })
        },
        observer: Arc::new(move |e: &Event| {
            let _ = tx_events.send(e.clone());
        }),
    };
    let task = start_sampler(&tracker, &projector, &t.store, config);
    let mut next = async || events.recv().await.unwrap();

    let first = NOW + MINUTE;
    // The first minute was not watched whole.
    assert_eq!(
        next().await,
        Event::Skipped {
            time: first,
            reason: Skip::Syncing
        }
    );
    let Event::Written { time, rolled, .. } = next().await else {
        panic!("the second minute is written")
    };
    assert_eq!(
        (time, rolled),
        (
            first + MINUTE,
            Rolled {
                minutes: 3,
                hours: 0
            }
        )
    );

    // Two arrive, then one of them and the first are mined.
    mock.set(&[(&a, 500), (&b, 700), (&c, 900)]);
    assert!(matches!(next().await, Event::Written { time, .. } if time == first + 2 * MINUTE));
    mock.set(&[(&c, 900)]);
    mock.mine(block(h0, &[&tx(9_001), &a, &b]), 1);
    tracker.tip_changed();
    assert!(matches!(next().await, Event::Written { time, .. } if time == first + 3 * MINUTE));

    // The node stops answering cluster requests while the mempool changes: stale minutes are
    // skipped, and so is the first minute back, whose flow spans the outage.
    mock.0.lock().unwrap().fail_clusters = true;
    mock.set(&[(&c, 900), (&tx(4), 100)]);
    let mut skipped = Vec::new();
    loop {
        match next().await {
            Event::Skipped {
                reason: Skip::Stale,
                time,
            } => {
                skipped.push(time);
                if skipped.len() == 2 {
                    mock.0.lock().unwrap().fail_clusters = false;
                }
            }
            Event::Written { time, .. } if skipped.is_empty() => {
                assert!(time < first + 6 * MINUTE, "staleness takes a cycle to show");
            }
            e => panic!("{e:?} while stale"),
        }
        if skipped.len() == 2 {
            break;
        }
    }
    let back = skipped[1] + MINUTE;
    assert_eq!(
        next().await,
        Event::Skipped {
            time: back,
            reason: Skip::Syncing
        }
    );
    assert!(matches!(next().await, Event::Written { time, .. } if time == back + MINUTE));

    let s = series(&t.store, Interval::TwoHours, back + MINUTE + 1)
        .await
        .unwrap();
    let at = |time| *s.points.iter().find(|p| p.time == time).unwrap();
    let (va, vb, vc) = (vsize(&a), vsize(&b), vsize(&c));
    let p = at(first + MINUTE);
    assert_eq!((p.sums.count, p.sums.vsize, p.sums.fees), (1, va, 500));
    assert_eq!(p.sums.bands.iter().sum::<u64>(), va);
    let p = at(first + 2 * MINUTE).sums;
    assert_eq!((p.count, p.vsize), (3, va + vb + vc));
    assert_eq!(
        (p.flow.in_count, p.flow.in_vsize, p.flow.out_count),
        (2, vb + vc, 0)
    );
    let p = at(first + 3 * MINUTE).sums;
    assert_eq!((p.count, p.flow.in_count), (1, 0));
    assert_eq!((p.flow.out_count, p.flow.out_vsize), (2, va + vb));
    assert_eq!((p.flow.mined_count, p.flow.mined_vsize), (2, va + vb));
    assert!(
        skipped
            .iter()
            .all(|t| s.points.iter().all(|p| p.time != *t))
    );
    assert!(!s.points.iter().any(|p| p.time == back));

    // The clock is set back two minutes, and transactions arrive meanwhile: the minutes it
    // repeats are not sampled again, their flow is dropped, and the first minute past the last
    // one sampled is skipped too, so no sample counts more than one minute's flow.
    let latest = back + MINUTE;
    behind.store(2 * MINUTE, Ordering::SeqCst);
    mock.set(&[(&c, 900), (&tx(4), 100), (&tx(5), 100), (&tx(6), 100)]);
    assert_eq!(
        next().await,
        Event::Skipped {
            time: latest + MINUTE,
            reason: Skip::Syncing
        }
    );
    mock.set(&[
        (&c, 900),
        (&tx(4), 100),
        (&tx(5), 100),
        (&tx(6), 100),
        (&tx(7), 100),
    ]);
    assert!(matches!(next().await, Event::Written { time, .. } if time == latest + 2 * MINUTE));
    let s = series(&t.store, Interval::TwoHours, latest + 2 * MINUTE)
        .await
        .unwrap();
    let p = s.points.last().unwrap();
    assert_eq!(
        (p.time, p.sums.count, p.sums.flow.in_count),
        (latest + 2 * MINUTE, 5, 1)
    );

    task.abort();
    projector_task.abort();
    tracker_task.abort();
}

fn start_sampler(
    tracker: &Tracker,
    projector: &Projector,
    store: &Arc<Store>,
    config: Config,
) -> tokio::task::JoinHandle<()> {
    start(
        tracker.clone(),
        projector.clone(),
        Arc::clone(store),
        config,
    )
}

/// Fills a table with a synthetic year of minutes and ten years rolled up, and times the
/// roll-up and every interval's query. `HISTORY_BENCH_DIR=<dir> cargo test --release -p engine
/// -- --ignored --nocapture history_benchmark`
#[tokio::test(flavor = "multi_thread")]
#[ignore = "a benchmark: fills a table with years of samples"]
async fn history_benchmark() {
    let dir = PathBuf::from(std::env::var("HISTORY_BENCH_DIR").unwrap());
    let _ = std::fs::remove_dir_all(&dir);
    let store = Store::open(&dir, "regtest", Options::default()).unwrap();
    let file = dir.join("regtest.sqlite");
    let size = || {
        ["", "-wal"]
            .iter()
            .map(|s| std::fs::metadata(format!("{}{s}", file.display())).map_or(0, |m| m.len()))
            .sum::<u64>()
    };
    let now = NOW;
    // Ten years ago to now: nine years as days (already rolled), the last year as minutes.
    let days: Vec<Point> = (366..3_653)
        .map(|d| Point {
            time: (now / DAY - d) * DAY,
            samples: 1_440,
            sums: minute(0, 1_440 * 60_000).sums,
        })
        .collect();
    insert(&store, Level::Day, days).await.unwrap();
    let started = std::time::Instant::now();
    let year = 365 * 24 * 60;
    for chunk in (0..year).collect::<Vec<u64>>().chunks(10_000) {
        let rows = chunk
            .iter()
            .map(|i| minute(now - (year - 1 - i) * MINUTE, 60_000 + i % 997))
            .collect();
        insert(&store, Level::Minute, rows).await.unwrap();
    }
    println!(
        "a year of minutes and nine years of days: written in {:?}, {} MB",
        started.elapsed(),
        size() / 1_000_000
    );
    for i in Interval::ALL {
        let started = std::time::Instant::now();
        let s = series(&store, i, now).await.unwrap();
        println!(
            "  unrolled {:>3}: {:>5} points in {:?}",
            i.name(),
            s.points.len(),
            started.elapsed()
        );
    }
    let started = std::time::Instant::now();
    let rolled = roll_up(&store, now).await.unwrap();
    println!("roll-up of a year: {rolled:?} in {:?}", started.elapsed());
    let started = std::time::Instant::now();
    let again = roll_up(&store, now + MINUTE).await.unwrap();
    println!(
        "a roll-up with nothing to do: {again:?} in {:?}",
        started.elapsed()
    );
    let n: i64 = store
        .read(|sql| {
            Ok(sql
                .prepare("SELECT count(*) FROM mempool_history")?
                .query_row(params![], |r| r.get(0))?)
        })
        .await
        .unwrap();
    for i in Interval::ALL {
        let mut times = Vec::new();
        let mut points = 0;
        for _ in 0..5 {
            let started = std::time::Instant::now();
            points = series(&store, i, now).await.unwrap().points.len();
            times.push(started.elapsed());
        }
        times.sort();
        println!(
            "  rolled {:>3}: {:>5} points, median {:?}, max {:?}",
            i.name(),
            points,
            times[2],
            times[4]
        );
    }
    store.close().await;
    println!(
        "rows {n}, file {} KB after the roll-up, closed",
        size() / 1_000
    );

    // The same rows written afresh: what a table holds at the bound, without the free pages a
    // year of minutes left behind.
    let _ = std::fs::remove_dir_all(&dir);
    let store = Store::open(&dir, "regtest", Options::default()).unwrap();
    let mut rows = Vec::new();
    for (level, from, to, period) in [
        (Level::Day, 0, now - KEEP_HOURS, DAY),
        (Level::Hour, now - KEEP_HOURS, now - KEEP_MINUTES, HOUR),
        (Level::Minute, now - KEEP_MINUTES, now + 1, MINUTE),
    ] {
        let start = (now - 10 * 365 * DAY).max(from) / period * period;
        let pts: Vec<Point> = (start..to)
            .step_by(usize::try_from(period).unwrap())
            .map(|t| Point {
                samples: period / MINUTE,
                ..minute(t, 60_000 * period / MINUTE)
            })
            .collect();
        rows.push((level, pts.len()));
        insert(&store, level, pts).await.unwrap();
    }
    store.close().await;
    println!(
        "ten years at the bound: {rows:?}, file {} KB",
        size() / 1_000
    );
}
