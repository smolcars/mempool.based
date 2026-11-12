//! The SQLite store: one file per network for the data the node does not keep (docs/STORE.md).
//!
//! One writer thread owns the only read-write connection and takes work through a bounded
//! queue. Reads go to a small pool of read-only connections on tokio's blocking threads. SQL is
//! a `&'static str`, so a query cannot be built from request data by formatting a string.

mod migrate;
#[cfg(test)]
mod tests;

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::{Connection, ErrorCode, OpenFlags, TransactionBehavior};
use tokio::sync::{Semaphore, mpsc, oneshot};

pub use migrate::MigrationError;
pub use rusqlite::{Error as SqlError, OptionalExtension, Row, Statement, params};

/// The migrations in `migrations/`, compiled in by `build.rs`.
pub(crate) const MIGRATIONS: &[migrate::Migration] =
    include!(concat!(env!("OUT_DIR"), "/migrations.rs"));

/// A write that holds the writer this long is reported as [`Event::SlowWrite`].
const SLOW_WRITE: Duration = Duration::from_secs(1);
/// How long a connection waits for a lock held by another process, such as a backup.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Something an operator may want recorded. Passed to [`Options::observer`].
#[derive(Debug)]
pub enum Event<'a> {
    /// A migration was applied while opening.
    MigrationApplied { name: &'a str, elapsed: Duration },
    /// One write held the writer for at least a second. `queue_depth` writes were waiting.
    SlowWrite {
        elapsed: Duration,
        queue_depth: usize,
    },
    /// A read ran past [`Options::read_time_limit`] and was stopped.
    ReadTimedOut { limit: Duration },
    /// A write failed in SQLite, at the 1st, 2nd, 4th… failure in a row. `kind` is from
    /// [`failure_kind`]; `detail` is SQLite's own message, for the log and never for a client.
    WriteFailed {
        kind: &'static str,
        detail: &'a str,
        failures: u32,
    },
    /// A write committed after `failures` in a row had failed.
    WriteRecovered { failures: u32 },
}

/// What the writer last saw, for a status page: [`Store::write_health`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WriteHealth {
    /// When a write last committed.
    pub last_ok: Option<Instant>,
    /// Writes that failed in SQLite since then; `0` while writes work.
    pub failures: u32,
    /// The latest failure's [`failure_kind`].
    pub kind: Option<&'static str>,
}

/// Classifies a failed write by SQLite's code, so an operator reads "disk_full", not a code:
/// `disk_full`, `read_only`, `corrupt`, `busy` (another process holds the lock past the busy
/// timeout), `io` or `other`. `None` for a constraint failure, which is the caller's logic and
/// says nothing about the store.
pub fn failure_kind(e: &rusqlite::Error) -> Option<&'static str> {
    Some(match e.sqlite_error_code() {
        Some(ErrorCode::ConstraintViolation) => return None,
        Some(ErrorCode::DiskFull) => "disk_full",
        Some(ErrorCode::ReadOnly) => "read_only",
        Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase) => "corrupt",
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => "busy",
        Some(ErrorCode::SystemIoFailure | ErrorCode::CannotOpen) => "io",
        _ => "other",
    })
}

type Observer = Arc<dyn Fn(&Event) + Send + Sync>;

/// Tuning for [`Store::open`]. The defaults suit the server.
#[derive(Clone)]
pub struct Options {
    /// Read-only connections; at most this many reads run at once.
    pub readers: usize,
    /// Writes that may wait for the writer before [`Store::write`] waits too.
    pub queue: usize,
    /// A read whose SQL is still running after this is stopped and fails with [`Error::TimedOut`].
    pub read_time_limit: Duration,
    /// Called with each [`Event`], from whichever thread saw it. Keep it quick.
    pub observer: Observer,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            readers: 4,
            queue: 64,
            read_time_limit: Duration::from_secs(2),
            observer: Arc::new(|_| {}),
        }
    }
}

/// Why [`Store::open`] failed. Opening never panics.
#[derive(Debug)]
pub enum OpenError {
    /// The network name is not lowercase letters and digits.
    BadName,
    /// The data directory could not be created.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The file is not a SQLite database, or it is damaged. Restore a backup or delete it.
    Corrupt { path: PathBuf, detail: String },
    /// A newer version of the software wrote this file. Run that version, or delete the file.
    TooNew { found: i64, supported: usize },
    /// The compiled-in migrations are misnumbered. A build error that a test catches.
    BadMigrations(MigrationError),
    /// A migration failed; the database stays at the version before it.
    Migration {
        name: &'static str,
        source: rusqlite::Error,
    },
    /// The filesystem would not give us WAL mode (some network filesystems).
    NoWal(String),
    /// The writer thread could not be started.
    Thread(std::io::Error),
    /// Any other SQLite error.
    Sql(rusqlite::Error),
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadName => f.write_str("network name must be lowercase letters and digits"),
            Self::Io { path, source } => write!(f, "cannot create {}: {source}", path.display()),
            Self::Corrupt { path, detail } => {
                write!(
                    f,
                    "{} is corrupt or not a database: {detail}",
                    path.display()
                )
            }
            Self::TooNew { found, supported } => write!(
                f,
                "database schema version {found} is newer than this build supports ({supported})"
            ),
            Self::BadMigrations(e) => write!(f, "bad migrations: {e}"),
            Self::Migration { name, source } => write!(f, "migration {name} failed: {source}"),
            Self::NoWal(mode) => write!(f, "could not enable WAL mode (got {mode:?})"),
            Self::Thread(e) => write!(f, "could not start the writer thread: {e}"),
            Self::Sql(e) => write!(f, "sqlite: {e}"),
        }
    }
}

impl std::error::Error for OpenError {}

impl From<rusqlite::Error> for OpenError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sql(e)
    }
}

/// Why a read or a write failed.
#[derive(Debug)]
pub enum Error {
    Sql(rusqlite::Error),
    /// [`Store::try_write`] found the queue full.
    Full,
    /// The store was closed.
    Closed,
    /// The read ran past [`Options::read_time_limit`].
    TimedOut,
    /// The closure panicked. A write was rolled back.
    Panicked,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sql(e) => write!(f, "sqlite: {e}"),
            Self::Full => f.write_str("store write queue is full"),
            Self::Closed => f.write_str("store is closed"),
            Self::TimedOut => f.write_str("store read timed out"),
            Self::Panicked => f.write_str("store closure panicked"),
        }
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sql(e)
    }
}

/// A connection as the closures given to [`Store::read`] and [`Store::write`] see it. SQL must be
/// a `&'static str` literal; values go in as parameters (SECURITY.md rule 12).
/// In a write, the store owns the transaction: `BEGIN`, `COMMIT`, `ROLLBACK` and savepoints are
/// refused, and any other way of committing from inside the closure fails and rolls back.
pub struct Sql<'c> {
    conn: &'c Connection,
    /// A read's deadline. Checked here because a statement too short to reach SQLite's progress
    /// handler would otherwise never see it.
    deadline: Option<Instant>,
}

impl<'c> Sql<'c> {
    /// Prepares one statement. Bind values with [`params!`] when executing it.
    pub fn prepare(&self, sql: &'static str) -> Result<Statement<'c>, Error> {
        if past(self.deadline) {
            return Err(Error::TimedOut);
        }
        Ok(self.conn.prepare(sql)?)
    }
}

fn past(deadline: Option<Instant>) -> bool {
    deadline.is_some_and(|d| Instant::now() >= d)
}

/// Writer work. The flag is set while caller or migration SQL runs, to refuse any commit.
type Job = Box<dyn FnOnce(&mut Connection, &AtomicBool) -> Wrote + Send>;

/// How a job ended, for [`WriteHealth`].
enum Wrote {
    Committed,
    /// SQLite refused it: the [`failure_kind`] and SQLite's message.
    Failed(&'static str, String),
    /// The caller's own error, a constraint or a panic: nothing about the store.
    Other,
}

/// One network's SQLite file. Share it behind an `Arc`.
pub struct Store {
    queue: mpsc::Sender<Job>,
    health: Arc<Mutex<WriteHealth>>,
    writer: JoinHandle<()>,
    readers: Arc<Readers>,
    options: Options,
}

struct Readers {
    idle: Mutex<Vec<Connection>>,
    permits: Arc<Semaphore>,
}

impl Readers {
    fn take(&self) -> Option<Connection> {
        self.idle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop()
    }
    fn put(&self, conn: Connection) {
        self.idle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(conn);
    }
}

impl Store {
    /// Opens or creates `<dir>/<network>.sqlite` and brings its schema up to date. Blocking: call
    /// it at startup, before serving.
    pub fn open(dir: &Path, network: &str, options: Options) -> Result<Self, OpenError> {
        Self::open_with(dir, network, options, MIGRATIONS)
    }

    fn open_with(
        dir: &Path,
        network: &str,
        options: Options,
        migrations: &[migrate::Migration],
    ) -> Result<Self, OpenError> {
        let named = !network.is_empty()
            && network
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
        if !named {
            return Err(OpenError::BadName);
        }
        std::fs::create_dir_all(dir).map_err(|source| OpenError::Io {
            path: dir.into(),
            source,
        })?;
        let path = dir.join(format!("{network}.sqlite"));
        let corrupt = |e: rusqlite::Error| match e.sqlite_error_code() {
            Some(ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt) => OpenError::Corrupt {
                path: path.clone(),
                detail: e.to_string(),
            },
            _ => OpenError::Sql(e),
        };

        let mut conn = Connection::open(&path).map_err(corrupt)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        // WAL: readers never block the writer and the writer never blocks readers.
        let mode: String = conn
            .pragma_update_and_check(None, "journal_mode", "wal", |r| r.get(0))
            .map_err(corrupt)?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(OpenError::NoWal(mode));
        }
        // FULL: a commit is fsynced before the writer acknowledges it, so an acknowledged write
        // survives power loss. With one writer and batched writes the extra fsync is cheap.
        conn.pragma_update(None, "synchronous", "full")?;
        // Keeps the WAL from staying huge after a bulk insert that readers held open.
        conn.pragma_update(None, "journal_size_limit", 64 * 1024 * 1024)?;
        conn.pragma_update(None, "foreign_keys", true)?;
        let check: String = conn
            .pragma_query_value(None, "quick_check", |r| r.get(0))
            .map_err(corrupt)?;
        if check != "ok" {
            return Err(OpenError::Corrupt {
                path,
                detail: check,
            });
        }
        // Only the store begins and ends transactions. While caller or migration SQL runs
        // (`sealed`), transaction and savepoint statements are refused, so the transaction cannot
        // be ended or replaced; and any commit, such as an autocommit after `INSERT OR ROLLBACK`
        // ended it, is turned into a rollback. An error or panic still undoes everything.
        let sealed = Arc::new(AtomicBool::new(false));
        let gate = sealed.clone();
        conn.authorizer(Some(move |ctx: AuthContext<'_>| match ctx.action {
            AuthAction::Transaction { .. } | AuthAction::Savepoint { .. }
                if gate.load(Ordering::Relaxed) =>
            {
                Authorization::Deny
            }
            _ => Authorization::Allow,
        }))?;
        let gate = sealed.clone();
        conn.commit_hook(Some(move || gate.load(Ordering::Relaxed)))?;
        migrate::migrate(&mut conn, migrations, &sealed, &options.observer).map_err(
            |e| match e {
                OpenError::Sql(e) => corrupt(e),
                e => e,
            },
        )?;

        let mut idle = Vec::with_capacity(options.readers);
        for _ in 0..options.readers.max(1) {
            let reader = Connection::open_with_flags(
                &path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            reader.pragma_update(None, "query_only", true)?;
            reader.busy_timeout(BUSY_TIMEOUT)?;
            idle.push(reader);
        }
        let readers = Arc::new(Readers {
            permits: Arc::new(Semaphore::new(idle.len())),
            idle: Mutex::new(idle),
        });

        let (queue, mut jobs) = mpsc::channel::<Job>(options.queue.max(1));
        let observer = options.observer.clone();
        let health = Arc::new(Mutex::new(WriteHealth::default()));
        let seen = health.clone();
        let writer = std::thread::Builder::new()
            .name(format!("store-{network}"))
            .spawn(move || {
                while let Some(job) = jobs.blocking_recv() {
                    let start = Instant::now();
                    let wrote = job(&mut conn, &sealed);
                    let elapsed = start.elapsed();
                    record(&seen, wrote, &observer);
                    if elapsed >= SLOW_WRITE {
                        observer(&Event::SlowWrite {
                            elapsed,
                            queue_depth: jobs.len(),
                        });
                    }
                }
            })
            .map_err(OpenError::Thread)?;
        Ok(Self {
            queue,
            health,
            writer,
            readers,
            options,
        })
    }

    /// Runs `f` in one transaction on the writer, after the writes queued before it. Resolves
    /// once the transaction is committed and on disk; an error or a panic rolls it back. Waits
    /// for room when the queue is full: that wait is the back-pressure.
    pub async fn write<T, F>(&self, f: F) -> Result<T, Error>
    where
        F: FnOnce(&Sql) -> Result<T, Error> + Send + 'static,
        T: Send + 'static,
    {
        let (job, done) = job(f);
        self.queue.send(job).await.map_err(|_| Error::Closed)?;
        done.await.map_err(|_| Error::Closed)?
    }

    /// Like [`Store::write`], but fails with [`Error::Full`] at once when the queue is full,
    /// for callers that would rather drop a sample than wait.
    pub async fn try_write<T, F>(&self, f: F) -> Result<T, Error>
    where
        F: FnOnce(&Sql) -> Result<T, Error> + Send + 'static,
        T: Send + 'static,
    {
        let (job, done) = job(f);
        self.queue.try_send(job).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => Error::Full,
            mpsc::error::TrySendError::Closed(_) => Error::Closed,
        })?;
        done.await.map_err(|_| Error::Closed)?
    }

    /// Whether writes are committing, and when one last did.
    pub fn write_health(&self) -> WriteHealth {
        *self.health.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Writes waiting for the writer.
    pub fn queue_depth(&self) -> usize {
        self.queue.max_capacity() - self.queue.capacity()
    }

    /// Runs `f` on a read-only connection on tokio's blocking pool, so a slow query never holds
    /// a runtime thread. Waits for a free connection. Once `f` has run for
    /// [`Options::read_time_limit`], its running statement and any later one fail, and the read
    /// returns [`Error::TimedOut`]; this holds even if the caller stops waiting.
    pub async fn read<T, F>(&self, f: F) -> Result<T, Error>
    where
        F: FnOnce(&Sql) -> Result<T, Error> + Send + 'static,
        T: Send + 'static,
    {
        let permit = self
            .readers
            .permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::Closed)?;
        let mut conn = self.readers.take().ok_or(Error::Closed)?;
        let readers = self.readers.clone();
        let (limit, observer) = (self.options.read_time_limit, self.options.observer.clone());
        let task = tokio::task::spawn_blocking(move || {
            // The deadline is enforced on this thread, whether or not anyone still awaits the
            // read: SQLite's progress handler stops a long statement every 1,000 VM steps, and
            // `Sql::prepare` refuses a new one.
            let deadline = Instant::now().checked_add(limit);
            let result = match conn.progress_handler(1000, Some(move || past(deadline))) {
                Ok(()) => catch_unwind(AssertUnwindSafe(|| {
                    f(&Sql {
                        conn: &conn,
                        deadline,
                    })
                }))
                .unwrap_or(Err(Error::Panicked)),
                Err(e) => Err(e.into()),
            };
            // A read transaction left open would pin its snapshot for the next read.
            if !conn.is_autocommit() {
                let _ = conn.execute_batch("ROLLBACK");
            }
            readers.put(conn);
            drop(permit);
            if result.is_err() && past(deadline) {
                observer(&Event::ReadTimedOut { limit });
                return Err(Error::TimedOut);
            }
            result
        });
        task.await.unwrap_or(Err(Error::Panicked))
    }

    /// Finishes the queued writes, then closes the file. Dropping the store does the same
    /// without waiting.
    pub async fn close(self) {
        drop(self.queue);
        let writer = self.writer;
        // A panicking writer thread has nothing left to clean up.
        let _ = tokio::task::spawn_blocking(move || writer.join()).await;
    }
}

/// Wraps `f` as a writer job that commits on `Ok` and reports through the returned receiver.
fn job<T, F>(f: F) -> (Job, oneshot::Receiver<Result<T, Error>>)
where
    F: FnOnce(&Sql) -> Result<T, Error> + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = oneshot::channel();
    let job: Job = Box::new(move |conn, sealed| {
        let result: Result<T, Error> = (|| {
            let t = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            sealed.store(true, Ordering::Relaxed);
            let value = catch_unwind(AssertUnwindSafe(|| {
                f(&Sql {
                    conn: &t,
                    deadline: None,
                })
            }));
            sealed.store(false, Ordering::Relaxed);
            // Dropping `t` without committing rolls back, on an error and on a panic alike.
            let value = value.unwrap_or(Err(Error::Panicked))?;
            t.commit()?;
            Ok(value)
        })();
        let wrote = match &result {
            Ok(_) => Wrote::Committed,
            Err(Error::Sql(e)) => {
                failure_kind(e).map_or(Wrote::Other, |kind| Wrote::Failed(kind, e.to_string()))
            }
            Err(_) => Wrote::Other,
        };
        // The caller may have stopped waiting; the write stands either way.
        let _ = tx.send(result);
        wrote
    });
    (job, rx)
}

/// Keeps [`WriteHealth`] and reports failures at the 1st, 2nd, 4th… in a row, then the recovery.
fn record(health: &Mutex<WriteHealth>, wrote: Wrote, observer: &Observer) {
    let mut h = health.lock().unwrap_or_else(PoisonError::into_inner);
    match wrote {
        Wrote::Committed => {
            let failures = std::mem::take(&mut h.failures);
            *h = WriteHealth {
                last_ok: Some(Instant::now()),
                failures: 0,
                kind: None,
            };
            drop(h);
            if failures > 0 {
                observer(&Event::WriteRecovered { failures });
            }
        }
        Wrote::Failed(kind, detail) => {
            h.failures = h.failures.saturating_add(1);
            h.kind = Some(kind);
            let failures = h.failures;
            drop(h);
            if failures.is_power_of_two() {
                observer(&Event::WriteFailed {
                    kind,
                    detail: &detail,
                    failures,
                });
            }
        }
        Wrote::Other => {}
    }
}
