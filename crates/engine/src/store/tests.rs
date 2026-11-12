#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc as std_mpsc};
use std::time::{Duration, Instant};

use super::migrate::{Migration, MigrationError, check_checksums, validate};
use super::*;

/// A directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("engine-store-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const M1: Migration = (
    "0001_items.sql",
    "CREATE TABLE items (id INTEGER PRIMARY KEY, label TEXT NOT NULL);",
);
const M2: Migration = (
    "0002_items_size.sql",
    "ALTER TABLE items ADD COLUMN size INTEGER;",
);

fn open(dir: &Path, migrations: &[Migration]) -> Result<Store, OpenError> {
    Store::open_with(dir, "regtest", Options::default(), migrations)
}

async fn version(store: &Store) -> i64 {
    store
        .read(|sql| {
            Ok(sql
                .prepare("PRAGMA user_version")?
                .query_row([], |r| r.get(0))?)
        })
        .await
        .unwrap()
}

async fn count(store: &Store) -> i64 {
    store
        .read(|sql| {
            Ok(sql
                .prepare("SELECT count(*) FROM items")?
                .query_row([], |r| r.get(0))?)
        })
        .await
        .unwrap()
}

async fn insert(store: &Store, id: i64) {
    store
        .write(move |sql| {
            sql.prepare("INSERT INTO items (id, label) VALUES (?1, 'x')")?
                .execute(params![id])?;
            Ok(())
        })
        .await
        .unwrap();
}

#[test]
fn validate_accepts_a_sequence_and_rejects_mistakes() {
    assert_eq!(validate(&[]), Ok(()));
    assert_eq!(validate(&[M1, M2]), Ok(()));
    assert_eq!(
        validate(&[M1, ("0003_c.sql", "")]),
        Err(MigrationError::Gap {
            expected: 2,
            file: "0003_c.sql".into()
        })
    );
    assert_eq!(
        validate(&[M2]),
        Err(MigrationError::Gap {
            expected: 1,
            file: M2.0.into()
        })
    );
    assert_eq!(
        validate(&[M1, ("0001_again.sql", "")]),
        Err(MigrationError::Duplicate("0001_again.sql".into()))
    );
    for bad in [
        "1_a.sql",
        "0001_A.sql",
        "0001_.sql",
        "0001-a.sql",
        "0001_a.SQL",
        "00001_a.sql",
    ] {
        assert_eq!(
            validate(&[(bad, "")]),
            Err(MigrationError::BadName(bad.into())),
            "{bad}"
        );
    }
}

#[test]
fn checksums_catch_edited_missing_and_removed_migrations() {
    let lock = "# header\n0001_items.sql ad2b2d0c9e6d1e4c\n";
    let real = check_checksums(&[M1], "").unwrap_err();
    let line = real.lines().last().unwrap().to_owned();
    assert!(line.starts_with("0001_items.sql "), "{real}");
    assert_eq!(check_checksums(&[M1], &line), Ok(()));
    assert!(
        check_checksums(&[M1], lock)
            .unwrap_err()
            .contains("changed after it shipped")
    );
    assert!(check_checksums(&[], &line).unwrap_err().contains("gone"));
    let edited = (
        "0001_items.sql",
        "CREATE TABLE items (id INTEGER PRIMARY KEY);",
    );
    assert!(
        check_checksums(&[edited], &line)
            .unwrap_err()
            .contains("changed after it shipped")
    );
}

#[test]
fn shipped_migrations_are_numbered_and_unchanged() {
    validate(MIGRATIONS).unwrap();
    let lock = include_str!("migrations/CHECKSUMS");
    if let Err(e) = check_checksums(MIGRATIONS, lock) {
        panic!("{e}");
    }
}

/// `build.rs` reruns when a migration is added or changed: the compiled-in list is the directory
/// as it is now, names and text. A stale build-script output fails here.
#[test]
fn compiled_migrations_match_the_files() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/store/migrations");
    let mut files: Vec<(String, String)> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".sql"))
        .map(|n| {
            let text = std::fs::read_to_string(dir.join(&n)).unwrap();
            (n, text)
        })
        .collect();
    files.sort();
    let compiled: Vec<(String, String)> = MIGRATIONS
        .iter()
        .map(|(n, t)| ((*n).to_owned(), (*t).to_owned()))
        .collect();
    assert_eq!(compiled, files);
}

#[tokio::test]
async fn fresh_database_gets_every_migration_and_wal() {
    let dir = TempDir::new();
    let applied = Arc::new(Mutex::new(Vec::new()));
    let seen = applied.clone();
    let options = Options {
        observer: Arc::new(move |e| {
            if let Event::MigrationApplied { name, .. } = e {
                seen.lock().unwrap().push(name.to_string());
            }
        }),
        ..Options::default()
    };
    let store = Store::open_with(&dir.0, "regtest", options, &[M1, M2]).unwrap();
    assert_eq!(*applied.lock().unwrap(), [M1.0, M2.0]);
    assert_eq!(version(&store).await, 2);
    let mode: String = store
        .read(|sql| {
            Ok(sql
                .prepare("PRAGMA journal_mode")?
                .query_row([], |r| r.get(0))?)
        })
        .await
        .unwrap();
    assert_eq!(mode, "wal");
    assert!(dir.0.join("regtest.sqlite").exists());
    store.close().await;
}

#[tokio::test]
async fn upgrade_applies_only_the_new_migrations() {
    let dir = TempDir::new();
    let store = open(&dir.0, &[M1]).unwrap();
    insert(&store, 7).await;
    store.close().await;

    let applied = Arc::new(AtomicUsize::new(0));
    let seen = applied.clone();
    let options = Options {
        observer: Arc::new(move |_| {
            seen.fetch_add(1, Ordering::Relaxed);
        }),
        ..Options::default()
    };
    let store = Store::open_with(&dir.0, "regtest", options.clone(), &[M1, M2]).unwrap();
    assert_eq!(applied.load(Ordering::Relaxed), 1);
    assert_eq!(version(&store).await, 2);
    assert_eq!(count(&store).await, 1);
    store.close().await;

    // Opening again applies nothing.
    let store = Store::open_with(&dir.0, "regtest", options, &[M1, M2]).unwrap();
    assert_eq!(applied.load(Ordering::Relaxed), 1);
    store.close().await;
}

#[tokio::test]
async fn a_newer_database_is_refused() {
    let dir = TempDir::new();
    open(&dir.0, &[M1, M2]).unwrap().close().await;
    match open(&dir.0, &[M1]) {
        Err(OpenError::TooNew {
            found: 2,
            supported: 1,
        }) => {}
        Err(e) => panic!("{e}"),
        Ok(_) => panic!("opened a newer database"),
    }
}

#[tokio::test]
async fn a_failed_migration_rolls_back_alone() {
    let dir = TempDir::new();
    let bad: Migration = (
        "0002_bad.sql",
        "CREATE TABLE half (x); SELECT * FROM missing;",
    );
    match open(&dir.0, &[M1, bad]) {
        Err(OpenError::Migration {
            name: "0002_bad.sql",
            ..
        }) => {}
        Err(e) => panic!("{e}"),
        Ok(_) => panic!("a broken migration applied"),
    }
    let store = open(&dir.0, &[M1]).unwrap();
    assert_eq!(version(&store).await, 1);
    let half: i64 = store
        .read(|sql| {
            Ok(sql
                .prepare("SELECT count(*) FROM sqlite_schema WHERE name = 'half'")?
                .query_row([], |r| r.get(0))?)
        })
        .await
        .unwrap();
    assert_eq!(half, 0);
    store.close().await;
}

#[test]
fn misnumbered_migrations_stop_the_open() {
    let dir = TempDir::new();
    assert!(matches!(
        open(&dir.0, &[M2]),
        Err(OpenError::BadMigrations(_))
    ));
}

#[tokio::test]
async fn a_corrupt_file_is_reported() {
    let dir = TempDir::new();
    std::fs::create_dir_all(&dir.0).unwrap();
    std::fs::write(dir.0.join("regtest.sqlite"), vec![0x5a; 8192]).unwrap();
    assert!(matches!(
        open(&dir.0, &[M1]),
        Err(OpenError::Corrupt { .. })
    ));

    // A real database with a damaged page, past the header.
    let dir = TempDir::new();
    let store = open(&dir.0, &[M1]).unwrap();
    store
        .write(|sql| {
            let mut insert = sql.prepare("INSERT INTO items (label) VALUES (?1)")?;
            for i in 0..2000 {
                insert.execute(params![format!("item {i}")])?;
            }
            Ok(())
        })
        .await
        .unwrap();
    store.close().await;
    let file = dir.0.join("regtest.sqlite");
    let mut bytes = std::fs::read(&file).unwrap();
    assert!(bytes.len() > 3 * 4096);
    bytes[2 * 4096..3 * 4096].fill(0x5a);
    std::fs::write(&file, bytes).unwrap();
    match open(&dir.0, &[M1]) {
        Err(OpenError::Corrupt { .. }) => {}
        Err(e) => panic!("{e}"),
        Ok(_) => panic!("opened a damaged database"),
    }
}

#[test]
fn network_names_cannot_leave_the_directory() {
    let dir = TempDir::new();
    for bad in ["", "../x", "Main", "a/b", "a.b"] {
        assert!(
            matches!(
                Store::open(&dir.0, bad, Options::default()),
                Err(OpenError::BadName)
            ),
            "{bad}"
        );
    }
}

#[tokio::test]
async fn readers_cannot_write() {
    let dir = TempDir::new();
    let store = open(&dir.0, &[M1]).unwrap();
    let result = store
        .read(|sql| {
            Ok(sql
                .prepare("INSERT INTO items (label) VALUES ('x')")?
                .execute([])?)
        })
        .await;
    match result {
        Err(Error::Sql(e)) => {
            assert_eq!(e.sqlite_error_code(), Some(rusqlite::ErrorCode::ReadOnly))
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(count(&store).await, 0);
    store.close().await;
}

#[tokio::test]
async fn a_failed_or_panicking_write_rolls_back_and_the_writer_survives() {
    let dir = TempDir::new();
    let store = open(&dir.0, &[M1]).unwrap();
    let failed = store
        .write(|sql| {
            sql.prepare("INSERT INTO items (id, label) VALUES (1, 'x')")?
                .execute([])?;
            sql.prepare("INSERT INTO items (id, label) VALUES (1, 'duplicate')")?
                .execute([])?;
            Ok(())
        })
        .await;
    assert!(matches!(failed, Err(Error::Sql(_))));
    let panicked = store
        .write(|sql| -> Result<(), Error> {
            sql.prepare("INSERT INTO items (id, label) VALUES (2, 'x')")?
                .execute([])?;
            panic!("job bug");
        })
        .await;
    assert!(matches!(panicked, Err(Error::Panicked)));
    assert_eq!(count(&store).await, 0);
    insert(&store, 3).await;
    assert_eq!(count(&store).await, 1);
    store.close().await;
}

/// What the observer heard about failing writes: `(kind, detail, failures)`, and recoveries.
type Heard = Arc<Mutex<(Vec<(&'static str, String, u32)>, Vec<u32>)>>;

fn listening(dir: &Path) -> (Store, Heard) {
    let heard = Heard::default();
    let h = heard.clone();
    let observer = move |e: &Event| {
        let mut h = h.lock().unwrap();
        match e {
            Event::WriteFailed {
                kind,
                detail,
                failures,
            } => h.0.push((kind, (*detail).to_owned(), *failures)),
            Event::WriteRecovered { failures } => h.1.push(*failures),
            _ => {}
        }
    };
    let options = Options {
        observer: Arc::new(observer),
        ..Options::default()
    };
    (
        Store::open_with(dir, "regtest", options, &[M1]).unwrap(),
        heard,
    )
}

/// Sets a writer pragma that outlives the write, as a full disk or a read-only mount would.
async fn writer_pragma(store: &Store, sql: &'static str) {
    store
        .write(move |s| {
            let _: Option<i64> = s.prepare(sql)?.query_row([], |r| r.get(0)).optional()?;
            Ok(())
        })
        .await
        .unwrap();
}

async fn insert_big(store: &Store, id: i64) -> Result<(), Error> {
    store
        .write(move |sql| {
            sql.prepare("INSERT INTO items (id, label) VALUES (?1, ?2)")?
                .execute(params![id, "x".repeat(100_000)])?;
            Ok(())
        })
        .await
}

/// A full disk, simulated by capping the file at its current size: SQLite answers exactly as it
/// does when the disk is full (`SQLITE_FULL`). Every failed write counts; the 1st, 2nd, 4th and
/// 8th are reported with SQLite's message; the health says so until a write commits again.
#[tokio::test]
async fn a_full_disk_is_classified_reported_in_powers_of_two_and_recovers() {
    let dir = TempDir::new();
    let (store, heard) = listening(&dir.0);
    assert_eq!(store.write_health(), WriteHealth::default());
    insert(&store, 1).await;
    writer_pragma(&store, "PRAGMA max_page_count = 1").await;
    let ok = store.write_health();
    assert!(ok.last_ok.is_some() && ok.failures == 0 && ok.kind.is_none());

    for i in 0..9 {
        let e = insert_big(&store, 10 + i).await.unwrap_err();
        assert!(matches!(e, Error::Sql(_)), "{e}");
    }
    let h = store.write_health();
    assert_eq!(
        (h.failures, h.kind, h.last_ok),
        (9, Some("disk_full"), ok.last_ok)
    );
    let failed = heard.lock().unwrap().0.clone();
    assert_eq!(
        failed.iter().map(|f| f.2).collect::<Vec<_>>(),
        [1, 2, 4, 8],
        "{failed:?}"
    );
    assert!(failed.iter().all(|f| f.0 == "disk_full"));
    assert_eq!(failed[0].1, "database or disk is full");

    writer_pragma(&store, "PRAGMA max_page_count = 1000000").await;
    assert_eq!(heard.lock().unwrap().1, [9]);
    let h = store.write_health();
    assert!(h.failures == 0 && h.kind.is_none() && h.last_ok > ok.last_ok);
    insert_big(&store, 99).await.unwrap();
    store.close().await;
}

/// Writes refused as on a read-only mount (`query_only` gives SQLite's `SQLITE_READONLY`), and a
/// read-only file classified the same way. A constraint failure is the caller's, not the store's.
#[tokio::test]
async fn read_only_is_classified_and_constraints_are_not_failures() {
    let dir = TempDir::new();
    let (store, heard) = listening(&dir.0);
    insert(&store, 1).await;
    let dup = store
        .write(|sql| {
            sql.prepare("INSERT INTO items (id, label) VALUES (1, 'x')")?
                .execute([])?;
            Ok(())
        })
        .await;
    assert!(matches!(dup, Err(Error::Sql(_))));
    assert_eq!(store.write_health().failures, 0);

    writer_pragma(&store, "PRAGMA query_only = ON").await;
    assert!(insert_big(&store, 2).await.is_err());
    assert_eq!(store.write_health().kind, Some("read_only"));
    let failed = heard.lock().unwrap().0.clone();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].0, "read_only");
    assert!(failed[0].1.contains("readonly"), "{failed:?}");
    store.close().await;

    // The same code from a file the process may not write.
    let file = dir.0.join("regtest.sqlite");
    let conn =
        rusqlite::Connection::open_with_flags(&file, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let e = conn
        .execute("INSERT INTO items (id, label) VALUES (5, 'x')", [])
        .unwrap_err();
    assert_eq!(failure_kind(&e), Some("read_only"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_queue_pushes_back() {
    let dir = TempDir::new();
    let options = Options {
        queue: 1,
        ..Options::default()
    };
    let store = Arc::new(Store::open_with(&dir.0, "regtest", options, &[M1]).unwrap());
    let (started_tx, started) = std_mpsc::channel();
    let (release, wait) = std_mpsc::channel::<()>();
    let busy = tokio::spawn({
        let store = store.clone();
        async move {
            store
                .write(move |_| {
                    started_tx.send(()).unwrap();
                    wait.recv().unwrap();
                    Ok(())
                })
                .await
        }
    });
    tokio::task::spawn_blocking(move || started.recv().unwrap())
        .await
        .unwrap();
    // The writer is busy; this one fills the queue.
    let queued = tokio::spawn({
        let store = store.clone();
        async move { store.write(|_| Ok(())).await }
    });
    while store.queue_depth() < 1 {
        tokio::task::yield_now().await;
    }
    assert!(matches!(
        store.try_write(|_| Ok(())).await,
        Err(Error::Full)
    ));
    let waiting = tokio::time::timeout(Duration::from_millis(100), store.write(|_| Ok(()))).await;
    assert!(waiting.is_err(), "write did not wait for room");

    release.send(()).unwrap();
    busy.await.unwrap().unwrap();
    queued.await.unwrap().unwrap();
    store.write(|_| Ok(())).await.unwrap();
    assert_eq!(store.queue_depth(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn readers_answer_while_a_write_is_open() {
    let dir = TempDir::new();
    let store = Arc::new(open(&dir.0, &[M1]).unwrap());
    insert(&store, 1).await;
    let (inserted_tx, inserted) = std_mpsc::channel();
    let (release, wait) = std_mpsc::channel::<()>();
    let write = tokio::spawn({
        let store = store.clone();
        async move {
            store
                .write(move |sql| {
                    let mut insert = sql.prepare("INSERT INTO items (label) VALUES ('bulk')")?;
                    for _ in 0..10_000 {
                        insert.execute([])?;
                    }
                    inserted_tx.send(()).unwrap();
                    wait.recv().unwrap();
                    Ok(())
                })
                .await
        }
    });
    tokio::task::spawn_blocking(move || inserted.recv().unwrap())
        .await
        .unwrap();
    // The write transaction is open with 10,000 rows in it: readers see the last commit.
    for _ in 0..20 {
        let start = Instant::now();
        assert_eq!(count(&store).await, 1);
        assert!(start.elapsed() < Duration::from_secs(1));
    }
    release.send(()).unwrap();
    write.await.unwrap().unwrap();
    assert_eq!(count(&store).await, 10_001);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_slow_read_is_interrupted_and_the_connection_reused() {
    let dir = TempDir::new();
    let timeouts = Arc::new(AtomicUsize::new(0));
    let seen = timeouts.clone();
    let options = Options {
        readers: 1,
        read_time_limit: Duration::from_millis(100),
        observer: Arc::new(move |e| {
            if matches!(e, Event::ReadTimedOut { .. }) {
                seen.fetch_add(1, Ordering::Relaxed);
            }
        }),
        ..Options::default()
    };
    let store = Store::open_with(&dir.0, "regtest", options, &[M1]).unwrap();
    let start = Instant::now();
    let endless = store
        .read(|sql| {
            Ok(sql
                .prepare("WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c) SELECT count(*) FROM c")?
                .query_row([], |r| r.get::<_, i64>(0))?)
        })
        .await;
    assert!(matches!(endless, Err(Error::TimedOut)), "{endless:?}");
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(timeouts.load(Ordering::Relaxed), 1);
    assert_eq!(count(&store).await, 0);
    store.close().await;
}

const ENDLESS: &str =
    "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c) SELECT count(*) FROM c";

fn one_reader(limit_ms: u64) -> Options {
    Options {
        readers: 1,
        read_time_limit: Duration::from_millis(limit_ms),
        ..Options::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_time_limit_holds_across_statements() {
    let dir = TempDir::new();
    let store = Store::open_with(&dir.0, "regtest", one_reader(50), &[M1]).unwrap();
    let start = Instant::now();
    // No statement runs when the limit passes; the next one must still be stopped.
    let late = store
        .read(|sql| {
            std::thread::sleep(Duration::from_millis(150));
            Ok(sql
                .prepare(ENDLESS)?
                .query_row([], |r| r.get::<_, i64>(0))?)
        })
        .await;
    assert!(matches!(late, Err(Error::TimedOut)), "{late:?}");
    assert!(start.elapsed() < Duration::from_secs(2));
    store.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_read_still_stops_and_frees_its_connection() {
    let dir = TempDir::new();
    let store = Store::open_with(&dir.0, "regtest", one_reader(100), &[M1]).unwrap();
    let cancelled = tokio::time::timeout(
        Duration::from_millis(20),
        store.read(|sql| {
            Ok(sql
                .prepare(ENDLESS)?
                .query_row([], |r| r.get::<_, i64>(0))?)
        }),
    )
    .await;
    assert!(cancelled.is_err());
    // The only connection comes back once the abandoned query hits its limit.
    let start = Instant::now();
    assert_eq!(count(&store).await, 0);
    assert!(start.elapsed() < Duration::from_secs(2));
    store.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_time_limit_stops_many_short_statements() {
    let dir = TempDir::new();
    let store = Store::open_with(&dir.0, "regtest", one_reader(20), &[M1]).unwrap();
    // Each statement is far too short to reach the progress handler on its own.
    let fresh: Result<(), _> = store
        .read(|sql| {
            loop {
                sql.prepare("SELECT 1")?
                    .query_row([], |r| r.get::<_, i64>(0))?;
            }
        })
        .await;
    assert!(matches!(fresh, Err(Error::TimedOut)), "{fresh:?}");
    let reused: Result<(), _> = store
        .read(|sql| {
            let mut one = sql.prepare("SELECT 1")?;
            loop {
                one.query_row([], |r| r.get::<_, i64>(0))?;
            }
        })
        .await;
    assert!(matches!(reused, Err(Error::TimedOut)), "{reused:?}");
    store.close().await;
}

#[tokio::test]
async fn a_write_cannot_end_its_own_transaction() {
    let dir = TempDir::new();
    let store = open(&dir.0, &[M1]).unwrap();
    for end in ["COMMIT", "END", "ROLLBACK"] {
        let ended = store
            .write(move |sql| {
                sql.prepare("INSERT INTO items (label) VALUES ('x')")?
                    .execute([])?;
                let mut stmt = match end {
                    "COMMIT" => sql.prepare("COMMIT")?,
                    "END" => sql.prepare("END")?,
                    _ => sql.prepare("ROLLBACK")?,
                };
                // Ignore the outcome, as careless code would, and carry on writing.
                let _ = stmt.execute([]);
                let _ = sql
                    .prepare("INSERT INTO items (label) VALUES ('after')")?
                    .execute([]);
                Ok(())
            })
            .await;
        assert!(matches!(ended, Err(Error::Sql(_))), "{end}: {ended:?}");
        assert_eq!(count(&store).await, 0, "{end}");
    }
    // `OR ROLLBACK` ends the transaction without a statement saying so; later rows must not
    // slip through in autocommit, whether the closure then fails or claims success.
    for fail in [true, false] {
        let result = store
            .write(move |sql| {
                sql.prepare("INSERT INTO items (id, label) VALUES (1, 'x')")?
                    .execute([])?;
                let again = sql
                    .prepare("INSERT OR ROLLBACK INTO items (id, label) VALUES (1, 'x')")?
                    .execute([]);
                assert!(again.is_err());
                let _ = sql
                    .prepare("INSERT INTO items (id, label) VALUES (2, 'x')")?
                    .execute([]);
                if fail { Err(Error::Closed) } else { Ok(()) }
            })
            .await;
        assert!(result.is_err(), "{result:?}");
        assert_eq!(count(&store).await, 0, "fail={fail}");
    }
    // Whatever the closure does with the refusal, an error or a panic still rolls back.
    let panicked = store
        .write(|sql| -> Result<(), Error> {
            sql.prepare("INSERT INTO items (label) VALUES ('x')")?
                .execute([])?;
            let _ = sql.prepare("COMMIT");
            panic!("after a refused commit");
        })
        .await;
    assert!(matches!(panicked, Err(Error::Panicked)));
    assert_eq!(count(&store).await, 0);
    // Ending the transaction implicitly and opening a new one must not commit the remainder.
    for begin in ["BEGIN", "SAVEPOINT s"] {
        let replaced = store
            .write(move |sql| {
                sql.prepare("INSERT INTO items (id, label) VALUES (1, 'x')")?
                    .execute([])?;
                let _ = sql
                    .prepare("INSERT OR ROLLBACK INTO items (id, label) VALUES (1, 'x')")?
                    .execute([]);
                if let Ok(mut stmt) = if begin == "BEGIN" {
                    sql.prepare("BEGIN")
                } else {
                    sql.prepare("SAVEPOINT s")
                } {
                    let _ = stmt.execute([]);
                }
                let _ = sql
                    .prepare("INSERT INTO items (id, label) VALUES (2, 'x')")?
                    .execute([]);
                Ok(())
            })
            .await;
        assert!(replaced.is_err(), "{begin}: {replaced:?}");
    }
    assert_eq!(count(&store).await, 0);
    store.close().await;
}

#[tokio::test]
async fn a_migration_cannot_end_its_own_transaction() {
    for sql in [
        "CREATE TABLE a (x); COMMIT; CREATE TABLE b (x);",
        "CREATE TABLE a (x); ROLLBACK;",
        "CREATE TABLE a (x); ROLLBACK; BEGIN IMMEDIATE; CREATE TABLE b (x);",
        "CREATE TABLE a (x); ROLLBACK; SAVEPOINT s; CREATE TABLE b (x);",
    ] {
        let dir = TempDir::new();
        match open(&dir.0, &[M1, ("0002_split.sql", sql)]) {
            Err(OpenError::Migration {
                name: "0002_split.sql",
                ..
            }) => {}
            Err(e) => panic!("{sql}: {e}"),
            Ok(_) => panic!("{sql}: applied"),
        }
        // Not marked applied: the database is still at version 1.
        let store = open(&dir.0, &[M1]).unwrap();
        assert_eq!(version(&store).await, 1, "{sql}");
        store.close().await;
    }
}

#[tokio::test]
async fn a_read_cannot_leave_a_snapshot_open() {
    let dir = TempDir::new();
    let store = Store::open_with(&dir.0, "regtest", one_reader(2000), &[M1]).unwrap();
    store
        .read(|sql| {
            sql.prepare("BEGIN")?.execute([])?;
            Ok(sql
                .prepare("SELECT count(*) FROM items")?
                .query_row([], |r| r.get::<_, i64>(0))?)
        })
        .await
        .unwrap();
    insert(&store, 1).await;
    assert_eq!(count(&store).await, 1);
    store.close().await;
}

const CHILD_DIR: &str = "ENGINE_STORE_DURABILITY_DIR";

/// Run by `acknowledged_writes_survive_a_kill` in a child process; does nothing on its own.
#[tokio::test]
async fn durability_child() {
    let Some(dir) = std::env::var_os(CHILD_DIR) else {
        return;
    };
    let dir = PathBuf::from(dir);
    let store = open(&dir, &[M1]).unwrap();
    insert(&store, 1).await;
    std::fs::write(dir.join("acked"), "").unwrap();
    let marker = dir.join("inflight");
    let _ = store
        .write(move |sql| {
            sql.prepare("INSERT INTO items (id, label) VALUES (2, 'x')")?
                .execute([])?;
            std::fs::write(marker, "").unwrap();
            std::thread::sleep(Duration::from_secs(600));
            Ok(())
        })
        .await;
}

#[tokio::test]
async fn acknowledged_writes_survive_a_kill() {
    let dir = TempDir::new();
    std::fs::create_dir_all(&dir.0).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "store::tests::durability_child", "--nocapture"])
        .env(CHILD_DIR, &dir.0)
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !dir.0.join("inflight").exists() {
        assert!(
            Instant::now() < deadline,
            "child never reached its uncommitted write"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(dir.0.join("acked").exists());
    child.kill().unwrap(); // SIGKILL: no destructors, no checkpoint.
    child.wait().unwrap();

    let store = open(&dir.0, &[M1]).unwrap();
    let ids: Vec<i64> = store
        .read(|sql| {
            let mut stmt = sql.prepare("SELECT id FROM items ORDER BY id")?;
            let ids = stmt
                .query_map([], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            Ok(ids)
        })
        .await
        .unwrap();
    assert_eq!(ids, [1]);
    store.close().await;
}

/// A block summary's shape: a dozen integer and short text columns, looked up by hash.
const BLOCKS: Migration = (
    "0001_blocks.sql",
    "CREATE TABLE blocks (
        height INTEGER PRIMARY KEY, hash TEXT NOT NULL, time INTEGER NOT NULL,
        tx_count INTEGER NOT NULL, size INTEGER NOT NULL, weight INTEGER NOT NULL,
        fees INTEGER NOT NULL, reward INTEGER NOT NULL, fee_p10 INTEGER NOT NULL,
        fee_median INTEGER NOT NULL, fee_p90 INTEGER NOT NULL, pool TEXT, coinbase_tag TEXT);
     CREATE UNIQUE INDEX blocks_hash ON blocks (hash);",
);

fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

fn block_hash(height: u64) -> String {
    (0..4)
        .map(|i| format!("{:016x}", mix(height * 4 + i)))
        .collect()
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    sorted[((sorted.len() - 1) as f64 * p) as usize]
}

/// `cargo test --release -p engine -- --ignored --nocapture bulk_insert_benchmark`
// A manual probe: its report is printed for a person to read (LOGGING.md rule 1).
#[allow(clippy::print_stdout)]
#[ignore = "benchmark: run in release with --nocapture"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bulk_insert_benchmark() {
    const ROWS: u64 = 1_000_000;
    const BATCH: u64 = 10_000;
    let dir = TempDir::new();
    let store = Arc::new(open(&dir.0, &[BLOCKS]).unwrap());
    let committed = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let reader = |range: bool| {
        let (store, committed, done) = (store.clone(), committed.clone(), done.clone());
        tokio::spawn(async move {
            let mut times = Vec::new();
            let mut n = 0u64;
            while !done.load(Ordering::Relaxed) {
                let top = committed.load(Ordering::Relaxed);
                if top == 0 {
                    tokio::task::yield_now().await;
                    continue;
                }
                n += 1;
                let height = (mix(n) % top) as i64;
                let start = Instant::now();
                let found = if range {
                    store
                        .read(move |sql| {
                            let mut stmt = sql.prepare(
                                "SELECT height, fees, fee_median FROM blocks
                                 WHERE height BETWEEN ?1 AND ?1 + 99",
                            )?;
                            let rows = stmt.query_map(params![height], |r| r.get::<_, i64>(1))?;
                            Ok(rows.count())
                        })
                        .await
                        .unwrap()
                } else {
                    let hash = block_hash(height as u64);
                    store
                        .read(move |sql| {
                            Ok(sql
                                .prepare("SELECT height FROM blocks WHERE hash = ?1")?
                                .query_row(params![hash], |r| r.get::<_, i64>(0))
                                .optional()?
                                .map_or(0, |_| 1))
                        })
                        .await
                        .unwrap()
                };
                times.push(start.elapsed());
                assert!(found > 0);
            }
            times
        })
    };
    let lookups = reader(false);
    let scans = reader(true);

    let start = Instant::now();
    let mut slowest_batch = Duration::ZERO;
    for first in (0..ROWS).step_by(BATCH as usize) {
        let batch_start = Instant::now();
        store
            .write(move |sql| {
                let mut insert = sql.prepare(
                    "INSERT INTO blocks VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                )?;
                for h in first..first + BATCH {
                    let r = (mix(h) >> 1) as i64;
                    insert.execute(params![
                        h as i64,
                        block_hash(h),
                        1_231_006_505 + h as i64 * 600,
                        r % 5000,
                        r % 2_000_000,
                        r % 4_000_000,
                        r % 100_000_000,
                        625_000_000 + r % 100_000_000,
                        r % 10,
                        r % 50,
                        r % 500,
                        ["Foundry USA", "AntPool", "ViaBTC", "F2Pool"][(r % 4) as usize],
                        "/mined by someone/",
                    ])?;
                }
                Ok(())
            })
            .await
            .unwrap();
        slowest_batch = slowest_batch.max(batch_start.elapsed());
        committed.store(first + BATCH, Ordering::Relaxed);
    }
    let elapsed = start.elapsed();
    done.store(true, Ordering::Relaxed);
    let mut lookups = lookups.await.unwrap();
    let mut scans = scans.await.unwrap();
    Arc::into_inner(store).unwrap().close().await;

    let size = std::fs::metadata(dir.0.join("regtest.sqlite"))
        .unwrap()
        .len();
    println!(
        "inserted {ROWS} rows in {elapsed:.2?}: {:.0} rows/s, slowest {BATCH}-row batch {slowest_batch:.2?}, file {:.1} MiB",
        ROWS as f64 / elapsed.as_secs_f64(),
        size as f64 / (1024.0 * 1024.0)
    );
    for (name, times) in [
        ("hash lookup", &mut lookups),
        ("100-row range scan", &mut scans),
    ] {
        times.sort();
        println!(
            "{name} during the insert: {} reads, median {:.2?}, p99 {:.2?}, worst {:.2?}",
            times.len(),
            percentile(times, 0.5),
            percentile(times, 0.99),
            percentile(times, 1.0)
        );
    }
    let start = Instant::now();
    let store = open(&dir.0, &[BLOCKS]).unwrap();
    println!("reopen with quick_check: {:.2?}", start.elapsed());
    store.close().await;
}
