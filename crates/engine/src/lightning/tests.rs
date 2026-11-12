#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bitcoin::hashes::Hash;
use bitcoin::{BlockHash, Txid};

use super::*;
use crate::store::{Options, Store};

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
        "mempool-based-ln-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let store = Arc::new(Store::open(&dir, "regtest", Options::default()).unwrap());
    Temp { dir, store }
}

fn block(height: u32, tag: u8) -> Block {
    Block {
        height,
        hash: BlockHash::from_byte_array([tag; 32]),
        time: 1_700_000_000 + u64::from(height),
    }
}

fn open(scid: u64, tag: u8) -> Open {
    Open {
        scid,
        node_one: [2; 33],
        node_two: [3; 33],
        capacity: 1_000_000 + scid,
        block: block(u32::try_from(scid >> 40).unwrap(), tag),
    }
}

fn close(height: u32, kind: Kind, tx: u8) -> Close {
    Close {
        block: block(height, 0xc0),
        txid: Txid::from_byte_array([tx; 32]),
        kind,
        fee: Some(150),
    }
}

const A: u64 = 800_000 << 40 | 5 << 16;
const B: u64 = 800_001 << 40 | 1 << 16 | 1;
const C: u64 = 800_002 << 40;

/// Every row, newest funding first.
async fn table(store: &Store) -> Vec<Channel> {
    opened(store, 1_000).await.unwrap()
}

#[tokio::test]
async fn the_table_follows_opens_closes_penalties_and_their_undoing() {
    let t = temp();
    let s = &t.store;
    let penalty = Penalty {
        block: block(800_020, 0xd0),
        txid: Txid::from_byte_array([9; 32]),
    };
    apply(
        s,
        vec![
            Change::Opened(open(A, 1)),
            Change::Opened(open(B, 1)),
            Change::Opened(open(C, 1)),
            Change::Closed(open(A, 1), close(800_010, Kind::Force, 7)),
            Change::Closed(open(B, 1), close(800_011, Kind::Cooperative, 8)),
            Change::Penalty(A, penalty),
            // A penalty for an open channel names nothing stored.
            Change::Penalty(C, penalty),
        ],
    )
    .await
    .unwrap();
    let rows = table(s).await;
    assert_eq!(
        rows.iter().map(|c| c.open.scid).collect::<Vec<_>>(),
        [C, B, A]
    );
    assert_eq!(rows[2].close, Some(close(800_010, Kind::Force, 7)));
    assert_eq!(rows[2].penalty, Some(penalty));
    assert_eq!(rows[2].open, open(A, 1));
    assert_eq!((rows[0].close, rows[0].penalty), (None, None));
    // Newest close first.
    let closes = closed(s, 10).await.unwrap();
    assert_eq!(
        closes.iter().map(|c| c.open.scid).collect::<Vec<_>>(),
        [B, A]
    );
    assert_eq!(closed(s, 1).await.unwrap().len(), 1);

    // A reorg: the penalty, then A's close, then C's open leave the chain; C is found again in
    // another block.
    apply(
        s,
        vec![
            Change::PenaltyUndone(A),
            Change::CloseUndone(A),
            Change::OpenUndone(C),
        ],
    )
    .await
    .unwrap();
    let rows = table(s).await;
    assert_eq!(rows.iter().map(|c| c.open.scid).collect::<Vec<_>>(), [B, A]);
    assert_eq!((rows[1].close, rows[1].penalty), (None, None));
    apply(s, vec![Change::Opened(open(C, 2))]).await.unwrap();
    assert_eq!(table(s).await[0].open, open(C, 2));
}

#[tokio::test]
async fn records_applied_again_or_replayed_by_a_restart_change_nothing() {
    let run = vec![
        Change::Opened(open(A, 1)),
        Change::Opened(open(B, 1)),
        Change::Closed(open(A, 1), close(800_010, Kind::Force, 7)),
        Change::Penalty(
            A,
            Penalty {
                block: block(800_020, 0xd0),
                txid: Txid::from_byte_array([9; 32]),
            },
        ),
    ];
    let once = temp();
    apply(&once.store, run.clone()).await.unwrap();
    let want = table(&once.store).await;

    let twice = temp();
    for _ in 0..2 {
        apply(&twice.store, run.clone()).await.unwrap();
    }
    // A restart: the peer opens every channel still open and finds A's close again by its spend
    // records, which do not show the penalty. The stored penalty stays with the same close.
    apply(
        &twice.store,
        vec![
            Change::Opened(open(B, 1)),
            Change::Closed(open(A, 1), close(800_010, Kind::Force, 7)),
        ],
    )
    .await
    .unwrap();
    assert_eq!(table(&twice.store).await, want);

    // An open after a stored close means the close left the chain while we were away.
    apply(&twice.store, vec![Change::Opened(open(A, 1))])
        .await
        .unwrap();
    let a = table(&twice.store).await[1];
    assert_eq!((a.close, a.penalty), (None, None));
}

/// Rows the writer's connection has changed since it opened.
async fn written(s: &Store) -> i64 {
    s.write(|sql| {
        Ok(sql
            .prepare("SELECT total_changes()")?
            .query_row([], |r| r.get(0))?)
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn an_unchanged_open_is_not_written_again() {
    let t = temp();
    apply(&t.store, vec![Change::Opened(open(A, 1))])
        .await
        .unwrap();
    let before = written(&t.store).await;
    apply(&t.store, vec![Change::Opened(open(A, 1))])
        .await
        .unwrap();
    // The open matched the stored row, so nothing was written.
    assert_eq!(written(&t.store).await, before);
    apply(&t.store, vec![Change::Opened(open(A, 2))])
        .await
        .unwrap();
    assert_eq!(written(&t.store).await, before + 1);
}

fn snapshot(n: u64) -> Stats {
    Stats {
        channels: 100 * n,
        nodes: 50 * n,
        capacity: 1_000 * n,
        tor_nodes: 20 * n,
        clearnet_nodes: 10 * n,
        clearnet_tor_nodes: 5 * n,
        unannounced_nodes: 15 * n,
        med_capacity: 7 * n,
        policies: 150 * n,
        fee_rate: 3_000 * n,
        base_fee: 1_500 * n,
        med_fee_rate: n,
        med_base_fee: 2 * n,
    }
}

fn point(time: u64, n: u64) -> Point {
    Point {
        time,
        samples: 1,
        sums: snapshot(n),
    }
}

const NOW: u64 = 1_800_000_000 / DAY * DAY;

#[tokio::test]
async fn snapshots_roll_into_hours_and_days_without_losing_sums() {
    let t = temp();
    // Six snapshots of one hour ten days ago, and one an hour later.
    let old = NOW - 10 * DAY;
    let rows: Vec<Point> = (0..6).map(|i| point(old + i * EVERY, i + 1)).collect();
    insert(&t.store, Level::Minute, rows).await.unwrap();
    insert(&t.store, Level::Minute, vec![point(old + HOUR, 10)])
        .await
        .unwrap();
    let rolled = record(&t.store, point(NOW, 1), NOW).await.unwrap();
    assert_eq!(
        rolled,
        crate::history::Rolled {
            minutes: 7,
            hours: 0
        }
    );
    // Running it again changes nothing.
    let again = record(&t.store, point(NOW, 1), NOW).await.unwrap();
    assert_eq!(again, crate::history::Rolled::default());

    let s = series(&t.store, Interval::Month, NOW).await.unwrap();
    assert_eq!(s.step, HOUR);
    let first = s.points[0];
    assert_eq!((first.time, first.samples), (old, 6));
    // Sums of 1..=6, so means stay exact: 21 / 6 snapshots.
    assert_eq!(first.sums, snapshot(21));
    assert_eq!(first.mean(first.sums.channels), 350.0);
    assert_eq!(s.points[1].sums, snapshot(10));
    assert_eq!(s.points.last().unwrap().time, NOW);
    assert!(!s.complete());

    // Past 400 days hours become days, sums kept; a snapshot for a rolled-up hour is refused.
    let later = NOW + 400 * DAY;
    let rolled = record(&t.store, point(later, 1), later).await.unwrap();
    assert_eq!(rolled.hours, 2);
    assert_eq!(rolled.minutes, 1);
    let day = old / DAY * DAY;
    let all = series(&t.store, Interval::All, later).await.unwrap();
    assert_eq!(all.first, Some(day));
    assert_eq!(all.points[0].samples, 7);
    assert_eq!(all.points[0].sums, snapshot(31));
    record(&t.store, point(old + 2 * EVERY, 99), later)
        .await
        .unwrap();
    let again = series(&t.store, Interval::All, later).await.unwrap();
    assert_eq!(again.points[0].sums, snapshot(31));

    let newest = latest(&t.store).await.unwrap().unwrap();
    assert_eq!((newest.time, newest.sums), (later, snapshot(1)));
}

#[tokio::test]
async fn the_table_stays_bounded_over_the_years() {
    let t = temp();
    let years = 3;
    let start = NOW - years * 365 * DAY;
    // A snapshot an hour (every tenth would be the same roll-up, at six times the test's time),
    // written a year at a time and rolled as a running server would.
    for year in 0..years {
        let from = start + year * 365 * DAY;
        let rows: Vec<Point> = (0..365 * DAY / HOUR)
            .map(|i| point(from + i * HOUR, 1))
            .collect();
        insert(&t.store, Level::Minute, rows).await.unwrap();
        record(&t.store, point(from + 365 * DAY, 1), from + 365 * DAY)
            .await
            .unwrap();
    }
    let rows: i64 = t
        .store
        .read(|sql| {
            Ok(sql
                .prepare("SELECT count(*) FROM ln_statistics")?
                .query_row([], |r| r.get(0))?)
        })
        .await
        .unwrap();
    // 8 days of snapshots, 392 days of hours, and a row a day before that.
    let bound = KEEP_MINUTES / HOUR + (KEEP_HOURS - KEEP_MINUTES) / HOUR + (years * 365 - 400) + 2;
    assert!(rows as u64 <= bound, "{rows} rows");
    let all = series(&t.store, Interval::All, NOW).await.unwrap();
    assert!(all.points.len() <= 1_000);
    let total: u64 = all.points.iter().map(|p| p.samples).sum();
    // No snapshot lost or counted twice in the roll-ups. Each year's last write is the next
    // year's first snapshot, stored once.
    assert_eq!(total, years * 365 * DAY / HOUR + 1);
}

#[tokio::test]
async fn an_interrupted_write_leaves_nothing_behind() {
    let t = temp();
    let old = NOW - 10 * DAY;
    insert(&t.store, Level::Minute, vec![point(old, 1)])
        .await
        .unwrap();
    // Fail the roll-up's delete: the snapshot inserted before it in the same write goes too.
    t.store
        .write(|sql| {
            sql.prepare(
                "CREATE TEMP TRIGGER fail BEFORE DELETE ON ln_statistics BEGIN SELECT RAISE(ABORT, 'no'); END",
            )?
            .execute([])?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(record(&t.store, point(NOW, 5), NOW).await.is_err());
    assert_eq!(latest(&t.store).await.unwrap().unwrap().time, old);
    t.store
        .write(|sql| {
            sql.prepare("DROP TRIGGER temp.fail")?.execute([])?;
            Ok(())
        })
        .await
        .unwrap();
    record(&t.store, point(NOW, 5), NOW).await.unwrap();
    let s = series(&t.store, Interval::Month, NOW).await.unwrap();
    assert_eq!(s.points.iter().map(|p| p.samples).sum::<u64>(), 2);
}

#[test]
fn medians_take_the_lower_middle() {
    assert_eq!(median(&mut []), 0);
    assert_eq!(median(&mut [5]), 5);
    assert_eq!(median(&mut [9, 1, 5, 3]), 3);
    assert_eq!(median(&mut [9, 1, 5]), 5);
}

#[tokio::test(start_paused = true)]
async fn snapshots_every_ten_minutes_skipping_unsynced_late_and_busy_slots() {
    // A blocking task waiting keeps tokio from jumping virtual time while the store's own
    // thread writes.
    let (release, held) = std::sync::mpsc::channel::<()>();
    let hold = tokio::task::spawn_blocking(move || held.recv());
    let t = temp();
    let wall = Duration::from_secs(NOW + 30);
    let start = tokio::time::Instant::now();
    let jump = Arc::new(Mutex::new(Duration::ZERO));
    let clock = {
        let jump = Arc::clone(&jump);
        move || wall + start.elapsed() + *jump.lock().unwrap()
    };
    let synced = Arc::new(AtomicBool::new(false));
    let events = Arc::new(Mutex::new(Vec::new()));
    let config = Config {
        clock: Arc::new(clock.clone()),
        observer: {
            let events = Arc::clone(&events);
            Arc::new(move |e: &Event| events.lock().unwrap().push(e.clone()))
        },
    };
    let task = tokio::spawn(run(
        Arc::clone(&t.store),
        {
            let synced = Arc::clone(&synced);
            move || synced.load(Ordering::SeqCst).then(|| snapshot(1))
        },
        config,
    ));
    let next = |n: u64| async move {
        // Let the loop take its time and start sleeping before time moves.
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        tokio::time::advance(Duration::from_secs(EVERY) * u32::try_from(n).unwrap()).await;
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
    };
    let wait_for = |n: usize| {
        let events = Arc::clone(&events);
        async move {
            let waited = std::time::Instant::now();
            while events.lock().unwrap().len() < n {
                assert!(
                    waited.elapsed() < Duration::from_secs(20),
                    "no event {n}: {:?}",
                    events.lock().unwrap()
                );
                tokio::task::yield_now().await;
            }
        }
    };

    next(1).await;
    wait_for(1).await;
    synced.store(true, Ordering::SeqCst);
    next(1).await;
    wait_for(2).await;
    // The process stalls: the clock is two minutes past the slot when the loop wakes.
    *jump.lock().unwrap() = Duration::from_secs(120);
    next(1).await;
    wait_for(3).await;
    *jump.lock().unwrap() = Duration::ZERO;
    next(1).await;
    wait_for(4).await;

    let events = events.lock().unwrap().clone();
    let slot = (NOW + 30).div_ceil(EVERY) * EVERY;
    assert_eq!(
        events[0],
        Event::Skipped {
            time: slot,
            reason: Skip::Syncing
        }
    );
    assert!(matches!(events[1], Event::Written { time, .. } if time == slot + EVERY));
    assert!(matches!(
        events[2],
        Event::Skipped {
            reason: Skip::Late,
            ..
        }
    ));
    assert!(matches!(events[3], Event::Written { time, .. } if time == slot + 3 * EVERY));
    let stored = series(&t.store, Interval::Day, clock().as_secs())
        .await
        .unwrap();
    assert_eq!(
        stored.points.iter().map(|p| p.time).collect::<Vec<_>>(),
        [slot + EVERY, slot + 3 * EVERY]
    );
    task.abort();
    drop(release);
    let _ = hold.await;
}
