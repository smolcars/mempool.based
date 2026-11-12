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
    let mut insert = sql.prepare("INSERT INTO blocks (height, fees) VALUES (?1, ?2)")?;
    for b in &blocks {
        insert.execute(params![b.height, b.fees])?;
    }
    Ok(())
}).await?;

// A read runs on tokio's blocking pool and is interrupted past Options::read_time_limit.
let fees: Option<i64> = store.read(move |sql| {
    Ok(sql.prepare("SELECT fees FROM blocks WHERE height = ?1")?
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

1. Add `crates/engine/src/store/migrations/NNNN_short_name.sql`, numbered one past the last (`0001_blocks.sql`, `0002_…`). Lowercase letters, digits and underscores.
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

## Tables

| Table | Migration | Holds | Rebuildable |
|---|---|---|---|
| [`blocks`](#blocks) | `0001_blocks.sql`, `0002_pools.sql` (the `pool` column) | One row per block on the best chain | Yes, by backfill |
| `pool_list` | `0002_pools.sql` | One row: the version of the pool list and rules the `pool` column was last fully computed with | Yes: a missing or other version redoes the column |
| [`audits`](#audits) | `0003_audits.sql` | One row per block we watched arrive: how it compared with the block we expected | **No:** observed live |
| [`mempool_history`](#mempool_history) | `0004_mempool_history.sql` | The mempool sampled once a minute, rolled into hours and days as it ages | No: observed live, lost with the file |
| [`replacements`](#replacements) | `0005_replacements.sql` | The replacement record: what replaced what, as the mempool tracker saw it | No: observed live, lost with the file |

### Adding a table

Follow `blocks` (`crates/engine/src/blocks/`):

1. **The migration:** the table, its indexes and a comment on each column, as above. Prefer `STRICT`. Every column earns its place by a use that is planned; a later need gets a later migration.
2. **A module in `crates/engine/`** owning every statement on the table: static SQL in one file (`db.rs`), typed reads for the tasks that use it (`async fn` taking `&Store`, returning plain structs), and the writers. Nothing outside it writes SQL for the table.
3. **I/O through a trait** (`Source`) with a real implementation over the node clients and a mock in the tests, so the logic is tested without a node.
4. **Report, don't log:** the engine returns errors and calls an observer with typed events; the server turns them into log events (`crates/server/src/blocks.rs`).
5. **Say here** which of its data is rebuildable and which is lost with the file.

### blocks

One row per block on the best chain: what the mining dashboard and the history charts (fees, rewards, fee rates, sizes, block times, difficulty, pools) need for any period without asking the node about each block. The code is `crates/engine/src/blocks/`.

| Column | Type | Why |
|---|---|---|
| `height` | integer key | Position on the best chain; every range and page is by height |
| `hash` | 32-byte blob, unique | Lookups by hash (the block facts route) and the reorg check. The bitcoin crate's internal byte order, the reverse of the printed hash |
| `time` | integer, indexed | Header timestamp: per-day buckets, block intervals, and periods asked by time |
| `tx_count` | integer | Transactions per block and per period; the coinbase included |
| `size`, `weight` | integer | Block size charts and fullness |
| `paying_weight` | integer | The weight of every transaction but the coinbase, which the block facts route serves as `weight` |
| `fees`, `subsidy`, `reward` | integer, sats | Fee and reward charts. `reward` is the coinbase's outputs summed: what the miner took, which can be less than subsidy plus fees |
| `difficulty` | real | Difficulty and hashrate charts (hashrate ≈ difficulty × 2³² / interval) |
| `coinbase_tag` | blob, ≤ 100 bytes | The coinbase input's script, for pool attribution by tag. Consensus caps it at 100 bytes |
| `payouts` | blob | The coinbase's outputs paying more than zero, at most 8, each at most 128 bytes, as a length byte and the script: pool attribution by payout address. Longer scripts are no address and are left out |
| `rated` | integer, 0 or 1 | Completeness: 1 once the columns below are known |
| `fee_min` … `fee_max` | real, nullable | Size-weighted fee-rate percentiles in sat/vB, unrounded (min, p10, p25, median, p75, p90, max). Null until rated, and for a rated block that holds only its coinbase |
| `band0` … `band9` | integer, nullable | Virtual size paying in each of the design's ten fee bands; they sum to the paying transactions' virtual size |
| `pool` | integer, indexed | Who mined the block, by the rules in [`API.md`](API.md#mining): the pool list's id, 0 for Unknown. Set when the row is written. The index gives a pool's blocks by height |

The coinbase tag and scripts are written by whoever mined the block. They are stored as bytes, bounded as above, and never turned into text here; a page renders them as text ([`SECURITY.md`](SECURITY.md) rule 14). Pool attribution reads them only as bytes to compare with the pool list.

**The pool column follows the list.** `blocks::reattribute` compares `pool_list.version` (a hash of the list file and a rules number) with the build's; when they differ, as on the first start after `0002_pools.sql` or with a new list, it recomputes every row's pool from its stored coinbase in writes of 10,000 rows, newest first, then records the version: 1.8 s for 975,000 rows, no node call. Rows written meanwhile already use the build's list, and a row replaced between the read and the write keeps its new pool (the update is keyed by height and hash). The mining routes wait for it.

**Completeness.** A row exists only once its totals and coinbase are known, so every row has everything above `rated`. `rated = 0` means the fee rates are still to come; a partial index on those rows lets the backfill find the next one at once. A height with no row is a block not yet backfilled.

**Size.** About 300 bytes a row with its indexes: 295 MB for a table filled to mainnet's 975,000 blocks.

**Rebuildable.** Everything here is derived from the node. Deleting the file loses nothing the backfill cannot redo.

#### Writers

| Writer | Heights | Does |
|---|---|---|
| `follow` | the newest 100 (`ZONE`) | On every new tip: reads Esplora's headers for the zone and compares each hash with the table. A row whose hash differs is stale; it is replaced, and every row above the tip is deleted, in one write. Below the zone it also checks the highest stored row, jumping any heights not stored yet (after downtime, say), and walks down while pages of ten headers hold stale rows, jumping gaps the same way, so a reorg of any depth is repaired in one call at one page more than its stale rows. It reads the tip again at the end and writes nothing if it moved, so every page comes from one chain. Missing rows in the zone are added; replacements are read 50 blocks a batch. The newest 10 get their fee rates from the summaries the server computes for every new block anyway, so a new block costs the node one `getblockstats` and a coinbase lookup more, nothing heavier |
| `backfill` | below the zone | Adds rows the table lacks and rates rows that lack fee rates, newest first. Never overwrites a row. Current rows must never land above stale ones, since `follow`'s walk stops at the first page that agrees and would never reach them. So the backfill starts only after `follow` has succeeded once; drops a batch read before any `follow` write that replaced rows; and before writing a batch checks the highest stored row below it against the node (by the batch's parent hash when adjacent, else one page of headers). A stale one is deleted instead, and the next steps check the rows under it the same way and refill the heights |

Fee rates are written with `UPDATE … WHERE height = ? AND hash = ?`, so rates computed for a block that has since left the chain never land on its replacement.

#### Backfill

Two tiers, one step of each in turn:

| Tier | Step | Node calls per step |
|---|---|---|
| Totals | one run of up to 50 consecutive missing heights, highest first, so the row checked below it is the only one beneath it | Esplora `/blocks/:height` (ten blocks each), one batched `getblockstats` asking only `totalfee`, `subsidy` and `total_weight`, Esplora `/block/:hash/txid/0` per block, one batched `getrawtransaction` for the coinbases |
| Rates | the highest unrated row within `BACKFILL_FEE_RATE_DEPTH` of the tip | One `getblock` at verbosity 2, through the same statistics code as the block facts route |

**Gentle by construction.** The backfill and `follow` share their own RPC client with one slot, so they never hold one of the four slots the mempool tracker and the summary routes share, and the node sees at most one RPC call from them at a time. Their Esplora requests are small (ten headers, one txid) and each task makes one at a time, so at most two are in flight. After each step the backfill waits so that it keeps that call busy for at most `BACKFILL_BUSY_PERCENT` of the time (50 by default). A failure of any kind, the node's 503 (`busy`) included, pauses it for 5 s, doubling up to 5 minutes, and is logged at the 1st, 2nd, 4th, 8th… in a row.

**Resumable.** The table is the progress. Each pass starts at the tip and reads the table 10,000 heights at a time looking for missing rows (about 100 reads, well under a second, for a full table), so a restart or a pause (`BACKFILL_ENABLED=false`, then `true`) costs those reads and nothing else. The tip is checked every minute, and a new one restarts the pass from the top, so new blocks below the zone are filled first.

**Observable.** `block_backfill_progress` every minute ([`LOGGING.md`](LOGGING.md)), and the same numbers in `BlockTable::progress` for a status page.

**Cost on mainnet,** measured with `cargo run --release -p engine --example blocks_probe` against a synced rbitcoin v0.9.2 over a private network, paced at 50%, on bounded ranges of each era. Time per block is the node calls' wall time including the round trips; at 50% busy, wall time is twice that.

| Tier | Blocks around | Node time a block | Read a block |
|---|---|---|---|
| Totals | 0 to 75,000 (2009–10) | 7 to 10 ms | 0.9 KB |
| Totals | 225,000 (2013) | 12 to 14 ms | 0.9 KB |
| Totals | 375,000 to 525,000 (2015–18) | 12 to 22 ms | 1.1 KB |
| Totals | 675,000 (2021) | 21 to 25 ms | 1.3 KB |
| Totals | 862,000 to the tip (2024–26) | 32 to 35 ms | 1.3 to 1.4 KB |
| Rates | 75,000 | 38 ms | 5 KB |
| Rates | 225,000 to 375,000 | 0.2 to 0.45 s | 2 to 4 MB |
| Rates | 525,000 | 1 s | 4.7 MB |
| Rates | 675,000 | 1.6 s | 9.2 MB |
| Rates | 862,000 to the tip | 0.8 to 2.2 s | 10.5 MB |

For all of mainnet (975,000 blocks), extrapolated era by era:

| Work | Node time | Read | At the default 50% |
|---|---|---|---|
| Totals for every block | 5.7 h | 1.2 GB | 11.4 h alone |
| Fee rates for a year (52,560 blocks, the default) | 13.4 h | 550 GB | 27 h alone |
| Both, interleaved as the backfill runs them | 19 h | 550 GB | totals complete after about 21 h, everything after about 38 h |
| Fee rates for all of history | about 9.7 days | | why the default is a year |

**What it costs the node per hour** at 50%: one call from the backfill for half the hour, so 30 minutes of one of the node's 16 RPC work slots, and nothing while the other four shared slots are busy, since it never uses them. While it rates recent blocks it reads about 7 GB an hour (one 10 MB `getblock` every 5 s), most of it the node's JSON encoding; once the year is rated, about 80 MB an hour, and when caught up, one `getblockstats` and a coinbase lookup per new block. Old blocks' totals are mostly round trips: `/block/:hash/txid/0` is one request per block, about 8 ms over the network measured and far less on the node's own host.

Unfiltered `getblockstats` took about 1 s per recent block (its UTXO statistics), forty times the filtered call; the backfill never asks for them ([`RBITCOIN.md`](RBITCOIN.md)).

#### Reading

`crates/engine/src/blocks/db.rs`, each an `async fn` on a read-only connection:

| Call | Returns | 975,000 rows, the last year rated |
|---|---|---|
| `by_height(store, h)`, `by_hash(store, hash)`, `by_hashes(store, hashes)` | `Row`: every column, `rates: Option<Rated>` for completeness, `Row::facts()` in the block facts route's shape | 0.04 to 0.15 ms |
| `page(store, below, limit)` | Up to `limit` (at most 1,000) rows below a height, newest first; missing heights skipped | 0.1 to 0.3 ms for 15 or 100 rows, at the top or deep in history |
| `heights_between(store, from, to)` | The heights timestamped in a period, through the `time` index | 1.7 to 2.6 ms for a year |
| `periods(store, from, to, Period::Day \| Period::Blocks(n))` | One `Bucket` per UTC day or per `n` heights, oldest first: block count, first and last height and time, sums of transactions, size, weight, fees, subsidy and reward, mean difficulty, and mean fee-rate percentiles over the rated blocks | 31 to 42 ms for a year by day; 0.5 to 0.8 s for all of history in 2,016-block buckets |

For the mining routes, `crates/engine/src/blocks/mining.rs` (and `pool_page` in `db.rs`):

| Call | Returns | 975,000 rows |
|---|---|---|
| `span(store)` | The tip and the lowest height from which every row up to it is stored | 64 ms (walks down to the first gap) |
| `row_counts(store)` | Rows, and rated rows | |
| `pool_counts(store, from, to)` | Blocks, empty blocks and reward per pool over a range, most blocks first | 0.2 ms for a week; 0.2 s for all of history |
| `pool_weeks(store, from, to)` | Blocks per pool per week from Monday 00:00 UTC | 0.24 s for all of history |
| `pool_page(store, pool, from, below, limit)` | One pool's rows below a height, newest first | 0.2 ms for 100 |
| `retargets(store, floor, from, to)` | Each retarget's time and difficulty, with the period before's when that is stored at the floor or above | 0.9 ms for all of history |
| `work(store, from, to)` | Difficulty summed over a range and its first and last time: a hashrate estimate's inputs | |
| `at_or_before(store, floor, time)`, `first_since(store, floor, time)` | The newest block stamped at or before a time, and the lowest height stamped at or after one, at the floor height or above, through the `time` index | 0.4 ms; 1 ms for a week back, 27 ms for a year |
| `top(store)` | The highest row's height and hash | |
| `reattribute(store, pools)` | Redoes the `pool` column if the list changed (above) | 1.8 s for every row |

Measured with `MINING_BENCH_DIR=<dir> cargo test --release -p engine -- --ignored --nocapture mining_benchmark`, which writes 975,000 rows attributed as they go in 9 s (attribution adds about 2 µs a row).

Measured on the machine and load described under [Measurements](#measurements) with `cargo test --release -p engine -- --ignored --nocapture read_benchmark`: 975,000 realistic rows written in 7 s, 295 MB.

### audits

One row per block audited when it arrived ([`MEMPOOL.md`](MEMPOOL.md#block-audits) has the method and the categories). The code is `crates/engine/src/audit/`.

| Column | Type | Why |
|---|---|---|
| `hash` | 32-byte blob, key | Every per-block route asks by hash. Keyed by hash, not height, so a block that leaves the best chain keeps its audit |
| `height` | integer, indexed | Pages of scores by height, joined to `blocks` on height and hash so only the best chain's are listed |
| `time` | integer | The header's timestamp: the match rate over time (`predictions`) |
| `seen_at` | integer | When we learnt of the block, Unix seconds: what `fresh` was measured from |
| `match_rate` | real | The score, percent, two decimals |
| `expected_*`, `actual_*` | integer × 8 | Transactions, vsize, weight and fees of the expected block and of the mined one (coinbase left out) |
| `matched_*` … `unseen_*` | integer × 14 | Transactions and vsize of each of the seven categories |
| `txs` | blob | The listed transactions, every category but `matched`: at most 1,000 records of 45 bytes (txid, kind, vsize, rate in sat per 1,000 vB, accept time or 0), the most telling kinds first, then highest rate. The totals count every one; a reader compares them to tell a capped list |

**No audit is a missing row,** and that is a real answer: the server was not running, had no projection on the parent, or the projection was stale or for another block. Rows are written once, when the block arrives (`INSERT … ON CONFLICT DO NOTHING`): the first audit of a block stands.

**Size.** About 300 bytes of totals and index, plus 45 bytes per listed transaction: 2.3 and 3.5 KB for the two mainnet blocks measured ([`MEMPOOL.md`](MEMPOOL.md#on-mainnet)), so about 150 MB a year and at most about 45 KB, for an empty block against a full expectation.

**Not rebuildable.** The expectation existed only in memory at the moment the block came. Deleting the file loses every audit.

#### Reading

`crates/engine/src/audit/db.rs`:

| Call | Returns |
|---|---|
| `by_hash(store, hash)` | One block's `Audit` with its listed transactions |
| `by_hashes(store, hashes)` | Each block's totals, in order, without the lists: the block facts |
| `scored_between(store, from, to)` | Score, expected fees and weight of the best chain's audited blocks in a height range, lowest first |
| `scored_since(store, time)` | The same for the blocks stamped since a time |
| `write(store, audit)` | Stores an audit unless the block has one |

### mempool_history

What the mempool looked like over time: its size, fees and fee bands through the day, the week and the year, for the mempool chart and `/api/v1/statistics` ([`API.md`](API.md#mempool-statistics)). The code is `crates/engine/src/history/`; the server runs it from `crates/server/src/statistics.rs`.

**Sampling.** One task per network, started with the mempool engine when the network has a store. At every whole minute of the wall clock it reads the projection's totals (the whole mempool, exactly what the `mempool` topic sends), the node's `mempoolminfee` from the tracker's own probe, and the flow it counted on the tracker's change stream since the minute before: what entered, what left, and of that what was confirmed. It keeps the virtual size of every mempool transaction to count removals (a removal carries only the txid): about 37 bytes each in its hash table, 4.8 MB at 75,000 transactions. It asks the node nothing. A minute is left out, never stored as a guess, when:

| Reason | When |
|---|---|
| `syncing` | The engine has not finished its first cycle, or this is the first minute after start, after a minute the engine was not live, or after a late one: the flow over it was not watched whole |
| `stale` | The projection is stale: the node is failing and the numbers are the last good ones |
| `lagged` | The sampler fell more than the change stream's 64 deltas behind, so the minute's flow is incomplete |
| `late` | It read the state more than 5 s from the minute (a stalled process, a loaded machine, a clock that jumped ahead), or after the next minute had begun: the state is not that minute's and the flow would not span one minute. The minute after a late one is skipped too (as `syncing`), since its flow began off the minute |
| `store_busy`, `store_failed` | The write queue was full (`try_write`), or the write failed |

So a gap in the table is the truth: the engine was off, the node was failing, or the server was down. History starts when the server first ran with the engine on and a `DATA_DIR`.

| Column | Type | Why |
|---|---|---|
| `level`, `time` | integer key | 0 a minute, 1 an hour, 2 a UTC day; the minute sampled (Unix seconds), or the period's start |
| `samples` | integer | Minutes sampled in the row: 1 for a minute, at most 60 for an hour and 1,440 for a day. The coverage the routes report |
| `count`, `vsize`, `fees` | integer | Transactions, sigop-adjusted virtual size and base fees (sats) in the mempool |
| `min_fee` | integer | `mempoolminfee` in sat per 1,000 vB |
| `band0` … `band9` | integer | Virtual size in each of the design's ten fee bands, by effective rate as the projected blocks band it; they sum to `vsize` |
| `in_count`, `in_vsize`, `out_count`, `out_vsize`, `mined_count`, `mined_vsize` | integer | The flow over the minute before the sample: entered, left for any reason, and confirmed |

**Every value is a sum over the row's `samples`.** A minute row holds its values as they are; an hour row the sum of its minutes. So rolling up adds rows together with nothing lost (a mean is the sum over `samples`, and flows are totals), and two roll-ups of the same period merge.

**Retention.** Minutes for 8 days, hours to 400 days, days after that. The 8 days cover a week of ten-minute points, the 400 a year of eight-hour points: no interval mixes rows coarser than its points ([`API.md`](API.md#mempool-statistics)). Neither is a setting; nothing needs it.

**The roll-up** runs in the writer, in the same write as every sample: the minute rows of every whole hour older than 8 days are summed into that hour's row (merged with one already there) and deleted, then the same for hour rows of whole UTC days older than 400 days. One write, so it is all or nothing: a crash or an error leaves the rows as they were, and the next minute does it again. Run twice it changes nothing. A sample for a minute whose hour or day is already rolled up (a clock that ran days ahead and was set back) is not stored, so no minute is counted twice; and when the clock goes back, the sampler drops the flow it was counting and samples again only after a whole minute past the last one it stored. With nothing to do it costs two index probes; once eight days are stored, it moves one hour of minutes an hour. After a long stop it catches up in the first write: a year of minutes took 0.56 s.

**Size.** A minute row is about 93 bytes on disk (a year of minutes alone, unrolled, was 49 MB for 529,000 rows). The bound is 11,520 minute rows and 9,408 hour rows, plus one row a day beyond 400 days: ten years held 24,180 rows in 2.5 MB, and each further year adds 365 rows, about 34 KB.

**Reading.** `history::series(store, interval, now)`, one read: a point per step with at least one sample, from the period's start to `now` (rows stamped later, by a clock that ran ahead, are left out) (a step with none has no point, so a chart shows a gap, not a zero), each point the sums and `samples` of its rows; the oldest stored time; and the minutes the period owes against the minutes sampled. Measured with `HISTORY_BENCH_DIR=<dir> cargo test --release -p engine -- --ignored --nocapture history_benchmark` on a table at the ten-year bound (on the machine under [Measurements](#measurements), load average 10 to 20):

| Interval | Points | Median of 5 |
|---|---|---|
| `2h` | 121 | 0.15 ms |
| `24h` | 1,441 | 1.3 ms |
| `1w` | 1,009 | 5.0 ms |
| `1m`, `3m`, `6m` | 721, 1,081, 1,081 | 5.8 to 7.6 ms |
| `1y` | 1,096 | 12 ms |
| `2y`, `3y`, `4y` | 730, 1,095, 730 | 13 ms |
| `all` (ten years) | 914 | 15 ms |

**Rebuildable:** no. It was observed live; deleting the file loses it.

### replacements

The replacement record ([`MEMPOOL.md`](MEMPOOL.md#following-a-transaction)), so that what replaced what survives a restart: the transaction page's "replaced by", the `tx:` topic and the `/api/v1` replacement and first-seen routes ([`API.md`](API.md#transactions)). The code is `crates/engine/src/replacements/db.rs`; the server writes it from `crates/server/src/replacements.rs`.

| Column | Type | Why |
|---|---|---|
| `n` | integer key | The order recorded, newest highest: what the bound keeps and what the trees list newest first |
| `txid` | 32-byte blob, unique | The replaced transaction. A transaction replaced again (it came back) replaces its row |
| `by` | 32-byte blob, indexed | Its replacement. Indexed to mark a replacement mined, and to walk a history backwards |
| `seen` | integer | Unix seconds we saw it replaced |
| `fee`, `vsize`, `weight`, `value`, `time`, `rbf` | integer | The replaced transaction's base fee and total output value (sats), sigop-adjusted vsize, weight, the node's accept time and whether it signalled BIP 125 |
| `by_fee` … `by_rbf` | integer or `NULL` | The same for the replacement; all `NULL` when the tracker never held it (it was confirmed in the block that removed the original) |
| `mined` | integer | 1 once the replacement was seen confirmed |

**Writes.** One writer task per network takes each tracker delta's changes from a queue of 64, with whatever else is queued, and writes them in one transaction, delta by delta in order: `INSERT OR REPLACE` per new link (after deleting the replacement's own row, which no longer stands now that it is back), an indexed `UPDATE` per replacement seen confirmed, and for a transaction back in the mempool a `DELETE` of its own row and an `UPDATE` clearing `mined` on what it replaced; then `DELETE` of every row more than `REPLACEMENTS_KEPT` places behind the newest (a deleted row's place still counts, so the table may hold fewer, never more). All or nothing: a crash or an error leaves the rows as they were, and memory still has the links. The tracker and the task that follows it never wait for the store: a full queue drops the write and logs `replacements_store_failed`.

**Reading.** `load(store, keep)` once, when the mempool engine starts: the newest `keep` rows, put back into the record oldest first. 10,000 rows took 3.6 ms (below). Requests never read the table; they read the record in memory.

**Size.** At most `REPLACEMENTS_KEPT` rows (10,000 by default). Measured with `cargo test --release -p engine -- --ignored --nocapture replacements_benchmark` (20,000 links written 20 at a time, so the bound had pruned half): 10,000 rows, 2.06 MB for the table and its two indexes, 205 bytes a row; reading them back took 3.6 ms.

**Rebuildable:** no. The node does not keep replaced transactions; deleting the file forgets every replacement seen before.

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
