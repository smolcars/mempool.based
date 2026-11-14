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
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::{Connection, ErrorCode, OpenFlags, TransactionBehavior};
use tokio::sync::{Semaphore, mpsc, oneshot, watch};

pub use migrate::MigrationError;
pub use rusqlite::{Error as SqlError, OptionalExtension, Row, Statement, params};

/// The migrations in `migrations/`, compiled in by `build.rs`.
pub(crate) const MIGRATIONS: &[migrate::Migration] =
    include!(concat!(env!("OUT_DIR"), "/migrations.rs"));

/// A write that holds the writer this long is reported as [`Event::SlowWrite`].
const SLOW_WRITE: Duration = Duration::from_secs(1);
/// How long a connection waits for a lock held by another process, such as a backup.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// A table whose writes [`Store::version`] follows, so an answer computed from it can be kept
/// until the table changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Table {
    Blocks,
    Audits,
    MempoolHistory,
}

/// The SQL names of [`Table`]'s tables, in its order.
const TABLES: [&str; 3] = ["blocks", "audits", "mempool_history"];

/// One counter per [`Table`]: odd from the first statement a write prepares on that table until
/// the write has committed or rolled back, even otherwise. Only the writer thread changes them.
#[derive(Debug, Default)]
struct Versions([AtomicU64; 3]);

impl Versions {
    /// Marks `table` as being written. Called by the authorizer, before the statement runs.
    fn begin(&self, table: &str) {
        let Some(v) = TABLES
            .iter()
            .position(|t| *t == table)
            .and_then(|i| self.0.get(i))
        else {
            return;
        };
        let n = v.load(Ordering::SeqCst);
        if n % 2 == 0 {
            v.store(n + 1, Ordering::SeqCst);
        }
    }

    /// Whether the write in progress has changed `table`.
    fn marked(&self, table: Table) -> bool {
        self.0
            .get(table as usize)
            .is_some_and(|v| v.load(Ordering::SeqCst) % 2 == 1)
    }

    /// Ends every open mark, once the write is committed or rolled back.
    fn settle(&self) {
        for v in &self.0 {
            let n = v.load(Ordering::SeqCst);
            if n % 2 == 1 {
                v.store(n + 1, Ordering::SeqCst);
            }
        }
    }
}

/// Something an operator may want recorded. Passed to [`Options::observer`].
/// What a read is for: named when it times out, so the log says which read was slow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Read {
    /// Block rows by height, hash or page.
    Blocks,
    /// The block table's row and rated counts: a full scan.
    BlockCounts,
    /// The newest rows, compared with a new tip.
    BlockFollow,
    /// The backfill's own reads: gaps, the highest row, the next unrated rows.
    Backfill,
    /// Mining pages: pools, hashrate, difficulty.
    Mining,
    Audit,
    MempoolHistory,
    Replacements,
    /// The fee score's samples and blocks, read once at start.
    FeeScore,
    /// The Lightning routes' reads.
    Lightning,
    /// The grouped read of every channel that the Lightning index is built with.
    LightningIndex,
    /// The background walk of channels whose funding txid is still to be asked.
    LightningFunding,
    /// The rows that show which chain a file from before `chain` was written for.
    Chain,
}

/// How many kinds of [`Read`] there are: one failure count each.
const READS: usize = Read::Chain as usize + 1;

impl Read {
    pub fn label(self) -> &'static str {
        match self {
            Self::Blocks => "blocks",
            Self::BlockCounts => "block_counts",
            Self::BlockFollow => "block_follow",
            Self::Backfill => "backfill",
            Self::Mining => "mining",
            Self::Audit => "audit",
            Self::MempoolHistory => "mempool_history",
            Self::Replacements => "replacements",
            Self::FeeScore => "fee_score",
            Self::Lightning => "lightning",
            Self::LightningIndex => "lightning_index",
            Self::LightningFunding => "lightning_funding",
            Self::Chain => "chain",
        }
    }
}

#[derive(Debug)]
pub enum Event<'a> {
    /// A migration was applied while opening.
    MigrationApplied { name: &'a str, elapsed: Duration },
    /// One write held the writer for at least a second. `queue_depth` writes were waiting.
    SlowWrite {
        elapsed: Duration,
        queue_depth: usize,
    },
    /// A read ran past [`Options::read_time_limit`] and was stopped. `read` says which.
    ReadTimedOut { read: Read, limit: Duration },
    /// A write failed in SQLite, at the 1st, 2nd, 4th… failure in a row. `kind` is from
    /// [`failure_kind`]; `detail` is SQLite's own message, for the log and never for a client.
    WriteFailed {
        kind: &'static str,
        detail: &'a str,
        failures: u32,
    },
    /// A write committed after `failures` in a row had failed.
    WriteRecovered { failures: u32 },
    /// A read of kind `read` failed in SQLite or panicked, at the 1st, 2nd, 4th… failure of that
    /// kind in a row. `kind` is from [`failure_kind`], or `panicked`; `detail` is SQLite's own
    /// message (empty for a panic), for the log and never for a client. A timeout is
    /// [`Event::ReadTimedOut`] instead.
    ReadFailed {
        read: Read,
        kind: &'static str,
        detail: &'a str,
        failures: u32,
    },
    /// A read of kind `read` worked after `failures` in a row had failed.
    ReadRecovered { read: Read, failures: u32 },
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
    /// The genesis hash of the chain the file is for, in internal order. When set, opening a
    /// file written for another chain fails with [`OpenError::WrongChain`], and writes wait for
    /// [`Store::set_gate`] to open. When unset, nothing is checked and writes go through.
    pub genesis: Option<[u8; 32]>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            readers: 4,
            queue: 64,
            read_time_limit: Duration::from_secs(2),
            observer: Arc::new(|_| {}),
            genesis: None,
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
    /// The file was written for another chain, whose genesis hash is `genesis` (internal
    /// order). Nothing is changed: move the file aside, or point at the right network's files.
    WrongChain { path: PathBuf, genesis: Vec<u8> },
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
            Self::WrongChain { path, genesis } => {
                let hex: String = genesis.iter().rev().map(|b| format!("{b:02x}")).collect();
                write!(
                    f,
                    "{} was written for another chain (genesis {hex}); nothing was changed: move it out of the data directory, or point the server at that chain",
                    path.display()
                )
            }
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
    /// The write was refused by the gate ([`Store::set_gate`]): the node was found on another
    /// chain while it waited, or the file belongs to another chain. Nothing was written.
    Held,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sql(e) => write!(f, "sqlite: {e}"),
            Self::Full => f.write_str("store write queue is full"),
            Self::Closed => f.write_str("store is closed"),
            Self::TimedOut => f.write_str("store read timed out"),
            Self::Panicked => f.write_str("store closure panicked"),
            Self::Held => f.write_str("store writes are held: the node's chain is not confirmed"),
        }
    }
}

impl std::error::Error for Error {}

impl Error {
    /// A fixed label for an event: [`failure_kind`] for SQLite's errors (`constraint` for a
    /// constraint), else `full`, `closed`, `timed_out`, `panicked` or `held`.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Sql(e) => failure_kind(e).unwrap_or("constraint"),
            Self::Full => "full",
            Self::Closed => "closed",
            Self::TimedOut => "timed_out",
            Self::Panicked => "panicked",
            Self::Held => "held",
        }
    }
}

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

/// Writer work. The flag is set while caller or migration SQL runs, to refuse any commit. Once
/// the transaction has ended, the job settles the versions and reports how it ended before it
/// wakes its caller, so both are current when `write` returns.
type Job = Box<dyn FnOnce(&mut Connection, &AtomicBool, &Versions, &dyn Fn(Wrote)) + Send>;

/// How a job ended, for [`WriteHealth`].
enum Wrote {
    Committed,
    /// SQLite refused it: the [`failure_kind`] and SQLite's message.
    Failed(&'static str, String),
    /// The caller's own error, a constraint or a panic: nothing about the store.
    Other,
}

/// Whether writes may commit. The server sets it from its checks of the node, so that nothing
/// from another chain is written into a file named for this one (docs/STORE.md, "chain").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// The node is not confirmed yet. Writes wait, and commit once the gate opens.
    Waiting,
    Open,
    /// The node was found on another chain. Writes wait; once the gate opens again, those that
    /// were waiting fail with [`Error::Held`], since they carry what was read meanwhile.
    Held,
    /// For as long as the process runs: the file belongs to another chain. Every write fails.
    Refused,
}

/// One network's SQLite file. Share it behind an `Arc`.
pub struct Store {
    queue: mpsc::Sender<Job>,
    /// The [`Gate`], and its [`Store::generation`].
    gate: watch::Sender<(Gate, u64)>,
    /// Block rows from before the file recorded its chain: see [`Store::unstamped`].
    unstamped: AtomicBool,
    health: Arc<Mutex<WriteHealth>>,
    writer: JoinHandle<()>,
    readers: Arc<Readers>,
    /// Failed reads in a row, per [`Read`], for [`Event::ReadFailed`].
    read_failures: Arc<[AtomicU32; READS]>,
    options: Options,
    versions: Arc<Versions>,
    /// Tests: reads of this kind time out at once.
    #[cfg(test)]
    pub(crate) time_out: Mutex<Option<Read>>,
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

    pub(crate) fn open_with(
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
        temp_in_memory(&conn)?;
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
        // Delete triggers also fire for the rows `INSERT OR REPLACE` removes: the block table's
        // totals must hear of every row that leaves it.
        conn.pragma_update(None, "recursive_triggers", true)?;
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
        // The authorizer also sees every table a statement will change, before it runs: that
        // marks the table's version (`Store::version`), whatever form the change takes.
        let versions = Arc::new(Versions::default());
        let marks = versions.clone();
        conn.authorizer(Some(move |ctx: AuthContext<'_>| match ctx.action {
            AuthAction::Transaction { .. } | AuthAction::Savepoint { .. }
                if gate.load(Ordering::Relaxed) =>
            {
                Authorization::Deny
            }
            AuthAction::Insert { table_name }
            | AuthAction::Delete { table_name }
            | AuthAction::Update { table_name, .. } => {
                marks.begin(table_name);
                Authorization::Allow
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
        versions.settle();
        let mut unstamped = false;
        if let Some(genesis) = options.genesis {
            let found: Option<Vec<u8>> = conn
                .query_row("SELECT genesis FROM chain", [], |r| r.get(0))
                .optional()?;
            match found {
                Some(found) if found != genesis => {
                    return Err(OpenError::WrongChain {
                        path,
                        genesis: found,
                    });
                }
                Some(_) => {}
                // Rows from before the file recorded its chain are checked against the node
                // before it is recorded; an empty file has nothing to check.
                None if conn
                    .query_row("SELECT EXISTS (SELECT 1 FROM blocks)", [], |r| r.get(0))? =>
                {
                    unstamped = true;
                }
                None => {
                    conn.execute(
                        "INSERT INTO chain (id, genesis) VALUES (0, ?1)",
                        [&genesis[..]],
                    )?;
                }
            }
        }
        let gate = if options.genesis.is_some() {
            Gate::Waiting
        } else {
            Gate::Open
        };

        let mut idle = Vec::with_capacity(options.readers);
        for _ in 0..options.readers.max(1) {
            let reader = Connection::open_with_flags(
                &path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            temp_in_memory(&reader)?;
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
        let settled = versions.clone();
        let writer = std::thread::Builder::new()
            .name(format!("store-{network}"))
            .spawn(move || {
                while let Some(job) = jobs.blocking_recv() {
                    let start = Instant::now();
                    job(&mut conn, &sealed, &settled, &|wrote| {
                        record(&seen, wrote, &observer);
                    });
                    // Settled again in case a job ended without reaching its own settle.
                    settled.settle();
                    let elapsed = start.elapsed();
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
            gate: watch::Sender::new((gate, 0)),
            unstamped: AtomicBool::new(unstamped),
            health,
            writer,
            readers,
            read_failures: Arc::default(),
            options,
            versions,
            #[cfg(test)]
            time_out: Mutex::new(None),
        })
    }

    /// Runs `f` in one transaction on the writer, after the writes queued before it. Resolves
    /// once the transaction is committed and on disk; an error or a panic rolls it back. Waits
    /// for room when the queue is full: that wait is the back-pressure.
    /// Waits while the [`Gate`] is not open, and fails with [`Error::Held`] when it refuses.
    pub async fn write<T, F>(&self, f: F) -> Result<T, Error>
    where
        F: FnOnce(&Sql) -> Result<T, Error> + Send + 'static,
        T: Send + 'static,
    {
        self.write_from(self.generation(), f).await
    }

    /// [`Store::write`] for data read at `generation` ([`Store::generation`]): it commits only if
    /// the gate has not been held since. For a caller that keeps data waiting in a queue of its
    /// own, which a hold could otherwise let through once the gate reopens.
    pub async fn write_from<T, F>(&self, generation: u64, f: F) -> Result<T, Error>
    where
        F: FnOnce(&Sql) -> Result<T, Error> + Send + 'static,
        T: Send + 'static,
    {
        let f = self.pass(generation, f).await?;
        self.enqueue(f).await
    }

    async fn enqueue<T, F>(&self, f: F) -> Result<T, Error>
    where
        F: FnOnce(&Sql) -> Result<T, Error> + Send + 'static,
        T: Send + 'static,
    {
        let (job, done) = job(f);
        self.queue.send(job).await.map_err(|_| Error::Closed)?;
        done.await.map_err(|_| Error::Closed)?
    }

    /// Waits for the gate to open, then wraps `f` so that it fails if the gate has closed by the
    /// time the writer runs it. Data read at another generation fails: it was read while the
    /// node was on another chain, or before a hold that a write cannot see past.
    async fn pass<T, F>(
        &self,
        generation: u64,
        f: F,
    ) -> Result<impl FnOnce(&Sql) -> Result<T, Error> + Send + 'static, Error>
    where
        F: FnOnce(&Sql) -> Result<T, Error> + Send + 'static,
        T: Send + 'static,
    {
        let mut gate = self.gate.subscribe();
        let now = *gate
            .wait_for(|(g, _)| matches!(g, Gate::Open | Gate::Refused))
            .await
            .map_err(|_| Error::Closed)?;
        if now != (Gate::Open, generation) {
            return Err(Error::Held);
        }
        Ok(move |sql: &Sql| {
            if *gate.borrow() != (Gate::Open, generation) {
                return Err(Error::Held);
            }
            f(sql)
        })
    }

    /// The gate's state.
    pub fn gate(&self) -> Gate {
        self.gate.borrow().0
    }

    /// A number that moves when the gate is held or refused, and again when a hold ends, but not
    /// when it first opens. Data read at one generation is written only at that generation
    /// ([`Store::write_from`]), so nothing read while the node was on another chain is written.
    pub fn generation(&self) -> u64 {
        self.gate.borrow().1
    }

    /// Opens, holds or refuses writes. [`Gate::Refused`] is final.
    pub fn set_gate(&self, to: Gate) {
        self.gate.send_if_modified(|(g, generation)| {
            if *g == to || *g == Gate::Refused {
                return false;
            }
            if matches!(to, Gate::Held | Gate::Refused) || *g == Gate::Held {
                *generation = generation.wrapping_add(1);
            }
            *g = to;
            true
        });
    }

    /// Whether the file holds block rows written before it recorded its chain, so the caller
    /// must check them against the node and then call [`Store::stamp`].
    pub fn unstamped(&self) -> bool {
        self.unstamped.load(Ordering::Relaxed)
    }

    /// Records the chain given in [`Options::genesis`] in a file whose rows the caller has
    /// checked. Not held by the gate.
    pub async fn stamp(&self) -> Result<(), Error> {
        let Some(genesis) = self.options.genesis else {
            return Ok(());
        };
        self.enqueue(move |sql| {
            sql.prepare("INSERT OR IGNORE INTO chain (id, genesis) VALUES (0, ?1)")?
                .execute([&genesis[..]])?;
            Ok(())
        })
        .await?;
        self.unstamped.store(false, Ordering::Relaxed);
        Ok(())
    }

    /// Like [`Store::write`], but fails with [`Error::Full`] at once when the queue is full,
    /// for callers that would rather drop a sample than wait. It still waits for the gate.
    pub async fn try_write<T, F>(&self, f: F) -> Result<T, Error>
    where
        F: FnOnce(&Sql) -> Result<T, Error> + Send + 'static,
        T: Send + 'static,
    {
        self.try_write_from(self.generation(), f).await
    }

    /// [`Store::try_write`] for data read at `generation`, as [`Store::write_from`].
    pub async fn try_write_from<T, F>(&self, generation: u64, f: F) -> Result<T, Error>
    where
        F: FnOnce(&Sql) -> Result<T, Error> + Send + 'static,
        T: Send + 'static,
    {
        let (job, done) = job(self.pass(generation, f).await?);
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

    /// A number that changes whenever any of `tables` changes, or `None` while a write to one
    /// of them is open. An answer computed from these tables after reading `Some(v)` is still
    /// what a fresh read would give for as long as this returns `Some(v)`: a write marks its
    /// tables before it changes them and settles them only after it has committed.
    pub fn version(&self, tables: &[Table]) -> Option<u64> {
        let mut sum = 0u64;
        for t in tables {
            let n = self.versions.0.get(*t as usize)?.load(Ordering::SeqCst);
            if n % 2 == 1 {
                return None;
            }
            sum = sum.wrapping_add(n);
        }
        Some(sum)
    }

    /// Writes waiting for the writer.
    pub fn queue_depth(&self) -> usize {
        self.queue.max_capacity() - self.queue.capacity()
    }

    /// Runs `f` on a read-only connection on tokio's blocking pool, so a slow query never holds
    /// a runtime thread. Waits for a free connection. Once `f` has run for
    /// [`Options::read_time_limit`], its running statement and any later one fail, and the read
    /// returns [`Error::TimedOut`]; this holds even if the caller stops waiting.
    pub async fn read<T, F>(&self, read: Read, f: F) -> Result<T, Error>
    where
        F: FnOnce(&Sql) -> Result<T, Error> + Send + 'static,
        T: Send + 'static,
    {
        #[cfg(test)]
        if *self.time_out.lock().unwrap_or_else(|e| e.into_inner()) == Some(read) {
            (self.options.observer)(&Event::ReadTimedOut {
                read,
                limit: self.options.read_time_limit,
            });
            return Err(Error::TimedOut);
        }
        let permit = self
            .readers
            .permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::Closed)?;
        let mut conn = self.readers.take().ok_or(Error::Closed)?;
        let readers = self.readers.clone();
        let failures = self.read_failures.clone();
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
                observer(&Event::ReadTimedOut { read, limit });
                return Err(Error::TimedOut);
            }
            record_read(&failures, read, &result, &observer);
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
    let job: Job = Box::new(move |conn, sealed, versions, report| {
        let result: Result<T, Error> = (|| {
            let t = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            sealed.store(true, Ordering::Relaxed);
            let value = catch_unwind(AssertUnwindSafe(|| {
                let sql = Sql {
                    conn: &t,
                    deadline: None,
                };
                let value = f(&sql)?;
                // The block table's totals follow its rows in the same transaction.
                if versions.marked(Table::Blocks) {
                    crate::blocks::settle(&sql)?;
                }
                Ok(value)
            }));
            sealed.store(false, Ordering::Relaxed);
            // Dropping `t` without committing rolls back, on an error and on a panic alike.
            let value = value.unwrap_or(Err(Error::Panicked))?;
            t.commit()?;
            Ok(value)
        })();
        // The transaction has ended: committed, or rolled back when `t` was dropped. Settle the
        // versions before the caller resumes, so a version read after `write` returns is
        // `Some` and already reflects this write.
        versions.settle();
        report(match &result {
            Ok(_) => Wrote::Committed,
            Err(Error::Sql(e)) => {
                failure_kind(e).map_or(Wrote::Other, |kind| Wrote::Failed(kind, e.to_string()))
            }
            Err(_) => Wrote::Other,
        });
        // The caller may have stopped waiting; the write stands either way.
        let _ = tx.send(result);
    });
    (job, rx)
}

/// Keeps SQLite's temporary data (sorts, groupings, transient indexes, statement journals) in
/// memory, so no query needs a writable temporary directory: the image has none
/// (docs/STORE.md, "Temporary data").
fn temp_in_memory(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "temp_store", "memory")
}

/// Counts a read's failures in a row for its kind, and reports them at the 1st, 2nd, 4th…, then
/// the recovery. A timeout has its own event and leaves the count alone.
fn record_read<T>(
    failures: &[AtomicU32; READS],
    read: Read,
    result: &Result<T, Error>,
    observer: &Observer,
) {
    let Some(count) = failures.get(read as usize) else {
        return;
    };
    let (kind, detail) = match result {
        Ok(_) => {
            let failures = count.swap(0, Ordering::Relaxed);
            if failures > 0 {
                observer(&Event::ReadRecovered { read, failures });
            }
            return;
        }
        Err(Error::Sql(e)) => (failure_kind(e).unwrap_or("other"), e.to_string()),
        Err(Error::Panicked) => ("panicked", String::new()),
        Err(_) => return,
    };
    let failures = count.fetch_add(1, Ordering::Relaxed).saturating_add(1);
    if failures.is_power_of_two() {
        observer(&Event::ReadFailed {
            read,
            kind,
            detail: &detail,
            failures,
        });
    }
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
