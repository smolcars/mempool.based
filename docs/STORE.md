# Store

One SQLite file per network, `<DATA_DIR>/<network>.sqlite`, for the little data the node does not keep: per-block mining summaries, mempool history, block audits, Lightning graph history, prices. rbitcoin stays the source of truth for blocks, transactions, addresses and spends ([`ARCHITECTURE.md`](ARCHITECTURE.md#storage)). The code is `crates/engine/src/store/`.

## Rules

1. **Store only what we derive or observe.** Never a copy of something the node can answer.
2. **One writer.** Every write goes through `Store::write` to the single writer thread. Nothing else holds a read-write connection. The store owns each write's transaction: `BEGIN`, `COMMIT`, `ROLLBACK` and savepoint statements in a write are refused by an authorizer, and a commit hook turns any other commit from inside it (an autocommit after `INSERT OR ROLLBACK` ended the transaction) into a rollback. The write then fails and leaves nothing behind.
3. **Readers cannot write.** `Store::read` runs on read-only connections that also have `query_only` set.
4. **SQL is a `&'static str`.** `Sql::prepare` takes only a string literal (or `concat!` of literals), so a query cannot be built from request data with `format!`. Values are bound as parameters with `params![…]` ([`SECURITY.md`](SECURITY.md) rule 12). A variable-length list is one parameter: a JSON array read with `json_each(?1)`, not a generated `IN (?, ?, …)`.
5. **A shipped migration never changes.** Fix it with a new one.
6. **The engine does not log** ([`LOGGING.md`](LOGGING.md) rule 6). It returns errors and calls `Options::observer` with an `Event`; the server turns those into log events.

## Using it

```rust
let store = Store::open(&data_dir, "mainnet", Options::default())?; // blocking: at startup

// A write is one transaction. It resolves once committed and on disk; Err or a panic rolls back.
store.write(move |sql| {
    let mut insert = sql.prepare("INSERT INTO block_summaries (height, fees) VALUES (?1, ?2)")?;
    for b in &blocks {
        insert.execute(params![b.height, b.fees])?;
    }
    Ok(())
}).await?;

// A read runs on tokio's blocking pool and is interrupted past Options::read_time_limit.
let fees: Option<i64> = store.read(move |sql| {
    Ok(sql.prepare("SELECT fees FROM block_summaries WHERE height = ?1")?
        .query_row(params![height], |r| r.get(0))
        .optional()?)
}).await?;
```

- **Batch writes.** One `write` per row pays one commit, and one fsync, per row. Put thousands of rows in one call: a backfill inserts in batches of about 10,000. Keep a single write under a second or so, because every other write waits behind it; a longer one is reported as `Event::SlowWrite`.
- **Back-pressure.** `write` waits when `Options::queue` writes (64) are already waiting. `try_write` fails with `Error::Full` instead, for a producer that would rather drop a sample than fall behind. `queue_depth()` says how many are waiting.
- **Reads.** At most `Options::readers` (4) reads run at once; more wait for a free connection. Once a read has run for `read_time_limit` (2 s), its running statement and any later one are stopped and it returns `Error::TimedOut`. The deadline is enforced on the blocking thread: SQLite's progress handler stops a long statement, and `Sql::prepare` refuses new ones, so it holds across statements and after the caller stops waiting. It bounds SQL, not Rust work inside the closure. Do not call `Statement::reset_status` on SQLite's VM-step counter in a read: the progress handler counts on it. A read may use `BEGIN` for a consistent snapshot across statements; one left open is rolled back before the connection is reused. Reads see the last committed write, never one in progress.
- **A queued write cannot be cancelled.** Dropping the future of a `write` that is already queued does not stop it; it still commits.
- **Errors stay inside.** A store error can contain SQL text. Never send it to a client ([`SECURITY.md`](SECURITY.md) rule 17): answer `500` and report it.
- **Closing.** `close().await` lets queued writes finish, then closes the file. Dropping the store does the same in the background.

## Adding a migration

1. Add `crates/engine/src/store/migrations/NNNN_short_name.sql`, numbered one past the last (`0001_block_summaries.sql`, `0002_…`). Lowercase letters, digits and underscores.
2. Run `cargo test -p engine store`. It fails and prints the line to append to `migrations/CHECKSUMS`. Append it.
3. That is all: `build.rs` compiles every `.sql` file in the directory into the binary, in name order.

What happens at open: each migration past the file's `user_version` runs in its own transaction together with the new `user_version`, so it applies exactly once, and a failure leaves the database at the version before it. Two processes opening the same file at once cannot both apply one, because the version is read under the write lock.

What fails loudly:

| Mistake | Where it fails |
|---|---|
| A gap (`0001`, `0003`), a duplicate number, or a bad name | `validate` in the tests, and `Store::open` refuses with `OpenError::BadMigrations` |
| A new file without its `CHECKSUMS` line | `shipped_migrations_are_numbered_and_unchanged` |
| An edited or deleted shipped migration | The same test: the checksum no longer matches. Changing an existing `CHECKSUMS` line in review means a shipped migration was edited |
| A database written by a newer build | `Store::open` refuses with `OpenError::TooNew` and leaves the schema alone |

Inside a migration: no `BEGIN` or `COMMIT` (it already runs in a transaction; they are refused and the open fails), and nothing that cannot run in one (`VACUUM`, `PRAGMA journal_mode`). SQLite's `ALTER TABLE` is limited; to reshape a table, create the new one, copy, drop and rename in the same migration.

## Pragmas

| Connection | Pragma | Why |
|---|---|---|
| file | `journal_mode=WAL` | Readers never block the writer and the writer never blocks readers. Stored in the file. Open fails with `OpenError::NoWal` on a filesystem that cannot do it |
| writer | `synchronous=FULL` | The WAL is fsynced at every commit, so a write that `write` acknowledged survives a crash and a power cut. The tests prove the crash half (a child process is killed mid-transaction); power loss rests on SQLite's documented `FULL` guarantee and a disk that honours fsync. With one writer and batched writes this costs little; `NORMAL` would lose the last commits on power loss |
| writer | `journal_size_limit=64 MiB` | The WAL is truncated back to this after a checkpoint, so one bulk insert does not leave a huge file behind |
| writer | `foreign_keys=ON` | SQLite ignores `REFERENCES` without it |
| every | `busy_timeout=5 s` | Only another process takes locks (a backup with the `sqlite3` tool); wait for it instead of failing |
| readers | `query_only=ON`, opened `SQLITE_OPEN_READ_ONLY` | Two independent guards against a write from a request handler |

Cache size and the auto-checkpoint interval stay at SQLite's defaults (2 MiB, 1,000 pages); see the measurements below for why.

Opening also runs `PRAGMA quick_check`: a file that is not a database, or whose pages are damaged, is refused with `OpenError::Corrupt` naming the file. It takes about 0.2 s on a million-row file.

## Backup and recovery

The database is one file plus, while the server runs, its `-wal` and `-shm` companions. **Copying the `.sqlite` file alone while the server runs can produce a corrupt copy**, because recent commits live in the WAL. Take a consistent copy while it runs with the `sqlite3` tool:

```sh
sqlite3 data/mainnet.sqlite "VACUUM INTO 'backup/mainnet.sqlite'"
```

`VACUUM INTO` writes a compacted copy from one read snapshot and does not block the server's writes. `.backup` works too. With the server stopped, the three files can be copied as they are (after a clean stop the WAL is checkpointed and removed).

To restore, stop the server and put the copy in place, removing any old `-wal` and `-shm` files. To recover from a corrupt file, restore a backup or delete it.

**Deleting the file** loses only what was observed live: mempool history, first-seen times, block audits, and any other history a later feature records as it happens. Everything derived from the chain, such as block summaries, is rebuilt from the node by backfill. Each feature's doc says which of its tables are which.

## Measurements

Apple M4 (10 cores), APFS SSD, release build, while other builds ran on the same machine (load average 7 to 17). Reproduce with:

```sh
cargo test --release -p engine -- --ignored --nocapture bulk_insert_benchmark
```

**Bulk insert.** One million rows shaped like a block summary (height as the key, a 64-character hex hash with a unique index, nine integers, two short texts), in writes of 10,000 rows, with two readers running flat out:

| | Result |
|---|---|
| Throughput | 17,000 to 73,000 rows/s over 28 runs, median 38,000: a million rows in 14 to 59 s |
| Slowest 10,000-row write | 0.2 to 11 s; usually under 2.5 s |
| File size | 210 MiB, about 220 bytes a row with its hash index |
| Reopen with `quick_check` | 0.18 to 0.28 s |

The spread is the shared disk, not the code: the same build gave 17,000 and 71,000 rows/s in consecutive runs. With `synchronous=OFF` and no readers, one run reached 87,000 rows/s, so the time goes to fsync at commits and checkpoints. A 64 MiB writer cache with a 10,000-page checkpoint interval was no better than the defaults in an interleaved A/B (four runs each, both 17,000 to 71,000), so the defaults stay.

**Reads during that insert,** each on a random height among the rows committed so far:

| Query | Median | p99 | Worst |
|---|---|---|---|
| Lookup by hash (unique index) | 11 µs | 40 to 70 µs | 170 to 330 ms |
| 100-row range scan by height | 20 µs | 60 to 110 µs | 170 to 330 ms |

Each worst case hit both readers at the same moment, so it was one machine-wide stall, not a slow query. Readers never wait for the writer.
