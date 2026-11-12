#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use bitcoin::hashes::Hash;

use super::db;
use super::*;
use crate::store::{Options, Store};

fn id(n: u8) -> Txid {
    Txid::from_byte_array([n; 32])
}

fn facts(n: u8, rbf: bool) -> Facts {
    Facts {
        fee: 100 * u64::from(n),
        vsize: 200,
        weight: 797,
        value: 5000 + u64::from(n),
        time: 1_700_000_000 + u32::from(n),
        rbf,
    }
}

/// `a` replaced by `b`, both signalling.
fn link(a: u8, b: u8) -> Link {
    Link {
        txid: id(a),
        by: id(b),
        seen: u64::from(a),
        tx: Facts {
            fee: 100,
            vsize: 200,
            ..facts(a, true)
        },
        by_tx: Some(Facts {
            fee: 300,
            ..facts(b, true)
        }),
        mined: Mined::No,
    }
}

fn block(n: u8) -> BlockHash {
    BlockHash::from_byte_array([n; 32])
}

fn mined(t: &Node) -> bool {
    matches!(t.mined, Mined::In(_))
}

fn txids(links: &[Link]) -> Vec<Txid> {
    links.iter().map(|l| l.txid).collect()
}

/// A tree as (txid, what it replaced), depth first.
fn shape(n: &Node) -> Vec<(Txid, Vec<Txid>)> {
    let mut out = vec![(n.txid, n.replaces.iter().map(|c| c.txid).collect())];
    for c in &n.replaces {
        out.extend(shape(c));
    }
    out
}

#[test]
fn chains_run_both_ways_and_stop_at_a_loop() {
    let mut r = Replacements::new(100);
    for (a, b) in [(1, 2), (2, 3), (3, 4), (9, 4)] {
        r.insert(link(a, b));
    }
    assert_eq!(txids(&r.chain(&id(1)).0), vec![id(1), id(2), id(3)]);
    assert_eq!(r.chain(&id(4)), (Vec::new(), false));
    let (back, more) = r.earlier(&id(4));
    assert!(!more);
    assert_eq!(back.len(), 4);
    assert_eq!(back[0].txid, id(3));
    // 1 came back (a miner took it) and replaced 4: what replaced 1 before no longer stands, so
    // every member has one newest end, 1, and there is one tree.
    r.insert(link(4, 1));
    assert_eq!(r.chain(&id(1)), (Vec::new(), false));
    assert_eq!(txids(&r.chain(&id(2)).0), vec![id(2), id(3), id(4)]);
    for t in [1, 2, 3, 4, 9] {
        assert_eq!(r.root(&id(t)), id(1), "{t}");
    }
    assert_eq!(r.recent(25, false).len(), 1);
    assert_eq!(
        link(1, 2).json()["rate"],
        json!(500),
        "100 sat over 200 vB is 0.5 sat/vB"
    );
    let mut unknown = link(1, 2);
    unknown.by_tx = None;
    assert_eq!(unknown.json()["by_rate"], Value::Null);
}

#[test]
fn a_replacement_undone_by_a_block_leaves_one_newest_end() {
    // B replaced A in our mempool, then a miner confirmed A, which replaced B.
    let mut r = Replacements::new(10);
    r.insert(link(1, 2));
    r.insert(link(2, 1));
    assert_eq!((r.root(&id(1)), r.root(&id(2))), (id(1), id(1)));
    let trees = r.recent(25, false);
    assert_eq!(trees.len(), 1);
    assert_eq!(
        shape(&trees[0]),
        vec![(id(1), vec![id(2)]), (id(2), vec![])]
    );
    // The same order read back from the store gives the same record.
    let mut again = Replacements::new(10);
    again.restore(link(1, 2));
    again.restore(link(2, 1));
    assert_eq!(again.tree(&id(2)), r.tree(&id(2)));
}

/// B replaced A in our mempool, then a miner confirmed A: the tracker no longer held A, so the
/// new link names A without figures, as it does a replacement straight from a block.
fn won_after_all() -> Link {
    Link {
        by_tx: None,
        mined: Mined::In(Some((7, block(7)))),
        ..link(2, 1)
    }
}

#[test]
fn the_original_confirmed_after_all_keeps_its_figures() {
    let mut r = Replacements::new(10);
    r.insert(link(1, 2));
    let recorded = r.insert(won_after_all());
    let a = link(1, 2).tx;
    assert_eq!(recorded.by_tx, Some(a), "what the store is given");
    let tree = r.tree(&id(2)).unwrap();
    assert_eq!(
        (tree.txid, tree.tx, tree.time),
        (id(1), Some(a), u64::from(a.time))
    );
    assert!(mined(&tree));
    assert_eq!(r.facts(&id(1)), Some(a));
    assert_eq!(r.chain(&id(2)).0[0].json()["by_fee"], a.fee);
    // A replacement that was never replaced itself still has none.
    let mut other = Replacements::new(10);
    other.insert(won_after_all());
    assert_eq!(other.tree(&id(2)).unwrap().tx, None);
}

/// A record put back from `store`, as at a start.
async fn reopen(store: &Store) -> Replacements {
    let mut r = Replacements::new(10);
    for l in db::load(store, 10).await.unwrap() {
        r.restore(l);
    }
    r
}

#[tokio::test]
async fn the_figures_kept_survive_a_restart_on_either_side_of_the_block() {
    let t = temp();
    let write = |l: Link| vec![Change::Link(l)];
    // Restarted between the replacement and the block: the record put back holds A's figures.
    let mut r = Replacements::new(10);
    db::write(&t.store, vec![write(r.insert(link(1, 2)))], 10)
        .await
        .unwrap();
    let mut r2 = reopen(&t.store).await;
    let recorded = r2.insert(won_after_all());
    db::write(&t.store, vec![write(recorded)], 10)
        .await
        .unwrap();
    let want = r2.tree(&id(2)).unwrap();
    assert_eq!(want.tx, Some(link(1, 2).tx));
    // And again after the block.
    assert_eq!(reopen(&t.store).await.tree(&id(2)), Some(want));
}

fn idn(n: u32) -> Txid {
    let mut b = [0u8; 32];
    b[..4].copy_from_slice(&n.to_le_bytes());
    Txid::from_byte_array(b)
}

#[test]
fn a_history_longer_than_any_answer_still_has_one_newest_end() {
    let mut r = Replacements::new(10_000);
    for i in 0..1025 {
        r.insert(Link {
            txid: idn(i),
            by: idn(i + 1),
            ..link(1, 2)
        });
    }
    assert_eq!(r.root(&idn(0)), idn(1025));
    let trees = r.recent(25, false);
    assert_eq!(trees.len(), 1, "one history, one tree");
    assert_eq!(trees[0].txid, idn(1025));
}

#[test]
fn a_transaction_back_in_the_mempool_is_no_longer_replaced_or_mined() {
    let mut r = Replacements::new(10);
    r.insert(link(1, 2));
    r.insert(link(3, 4));
    assert!(r.confirmed(&id(4), 7, block(7)));
    // 1 was rebroadcast after 2 left: nothing stands replaced by 2 any more.
    assert!(r.returned(&id(1)));
    assert!(r.get(&id(1)).is_none() && r.tree(&id(2)).is_none());
    // 4 came back from a block a reorg undid: its tree stands, unmined.
    let v = r.version();
    assert!(r.returned(&id(4)));
    assert!(r.version() > v);
    assert!(!mined(&r.tree(&id(3)).unwrap()));
    assert!(
        !r.returned(&id(9)),
        "an unknown transaction changes nothing"
    );
}

#[test]
fn long_chains_are_cut_and_marked() {
    let mut r = Replacements::new(1000);
    for i in 0..150 {
        r.insert(link(i, i + 1));
    }
    let (chain, more) = r.chain(&id(0));
    assert_eq!((chain.len(), more), (MAX_LINKS, true));
    let (back, more) = r.earlier(&id(150));
    assert_eq!((back.len(), more), (MAX_LINKS, true));
    // The tree is cut at its bound, newest first.
    let tree = r.tree(&id(0)).unwrap();
    assert_eq!(tree.txid, id(150));
    assert_eq!(shape(&tree).len(), MAX_TREE);
}

#[test]
fn the_oldest_fall_out_first_and_a_rewrite_counts_once() {
    let mut r = Replacements::new(3);
    for (a, b) in [(1, 2), (3, 4), (5, 6)] {
        r.insert(link(a, b));
    }
    r.insert(link(7, 8));
    assert_eq!((r.len(), r.get(&id(1))), (3, None));
    assert!(
        r.earlier(&id(2)).0.is_empty(),
        "the reverse index forgets it too"
    );
    assert!(r.tree(&id(2)).is_none() && r.facts(&id(1)).is_none());
    // 3 is replaced again by 9: its old entry in the order no longer evicts the new one.
    r.insert(link(3, 9));
    r.insert(link(10, 11));
    assert_eq!(r.get(&id(3)).map(|l| l.by), Some(id(9)));
    assert!(r.earlier(&id(4)).0.is_empty());
    assert_eq!(r.len(), 3);
    // Many rewrites of one link never grow the record past its cap.
    for _ in 0..50 {
        r.insert(link(3, 9));
    }
    assert!(r.order.len() <= 3 && r.len() <= 3);
    assert_eq!(r.take_recorded(), 56);
    assert_eq!(r.take_recorded(), 0);
}

#[test]
fn a_chain_a_then_b_then_c_is_one_tree_from_any_member() {
    let mut r = Replacements::new(100);
    r.insert(link(1, 2));
    r.insert(link(2, 3));
    for t in [1, 2, 3] {
        let tree = r.tree(&id(t)).unwrap();
        assert_eq!(
            shape(&tree),
            vec![(id(3), vec![id(2)]), (id(2), vec![id(1)]), (id(1), vec![])]
        );
    }
    let tree = r.tree(&id(1)).unwrap();
    // The newest replacement's figures come from the link that names it; the others' their own.
    assert_eq!(tree.tx.map(|f| f.fee), Some(300));
    assert_eq!(tree.time, u64::from(facts(3, true).time));
    assert_eq!(tree.replaces[0].tx.map(|f| f.fee), Some(100));
    assert!(!tree.full_rbf() && !mined(&tree));
    assert_eq!(r.replaced_by(&id(3)), &[id(2)]);
    assert_eq!(r.facts(&id(3)).map(|f| f.fee), Some(300));
    assert_eq!(r.facts(&id(1)).map(|f| f.time), Some(facts(1, true).time));
    // Mined: the tree says so, and only the newest replacement's confirmation counts.
    assert!(!r.confirmed(&id(1), 7, block(7)));
    let v = r.version();
    assert!(r.confirmed(&id(3), 7, block(7)));
    assert!(r.version() > v);
    assert!(mined(&r.tree(&id(1)).unwrap()));
    assert_eq!(r.recent(25, false).len(), 1);
    assert!(r.recent(25, true).is_empty());
}

#[test]
fn one_replacement_that_removes_several_and_a_package_replacement() {
    let mut r = Replacements::new(100);
    // 5 spends inputs of 1 and 2; 3 is a child of 1 that left with it.
    for a in [1, 2, 3] {
        r.insert(link(a, 5));
    }
    let tree = r.tree(&id(3)).unwrap();
    assert_eq!(
        shape(&tree)[0],
        (id(5), vec![id(1), id(2), id(3)]),
        "every transaction it removed, in the order recorded"
    );
    // A package: parent 7 and child 8 entered together; 8 spends 6's input, so 6 is replaced
    // by 8, the transaction that conflicts. The tree hangs from 8.
    r.insert(link(6, 8));
    assert_eq!(shape(&r.tree(&id(6)).unwrap())[0], (id(8), vec![id(6)]));
    // One without signalling makes its tree full RBF, and only that tree is listed as such.
    let mut quiet = link(9, 10);
    quiet.tx.rbf = false;
    r.insert(quiet);
    let full: Vec<Txid> = r.recent(25, true).iter().map(|t| t.txid).collect();
    assert_eq!(full, vec![id(10)]);
    let all: Vec<Txid> = r.recent(25, false).iter().map(|t| t.txid).collect();
    assert_eq!(all, vec![id(10), id(8), id(5)], "newest first");
    assert_eq!(r.recent(2, false).len(), 2);
}

#[test]
fn a_replacement_the_tracker_never_held_has_no_figures() {
    let mut r = Replacements::new(10);
    let mut l = link(1, 2);
    l.by_tx = None;
    l.seen = 42;
    r.insert(l);
    let tree = r.tree(&id(1)).unwrap();
    assert_eq!((tree.tx, tree.time), (None, 42));
    assert_eq!(r.facts(&id(2)), None);
}

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
        "rbitcoin-space-replacements-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let store = Store::open(&dir, "regtest", Options::default()).unwrap();
    Temp { dir, store }
}

#[tokio::test]
async fn the_stored_record_survives_a_restart_and_keeps_its_bound() {
    let t = temp();
    let mut quiet = link(1, 2);
    quiet.tx.rbf = false;
    let mut unknown = link(2, 3);
    unknown.by_tx = None;
    let first = vec![Change::Link(quiet), Change::Link(unknown)];
    db::write(&t.store, vec![first], 3).await.unwrap();
    db::write(
        &t.store,
        vec![vec![
            Change::Link(link(4, 5)),
            Change::Mined(id(3), Mined::In(Some((9, block(9))))),
        ]],
        3,
    )
    .await
    .unwrap();
    // Reopened, the record is what it was, mined flag included.
    let dir = t.dir.clone();
    let Temp { store, .. } = &t;
    let loaded = db::load(store, 3).await.unwrap();
    let mut want_unknown = unknown;
    want_unknown.mined = Mined::In(Some((9, block(9))));
    assert_eq!(loaded, vec![quiet, want_unknown, link(4, 5)]);
    let again = Store::open(&dir, "regtest", Options::default()).unwrap();
    assert_eq!(db::load(&again, 3).await.unwrap(), loaded);
    let mut r = Replacements::new(3);
    for l in loaded {
        r.restore(l);
    }
    assert_eq!(r.take_recorded(), 0, "restored links are not new");
    assert!(mined(&r.tree(&id(1)).unwrap()) && r.tree(&id(1)).unwrap().full_rbf());

    // The bound: only the newest three stay, a rewrite moves a link to the newest.
    db::write(
        &again,
        vec![vec![Change::Link(link(6, 7)), Change::Link(link(1, 9))]],
        3,
    )
    .await
    .unwrap();
    let kept: Vec<Txid> = txids(&db::load(&again, 100).await.unwrap());
    assert_eq!(kept, vec![id(4), id(6), id(1)]);
    // Batches apply in order: a link recorded then undone by a return is gone, and one recorded
    // after a return stays.
    db::write(
        &again,
        vec![
            vec![Change::Link(link(30, 31))],
            vec![Change::Returned(id(30)), Change::Returned(id(6))],
            vec![Change::Link(link(6, 32))],
        ],
        3,
    )
    .await
    .unwrap();
    let kept: Vec<Txid> = txids(&db::load(&again, 100).await.unwrap());
    // The bound counts places in the order, and the deleted link had one: never more than three.
    assert_eq!(kept, vec![id(1), id(6)]);
    // B replaced A, a miner confirmed A (A replaced B), and a reorg returned B: the store, read
    // back, holds what memory holds.
    let mut r = Replacements::new(10);
    r.insert(link(40, 41));
    db::write(&again, vec![r.take_changes()], 10).await.unwrap();
    r.insert(link(41, 40));
    db::write(&again, vec![r.take_changes()], 10).await.unwrap();
    r.returned(&id(41));
    db::write(&again, vec![r.take_changes()], 10).await.unwrap();
    let stored = db::load(&again, 100).await.unwrap();
    assert!(r.get(&id(40)).is_none() && r.get(&id(41)).is_none());
    assert!(
        stored.iter().all(|l| ![id(40), id(41)].contains(&l.txid)),
        "{stored:?}"
    );
    // A write that fails part way leaves nothing behind.
    let half = vec![Change::Link(link(20, 21))];
    let failed = again
        .write(move |sql| {
            db::insert(sql, &half)?;
            Err::<(), _>(crate::store::Error::Closed)
        })
        .await;
    assert!(failed.is_err());
    assert_eq!(txids(&db::load(&again, 100).await.unwrap()), kept);
}

/// The table at its default bound: its size on disk, and how long putting it back takes.
/// `cargo test --release -p engine -- --ignored --nocapture replacements_benchmark`
#[tokio::test]
#[ignore = "a measurement, not a check"]
#[allow(clippy::print_stdout)]
async fn replacements_benchmark() {
    let t = temp();
    let keep = 10_000;
    let mut n = 0u32;
    // Twice the bound, in writes of 20 links, so the bound has pruned half.
    for _ in 0..(2 * keep / 20) {
        let links = (0..20)
            .map(|_| {
                n += 1;
                let mut l = link(1, 2);
                let mut a = [0u8; 32];
                a[..4].copy_from_slice(&n.to_le_bytes());
                l.txid = Txid::from_byte_array(a);
                a[4] = 1;
                l.by = Txid::from_byte_array(a);
                Change::Link(l)
            })
            .collect();
        db::write(
            &t.store,
            vec![links],
            keep,
        )
        .await
        .unwrap();
    }
    let started = std::time::Instant::now();
    let loaded = db::load(&t.store, keep).await.unwrap();
    let took = started.elapsed();
    let mut r = Replacements::new(keep);
    for l in loaded.iter().copied() {
        r.restore(l);
    }
    let rows: i64 = t
        .store
        .read(|sql| {
            Ok(sql
                .prepare("SELECT count(*) FROM replacements")?
                .query_row([], |r| r.get(0))?)
        })
        .await
        .unwrap();
    let bytes: i64 = t
        .store
        .read(|sql| {
            Ok(sql
                .prepare("SELECT sum(pgsize) FROM dbstat WHERE name LIKE '%replacements%'")?
                .query_row([], |r| r.get(0))?)
        })
        .await
        .unwrap_or(-1);
    println!(
        "rows {rows}, loaded {} in {took:?}, table and indexes {bytes} bytes ({} a row), record {}",
        loaded.len(),
        bytes / rows.max(1),
        r.len()
    );
}
