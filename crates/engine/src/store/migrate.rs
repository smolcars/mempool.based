//! Numbered migrations: checked, then applied in order, each in its own transaction.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use rusqlite::{Connection, TransactionBehavior};

use super::{Event, Observer, OpenError};

/// A migration as compiled in: file name and SQL.
pub(crate) type Migration = (&'static str, &'static str);

/// What is wrong with the list of migration files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationError {
    /// Not `NNNN_lowercase_name.sql`.
    BadName(String),
    /// Two files share a number.
    Duplicate(String),
    /// Numbers must run 1, 2, 3… with no holes; this file is not the next one.
    Gap { expected: u32, file: String },
}

impl fmt::Display for MigrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadName(n) => write!(f, "migration {n:?} is not named NNNN_lowercase_name.sql"),
            Self::Duplicate(n) => write!(f, "migration {n:?} reuses the number of the one before"),
            Self::Gap { expected, file } => {
                write!(
                    f,
                    "migration {file:?} is out of sequence: expected number {expected:04}"
                )
            }
        }
    }
}

fn version(name: &str) -> Option<u32> {
    let rest = name.strip_suffix(".sql")?;
    let (num, label) = rest.split_once('_')?;
    let ok = num.len() == 4
        && num.bytes().all(|b| b.is_ascii_digit())
        && !label.is_empty()
        && label
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if ok { num.parse().ok() } else { None }
}

/// Checks that the files are numbered 1, 2, 3… in order, with no gap and no duplicate.
pub(crate) fn validate(migrations: &[Migration]) -> Result<(), MigrationError> {
    let mut previous = 0;
    for (name, _) in migrations {
        let v = version(name).ok_or_else(|| MigrationError::BadName((*name).into()))?;
        if v == previous {
            return Err(MigrationError::Duplicate((*name).into()));
        }
        if Some(v) != previous.checked_add(1) {
            return Err(MigrationError::Gap {
                expected: previous.saturating_add(1),
                file: (*name).into(),
            });
        }
        previous = v;
    }
    Ok(())
}

#[cfg(test)]
/// FNV-1a, 64 bit: enough to notice an edit, and stable across Rust releases.
fn checksum(sql: &str) -> u64 {
    sql.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
/// Compares the migrations against the `CHECKSUMS` file, so a shipped migration that is edited
/// or removed, or a new one that is not recorded, fails a test.
pub(crate) fn check_checksums(migrations: &[Migration], lock: &str) -> Result<(), String> {
    let mut recorded = lock
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'));
    for (name, sql) in migrations {
        let line = format!("{name} {:016x}", checksum(sql));
        match recorded.next() {
            Some(r) if r == line => {}
            Some(r) if r.split(' ').next() == Some(*name) => {
                return Err(format!(
                    "{name} changed after it shipped. Put the change in a new migration instead."
                ));
            }
            Some(r) => return Err(format!("CHECKSUMS has {r:?} where {line:?} was expected")),
            None => {
                return Err(format!(
                    "{name} is not recorded: append this line to CHECKSUMS:\n{line}"
                ));
            }
        }
    }
    match recorded.next() {
        Some(r) => Err(format!("CHECKSUMS lists {r:?}, but that migration is gone")),
        None => Ok(()),
    }
}

/// Brings the database up to the last migration. Each runs in its own transaction with the new
/// `user_version`, so a failure leaves the database at the version before it.
pub(crate) fn migrate(
    conn: &mut Connection,
    migrations: &[Migration],
    sealed: &AtomicBool,
    observer: &Observer,
) -> Result<(), OpenError> {
    validate(migrations).map_err(OpenError::BadMigrations)?;
    let supported = migrations.len();
    loop {
        // IMMEDIATE takes the write lock before reading the version, so two processes opening
        // the same file cannot both apply the same migration.
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let found: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let current = usize::try_from(found)
            .ok()
            .filter(|v| *v <= supported)
            .ok_or(OpenError::TooNew { found, supported })?;
        let Some((name, sql)) = migrations.get(current) else {
            return Ok(());
        };
        let start = Instant::now();
        let version = current + 1;
        // Sealed through the version bump: if the file ended the transaction itself (COMMIT,
        // ROLLBACK), the bump would autocommit alone, so the commit hook refuses it instead.
        sealed.store(true, Ordering::Relaxed);
        let applied = tx
            .execute_batch(sql)
            .and_then(|()| tx.pragma_update(None, "user_version", version as i64));
        sealed.store(false, Ordering::Relaxed);
        applied.map_err(|source| OpenError::Migration { name, source })?;
        tx.commit()?;
        observer(&Event::MigrationApplied {
            name,
            elapsed: start.elapsed(),
        });
    }
}
