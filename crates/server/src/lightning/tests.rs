//! The history consumer against a scripted peer: the same records, arriving live, as a backlog,
//! past a lagging receiver, across a restart or past a gap, leave the same table.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;

use bitcoin::hashes::Hash;
use bitcoin::{BlockHash, Txid};
use engine::store::Options;
use ln::{BlockRef, Closed, NodeId, Opened};

use super::*;

struct Temp {
    dir: PathBuf,
    store: Store,
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn temp() -> Temp {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "rbitcoin-space-ln-consumer-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let store = Store::open(&dir, "regtest", Options::default()).unwrap();
    Temp { dir, store }
}

fn lightning() -> Lightning {
    let config = LightningConfig {
        peers: Vec::new(),
        max_peers: 1,
        max_channels: 10,
    };
    Lightning::new(&config, &std::env::temp_dir(), Network::Regtest)
}

/// A peer that sends what the test scripts: it numbers records, keeps the newest `keep` for
/// `recent`, streams them with room for `room`, and tracks the open set as the real one does.
struct Scripted {
    tx: Mutex<Option<broadcast::Sender<Record>>>,
    kept: Mutex<VecDeque<Record>>,
    keep: usize,
    open: Mutex<HashMap<u64, Opened>>,
    seq: Mutex<u64>,
}

impl Scripted {
    fn new(keep: usize, room: usize) -> (Self, broadcast::Receiver<Record>) {
        let (tx, rx) = broadcast::channel(room);
        let s = Self {
            tx: Mutex::new(Some(tx)),
            kept: Mutex::new(VecDeque::new()),
            keep,
            open: Mutex::new(HashMap::new()),
            seq: Mutex::new(0),
        };
        (s, rx)
    }

    fn send(&self, change: &History) {
        let mut seq = self.seq.lock().unwrap();
        *seq += 1;
        let mut open = self.open.lock().unwrap();
        match change {
            History::Opened(o) => {
                open.insert(o.scid, o.clone());
            }
            History::Closed(c) => {
                open.remove(&c.scid);
            }
            History::OpenUndone { scid } => {
                open.remove(scid);
            }
            History::CloseUndone { scid } => {
                if let Some(o) = opens().into_iter().find(|o| o.scid == *scid) {
                    open.insert(*scid, o);
                }
            }
            _ => {}
        }
        let r = Record {
            seq: *seq,
            change: change.clone(),
        };
        let mut kept = self.kept.lock().unwrap();
        if kept.len() == self.keep {
            kept.pop_front();
        }
        kept.push_back(r.clone());
        if let Some(tx) = &*self.tx.lock().unwrap() {
            let _ = tx.send(r);
        }
    }

    /// The peer stopped: the stream closes.
    fn close(&self) {
        self.tx.lock().unwrap().take();
    }
}

impl HistorySource for Scripted {
    fn recent(&self, after: u64) -> Vec<Record> {
        let kept = self.kept.lock().unwrap();
        kept.iter().filter(|r| r.seq > after).cloned().collect()
    }

    fn open(&self) -> (u64, Vec<Opened>) {
        let seq = *self.seq.lock().unwrap();
        (seq, self.open.lock().unwrap().values().cloned().collect())
    }
}

fn at(height: u32, tag: u8) -> BlockRef {
    BlockRef {
        height,
        hash: BlockHash::from_byte_array([tag; 32]),
        time: 1_700_000_000 + u64::from(height),
    }
}

fn id(n: u8) -> NodeId {
    // Any 33 bytes make a NodeId; the table stores them as they are.
    NodeId::from_slice(&[n; 33]).unwrap()
}

const A: u64 = 800_000 << 40 | 5 << 16;
const B: u64 = 800_001 << 40 | 1 << 16 | 1;
const C: u64 = 800_002 << 40;
const D: u64 = 800_003 << 40;

fn opened(scid: u64, tag: u8) -> Opened {
    Opened {
        scid,
        node_one: id(2),
        node_two: id(3),
        capacity_sats: 1_000 + scid % 1_000,
        block: at(u32::try_from(scid >> 40).unwrap(), tag),
    }
}

fn opens() -> Vec<Opened> {
    vec![opened(A, 1), opened(B, 1), opened(C, 1), opened(D, 1)]
}

fn closed(o: &Opened, height: u32, kind: CloseKind, tx: u8) -> History {
    History::Closed(Closed {
        scid: o.scid,
        node_one: o.node_one,
        node_two: o.node_two,
        capacity_sats: o.capacity_sats,
        opened: o.block,
        closed: at(height, 0xc0),
        txid: Txid::from_byte_array([tx; 32]),
        kind,
        fee_sats: Some(200),
    })
}

/// Opens, closes of every kind, a penalty, and each kind of undoing, in the order a reorg sends
/// them.
fn script() -> Vec<History> {
    let o = opens();
    vec![
        History::Opened(o[0].clone()),
        History::Opened(o[1].clone()),
        History::Opened(o[2].clone()),
        closed(&o[0], 800_010, CloseKind::Force, 7),
        History::Penalty {
            scid: A,
            txid: Txid::from_byte_array([9; 32]),
            block: at(800_012, 0xd0),
        },
        closed(&o[1], 800_011, CloseKind::Cooperative, 8),
        History::Opened(o[3].clone()),
        // A reorg back to 800,001: the sweep, B's close and C's open leave the chain.
        History::PenaltyUndone { scid: A },
        History::CloseUndone { scid: B },
        History::OpenUndone { scid: C },
        // The new branch: C again in another block, B closes otherwise, D closes as a splice.
        History::Opened(opened(C, 2)),
        closed(&o[1], 800_013, CloseKind::Other, 10),
        closed(&o[3], 800_013, CloseKind::Force, 11),
    ]
}

/// The table after applying the script directly, in one write.
async fn want() -> Vec<store_ln::Channel> {
    let t = temp();
    store_ln::apply(&t.store, script().iter().map(change).collect())
        .await
        .unwrap();
    rows(&t.store).await
}

async fn rows(store: &Store) -> Vec<store_ln::Channel> {
    store_ln::opened(store, 1_000).await.unwrap()
}

/// The consumer from a fresh start until the stream closes.
async fn run(src: &Scripted, mut rx: broadcast::Receiver<Record>, store: &Store, l: &Lightning) {
    consume(src, &mut rx, &mut Pending::default(), store, l, "regtest").await;
}

#[tokio::test]
async fn records_sent_live_one_by_one() {
    let t = temp();
    let (peer, rx) = Scripted::new(100, 100);
    let l = lightning();
    tokio::join!(run(&peer, rx, &t.store, &l), async {
        for h in script() {
            peer.send(&h);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        peer.close();
    });
    assert_eq!(rows(&t.store).await, want().await);
    assert_eq!(l.seq.load(Ordering::Relaxed), 13);
    assert_eq!(l.records.load(Ordering::Relaxed), 13);
}

#[tokio::test]
async fn records_kept_before_the_consumer_started_are_stored_once() {
    let t = temp();
    // Everything is both in the peer's memory and on the stream: each is taken once.
    let (peer, rx) = Scripted::new(100, 100);
    for h in script() {
        peer.send(&h);
    }
    peer.close();
    let l = lightning();
    run(&peer, rx, &t.store, &l).await;
    assert_eq!(rows(&t.store).await, want().await);
    assert_eq!(l.records.load(Ordering::Relaxed), 13);
}

#[tokio::test]
async fn a_lagging_receiver_reads_the_peer_s_memory() {
    let t = temp();
    // The stream holds two; the peer keeps everything.
    let (peer, rx) = Scripted::new(100, 2);
    let l = lightning();
    let first = script().len() / 2;
    tokio::join!(run(&peer, rx, &t.store, &l), async {
        // Half while the consumer runs, then a burst that overruns the stream.
        for h in &script()[..first] {
            peer.send(h);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        for h in &script()[first..] {
            peer.send(h);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        peer.close();
    });
    assert_eq!(rows(&t.store).await, want().await);
    assert_eq!(l.seq.load(Ordering::Relaxed), 13);
}

#[tokio::test]
async fn a_restart_in_the_middle_resumes_without_gaps_or_duplicates() {
    let t = temp();
    let split = 6;
    let (before, rx) = Scripted::new(100, 100);
    for h in &script()[..split] {
        before.send(h);
    }
    before.close();
    run(&before, rx, &t.store, &lightning()).await;

    // The new process numbers from 1 again and first opens every channel still open (its graph
    // file replayed), then goes on.
    let (after, rx) = Scripted::new(100, 100);
    let mut still_open: Vec<Opened> = before.open().1;
    still_open.sort_by_key(|o| o.scid);
    for o in still_open {
        after.send(&History::Opened(o));
    }
    for h in &script()[split..] {
        after.send(h);
    }
    after.close();
    run(&after, rx, &t.store, &lightning()).await;
    assert_eq!(rows(&t.store).await, want().await);
}

#[tokio::test]
async fn a_gap_past_the_peer_s_memory_stores_the_open_set_and_goes_on() {
    let t = temp();
    // The peer keeps two records and the stream two: four opens leave the first two unseen.
    let (peer, rx) = Scripted::new(2, 2);
    for o in opens() {
        peer.send(&History::Opened(o));
    }
    let l = lightning();
    let o = opens();
    tokio::join!(run(&peer, rx, &t.store, &l), async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        peer.send(&closed(&o[0], 800_010, CloseKind::Cooperative, 7));
        tokio::time::sleep(Duration::from_millis(20)).await;
        peer.close();
    });
    let want = {
        let w = temp();
        let mut changes: Vec<Change> = o.iter().map(|o| Change::Opened(open(o))).collect();
        changes.push(change(&closed(&o[0], 800_010, CloseKind::Cooperative, 7)));
        store_ln::apply(&w.store, changes).await.unwrap();
        rows(&w.store).await
    };
    assert_eq!(rows(&t.store).await, want);
    assert_eq!(l.seq.load(Ordering::Relaxed), 5);
}

#[tokio::test]
async fn a_backlog_is_stored_in_bounded_writes_and_kept_until_committed() {
    let t = temp();
    let l = lightning();
    let mut p = Pending::default();
    let records: Vec<Record> = (1..=5_000u64)
        .map(|seq| Record {
            seq,
            change: History::Opened(opened(seq << 40, 1)),
        })
        .collect();
    let (peer, _rx) = Scripted::new(0, 1);
    p.extend(&peer, records, "regtest");
    assert_eq!(p.changes.len(), 5_000);
    p.store_front(&t.store, &l).await.unwrap();
    // One write took the first 2,000 and no more.
    assert_eq!(p.changes.len(), 5_000 - BATCH);
    assert_eq!(l.seq.load(Ordering::Relaxed), BATCH as u64);
    assert_eq!(
        store_ln::opened(&t.store, 5_000).await.unwrap().len(),
        BATCH
    );
    // A write that fails leaves its records pending.
    t.store
        .write(|sql| {
            sql.prepare(
                "CREATE TEMP TRIGGER fail BEFORE INSERT ON ln_channels BEGIN SELECT RAISE(ABORT, 'no'); END",
            )?
            .execute([])?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(p.store_front(&t.store, &l).await.is_err());
    assert_eq!(p.changes.len(), 5_000 - BATCH);
}

#[tokio::test]
async fn at_shutdown_what_is_pending_and_still_on_the_stream_is_stored() {
    let t = temp();
    let l = lightning();
    let (peer, mut rx) = Scripted::new(100, 100);
    let script = script();
    let (first, rest) = script.split_at(6);
    for h in first {
        peer.send(h);
    }
    // The consumer took the first records but was stopped before storing them (as when
    // shutdown wins while a write waits for a full queue).
    let mut p = Pending::default();
    while let Ok(r) = rx.try_recv() {
        p.extend(&peer, vec![r], "regtest");
    }
    // The peer sent the rest before it stopped.
    for h in rest {
        peer.send(h);
    }
    peer.close();
    let deadline = Instant::now() + Duration::from_secs(5);
    flush(&mut rx, &mut p, &t.store, &l, "regtest", deadline).await;
    assert!(p.changes.is_empty());
    assert_eq!(rows(&t.store).await, want().await);
    assert_eq!(l.seq.load(Ordering::Relaxed), 13);
}

#[tokio::test]
async fn the_shutdown_flush_gives_up_at_its_deadline_behind_a_busy_writer() {
    let t = temp();
    let l = lightning();
    let (peer, mut rx) = Scripted::new(100, 100);
    for o in opens() {
        peer.send(&History::Opened(o));
    }
    peer.close();
    // Another write holds the writer for a second.
    let busy = t.store.write(|_| {
        std::thread::sleep(Duration::from_secs(1));
        Ok(())
    });
    let mut p = Pending::default();
    let started = Instant::now();
    let flushed = async {
        flush(
            &mut rx,
            &mut p,
            &t.store,
            &l,
            "regtest",
            started + Duration::from_millis(200),
        )
        .await;
        started.elapsed()
    };
    // The busy write is queued first: polled first.
    let (done, took) = tokio::join!(busy, flushed);
    done.unwrap();
    assert!(took < Duration::from_millis(800), "{took:?}");
    // It stopped waiting at the deadline, with the records still pending, not stored.
    assert_eq!(p.changes.len(), 4);
    assert_eq!(l.records.load(Ordering::Relaxed), 0);
}

#[test]
fn error_texts_are_cut_to_one_short_line() {
    let page = "esplora unavailable: 502 Bad Gateway: <html>\r\n<head><title>502</title></head>";
    assert_eq!(short(page), "esplora unavailable: 502 Bad Gateway: <html>");
    let long = "é".repeat(100);
    assert_eq!(short(&long).len(), 120);
    assert_eq!(short(""), "");
}

#[test]
fn short_channel_ids_print_as_lightning_writes_them() {
    assert_eq!(scid_text(A), "800000x5x0");
    assert_eq!(scid_text(B), "800001x1x1");
}

#[test]
fn list_limits_are_strict() {
    let uri = |q: &str| q.parse::<Uri>().unwrap();
    assert_eq!(limit(&uri("/x")), Ok(DEFAULT_LIST));
    assert_eq!(limit(&uri("/x?limit=100")), Ok(100));
    assert_eq!(limit(&uri("/x?limit=1")), Ok(1));
    for bad in [
        "/x?limit=0",
        "/x?limit=101",
        "/x?limit=-1",
        "/x?limit=1e2",
        "/x?limit=",
        "/x?limit=0001",
        "/x?limit=5&limit=6",
        "/x?offset=5",
        "/x?limit=5&x=1",
    ] {
        assert!(limit(&uri(bad)).is_err(), "{bad}");
    }
}

#[tokio::test]
async fn records_sent_while_the_node_was_on_another_chain_are_never_stored() {
    let t = temp();
    let dir = t.dir.join("gated");
    let store = Store::open(
        &dir,
        "regtest",
        Options {
            genesis: Some([1; 32]),
            ..Options::default()
        },
    )
    .unwrap();
    store.set_gate(engine::store::Gate::Open);
    // Room for two on the stream, so what arrives during the hold overflows it.
    let (peer, rx) = Scripted::new(100, 2);
    let l = lightning();
    let wait = || tokio::time::sleep(Duration::from_millis(50));
    tokio::join!(run(&peer, rx, &store, &l), async {
        peer.send(&History::Opened(opened(A, 1)));
        wait().await;
        // Found on another chain: the consumer waits on its next write while more arrive.
        store.set_gate(engine::store::Gate::Held);
        peer.send(&History::Opened(opened(B, 1)));
        wait().await;
        for scid in [C, B + 1, C + 1, B + 2, C + 2] {
            peer.send(&History::Opened(opened(scid, 1)));
        }
        wait().await;
        store.set_gate(engine::store::Gate::Open);
        wait().await;
        peer.send(&History::Opened(opened(D, 1)));
        wait().await;
        peer.close();
    });
    let mut scids: Vec<u64> = rows(&store).await.iter().map(|c| c.open.scid).collect();
    scids.sort_unstable();
    assert_eq!(scids, [A, D]);
}

/// [`Scripted`], whose memory, read once during a hold, sends one more record before the gate
/// reopens: the race between the consumer's discard and the end of the hold.
struct ReopenOnRead<'a> {
    peer: &'a Scripted,
    store: &'a Store,
    late: Mutex<Option<History>>,
}

impl HistorySource for ReopenOnRead<'_> {
    fn recent(&self, after: u64) -> Vec<Record> {
        let kept = self.peer.recent(after);
        if self.store.gate() == engine::store::Gate::Held
            && let Some(h) = self.late.lock().unwrap().take()
        {
            self.peer.send(&h);
            self.store.set_gate(engine::store::Gate::Open);
        }
        kept
    }

    fn open(&self) -> (u64, Vec<Opened>) {
        self.peer.open()
    }
}

#[tokio::test]
async fn a_record_sent_as_the_hold_ends_is_never_stored() {
    let t = temp();
    let store = Arc::new(
        Store::open(
            &t.dir.join("gated"),
            "regtest",
            Options {
                genesis: Some([1; 32]),
                ..Options::default()
            },
        )
        .unwrap(),
    );
    store.set_gate(engine::store::Gate::Open);
    let (peer, mut rx) = Scripted::new(100, 100);
    let src = ReopenOnRead {
        peer: &peer,
        store: &store,
        late: Mutex::new(Some(History::Opened(opened(C, 1)))),
    };
    let l = lightning();
    let wait = || tokio::time::sleep(Duration::from_millis(50));
    let mut p = Pending::default();
    // Each step waits for the one before; one that never comes fails the test, not hangs it.
    let scenario = async {
        tokio::join!(
            consume(&src, &mut rx, &mut p, &store, &l, "regtest"),
            async {
                peer.send(&History::Opened(opened(A, 1)));
                while rows(&store).await.is_empty() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                // The writer is busy, so the next write passes the gate and queues; the node is
                // found on another chain before the writer reaches it, which rejects it while
                // still held.
                let (go, stop) = std::sync::mpsc::channel::<()>();
                let (running, started) = std::sync::mpsc::channel::<()>();
                let busy = tokio::spawn({
                    let s = Arc::clone(&store);
                    async move {
                        s.write(move |_| {
                            running.send(()).ok();
                            stop.recv().ok();
                            Ok(())
                        })
                        .await
                    }
                });
                while started.try_recv().is_err() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                peer.send(&History::Opened(opened(B, 1)));
                while store.queue_depth() == 0 {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                store.set_gate(engine::store::Gate::Held);
                go.send(()).unwrap();
                busy.await.unwrap().unwrap();
                wait().await;
                peer.send(&History::Opened(opened(D, 1)));
                wait().await;
                peer.close();
            }
        )
    };
    tokio::time::timeout(Duration::from_secs(10), scenario)
        .await
        .expect("a step of the scenario never came");
    let mut scids: Vec<u64> = rows(&store).await.iter().map(|c| c.open.scid).collect();
    scids.sort_unstable();
    assert_eq!(scids, [A, D]);
}

#[tokio::test]
async fn the_totals_at_the_first_sync_stand_in_until_a_newer_snapshot_is_stored() {
    let t = temp();
    let l = lightning();
    let point = |time, channels| Point {
        time,
        samples: 1,
        sums: Stats {
            channels,
            ..Stats::default()
        },
    };
    // Not synced yet: nothing, so the routes say there are no statistics.
    assert_eq!(newest(&l, &t.store).await.unwrap(), None);
    // Synced, before its first ten minutes: the live totals, not a 404 or nulls.
    *l.live.lock().unwrap() = Some(point(1_000_100, 1_065));
    assert_eq!(
        newest(&l, &t.store).await.unwrap(),
        Some(point(1_000_100, 1_065))
    );
    // A snapshot from before this start (a restart after downtime) is older: still the live ones.
    store_ln::insert(&t.store, store_ln::Level::Minute, vec![point(999_600, 900)])
        .await
        .unwrap();
    assert_eq!(
        newest(&l, &t.store).await.unwrap(),
        Some(point(1_000_100, 1_065))
    );
    // The first snapshot since: the stored one.
    store_ln::insert(
        &t.store,
        store_ln::Level::Minute,
        vec![point(1_000_200, 1_066)],
    )
    .await
    .unwrap();
    assert_eq!(
        newest(&l, &t.store).await.unwrap(),
        Some(point(1_000_200, 1_066))
    );
}
