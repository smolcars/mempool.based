# Mempool tracking

rbitcoin v0.9.1 has no mempool push, so `engine::mempool` polls the node, keeps its own copy of the mempool in memory and publishes the changes. Projected blocks, the live feed, fee recommendations, transaction tracking and the mempool charts are built on what it emits.

Measurements below are from a synced mainnet node on a private network, 2026-11-12, with about 55,000 transactions in its mempool. RPC and Esplora costs that other parts of the project rely on are in [`RBITCOIN.md`](RBITCOIN.md).

## Shape

| Part | File | Does |
|---|---|---|
| Model | `crates/engine/src/mempool/model.rs` | The state, the diff, removal classification and delta application. Pure: no I/O, no clock |
| Shell | `crates/engine/src/mempool/tracker.rs` | One loop that probes, lists, fetches and publishes; the subscriber API; the events |
| Proof | `crates/engine/examples/mempool_watch.rs` | Runs the tracker against a live node and prints its events |
| Projected blocks | `crates/engine/src/projected/` | The node's chunks packed into the next blocks; see [Projected blocks](#projected-blocks) |
| In the server | `crates/server/src/ws/mempool.rs` | Runs both per network and feeds the WebSocket; see [In the server](#in-the-server) |

```rust
let (tracker, task) = Tracker::start(NodeSource { rpc, esplora }, Config::default());
let (snapshot, mut sub) = tracker.subscribe(|m: &Mempool| /* copy what you need */);
while let Ok(delta) = sub.recv().await { /* delta.seq == previous + 1 */ }
tracker.block_found(tip);       // a block the node announced: what it confirms leaves at once
tracker.tip_changed();          // poll now (after a reorg or missed announcements)
tracker.read(|m| m.get(&txid)); // a point lookup
tracker.read(|m| m.links(&txids)); // which of these spends which, from the inputs held
tracker.events();               // statistics and abnormal conditions, for logging
task.abort();                   // stop polling
```

## The record

One per mempool transaction, keyed by txid:

| Field | Bytes | From | For |
|---|---|---|---|
| txid (the key) | 32 | listing | everything |
| fee | 8 | `getmempoolentry` `fees.base` | fee rates, charts, feed |
| total output value | 8 | the raw transaction | the live feed |
| vsize, sigop-adjusted | 4 | `getmempoolentry` | fee rates, projected blocks |
| weight | 4 | `getmempoolentry` | block fill |
| size | 4 | the raw transaction | the projected blocks' byte size (`/api/v1/fees/mempool-blocks`) |
| accept time | 4 | `getmempoolentry` `time` | "first seen"; it is the node's, so it survives our restarts |
| signals replaceability | 1, in padding | the raw transaction: an input's sequence below `0xfffffffe` (BIP 125) | the replacement trees' `rbf` ([`API.md`](API.md#transactions)). rbitcoin replaces regardless |
| output count | 4 | the raw transaction | finding children of a removed transaction |
| input fingerprints | 16 + 8 per input | the raw transaction | finding what replaced it |

The value is kept for every transaction rather than only the newest: it costs 8 bytes and arrives free with the inputs, which are needed anyway.

Input fingerprints are 64-bit SipHash values of each spent outpoint under a random key chosen at startup, not the 36-byte outpoints: an attacker cannot grind a collision without the key, and a chance collision across a whole mempool is about one in 10⁸.

Everything else is fetched on demand from the node: ancestors, descendants and cluster chunks (`getmempoolcluster`), scripts and addresses (Esplora).

**Cost.** The hash-map slot is 88 bytes. At 300,000 transactions the table has 524,288 slots (one control byte each), about 47 MB, plus the inputs on the heap: about 55 MB in all, or roughly 180 bytes a transaction. On mainnet at 55,000 transactions the whole process, runtime and HTTP clients included, measured 32–48 MB RSS over a 15-minute run.

## Initial sync

The record needs the inputs, and only a transaction's bytes have them: no listing or entry call returns them. That rules out the verbose `getrawmempool` (5.7 s and 39.5 MB on mainnet) as a shortcut, since it would still leave every transaction's bytes to fetch.

rbitcoin takes a JSON-RPC array as one request and one work-queue slot, so details are fetched in batches. Measured on mainnet:

| Way to get details | Time | Bytes |
|---|---|---|
| One `getmempoolentry` | 10 ms | 0.7 KB |
| One Esplora `/tx/:txid` | 11 ms | 1.3 KB |
| Batch of 1,000 `getmempoolentry` | 214 ms | 0.7 MB |
| Batch of 1,000 `getrawtransaction` (hex) | 305 ms | 5.0 MB |
| Both for 1,000 txids in one array | 425 ms | 5.3 MB |

One transaction at a time would take 55,000 × 10 ms: nine minutes on one connection. Batches take seconds.

The tracker sends, per batch of 500 txids, a `getmempoolentry` array and a `getrawtransaction` array in parallel, with two batches in flight: four requests, a quarter of the node's 16 work-queue slots, and the RPC client's own default limit. A batch over the client's 64 MB response cap is halved until it fits; a single transaction that still does not fit (none can: the largest is 8 MB of hex) would be skipped like a vanished one.

**Measured:** 54,509 transactions in 10.4 s: 229 calls (220 of them detail arrays, plus the probe, the listing, the tip and six blocks) and 252 MB read, about 4.6 KB a transaction, almost all of it raw transaction hex. No call failed and none answered `Busy`.

### Over a unix socket

The bulk `/internal/mempool/txs` routes exist only on rbitcoin's unix-socket Esplora listener. They are not used, because they would not save anything:

- They return Esplora's transaction JSON, which measured 6.2 times the size of the raw bytes over 30 sampled mainnet transactions.
- They carry neither the node's accept time nor its sigop-adjusted vsize, so the `getmempoolentry` batch would still be needed.
- The RPC batch already moves everything in one request per 500 transactions.

So both transports use the same RPC path. Not measured against a real unix socket: no shared node exposes one to this machine.

## Steady state

Each cycle:

1. **Probe:** `getmempoolinfo` (10 ms, under 1 KB). If its size, vsize total and fee total equal the ones taken before the last full cycle, and no tip hint arrived, and the last full cycle is under 10 s old, stop here.
2. **List:** `getrawmempool` with `mempool_sequence` (110–250 ms, 3.6 MB at 55,000 transactions).
3. **Tip:** Esplora's tip hash, stamped with its height (18 ms).
4. **Blocks:** the six most recent blocks on the active chain ending at that tip, fetched with Esplora's raw-block route (98 ms, 1.6 MB a block) only when not already held. Usually zero; one per new block.
5. **Diff** the listing against the model: added and removed txids.
6. **Fetch** details for the added ones, as in the initial sync.
7. **Classify** the removals, build one delta, apply it and publish it.

**Cadence.** A cycle starts at least 1 s after the previous one started, and never sooner after the previous one ended than that one took, so the tracker spends at most half its time talking to a slow node. After a cycle in which a listed transaction's details could not be read, the next cycle lists again whatever the probe says. One loop does everything in order: a cycle can never overlap another, and nothing queues behind a slow node. On failure the wait doubles from 2 s to a cap of 30 s, and comes back to 1 s on the next success. A hint (`block_found` or `tip_changed`) skips the wait but never interrupts a cycle in progress; what `block_found` does is in [When a block is found](#when-a-block-is-found).

The probe saves the 3.6 MB listing whenever the mempool did not move. On mainnet it almost always has, so the listing runs about once a second. A change the probe cannot see (an eviction and an arrival with identical size and fee) is caught by the 10 s heartbeat.

**Measured** over 15 minutes on mainnet, 54,000–55,000 transactions, one block, and a node restart that nobody planned:

| | |
|---|---|
| Cycles | 610 in 900 s, one every 1.5 s on average |
| A typical cycle | 5 calls (probe, listing, tip, entry array, raw array), 3.6–3.7 MB, 170–440 ms |
| The block cycle | 4 calls plus one block, 5.0 MB, 284 ms: 3,502 confirmed, 3 replaced, 0 evicted |
| Total read | 2.48 GB, the initial sync included: about 2.5 MB/s, nearly all of it the listing |
| Our count against `getmempoolinfo` | equal at 23 of 28 checks, 1–4 short at the others: transactions that arrived between our cycle and the check. Summed vsize and fees matched to the satoshi whenever the counts did |
| Removals | 3,502 confirmed, 45 replaced, 163 evicted |
| Node restart | 3 failed cycles (connection refused, retried after 2, 4 and 8 s), then `SequenceRegressed` from 110,704 to 54,650 and exact agreement on the next check |

The listing is the price of having no event stream: at one a second it is the whole steady-state cost.

## Why a transaction left

Each removal gets one reason, decided in this order:

1. **Confirmed** if its txid is in one of the six recent blocks. Exact.
2. **Replaced by X** if X spends one of its inputs, where X is a transaction in one of those blocks or one that entered the mempool in the same cycle. Exact: two transactions spending the same outpoint conflict, and the node cannot hold both, so the replacement enters in the cycle where the original leaves.
3. **Replaced by X** if it spends an output of a transaction replaced by X in the same cycle: a descendant leaves with its replaced ancestor. Repeated until no more are found.
4. **Evicted** otherwise.

The window of six blocks is there because a listing and the tip read after it are not atomic: a transaction confirmed just after a listing is removed one cycle later, and its block must still be at hand. A reorg needs nothing special: the window is walked back from the new tip, so it only ever holds the active chain, and transactions from disconnected blocks come back as additions.

### Where it can only infer

- **Evicted, expired and dropped are one reason.** The node does not say why it dropped a transaction. Size eviction, the two-week expiry, and dropping after a reorg made a transaction invalid all read as `Evicted`.
- **A replacement that came and went inside one cycle.** If A is replaced by B and B by C within one poll interval, A is shown replaced by C when C still spends one of A's inputs (the usual fee bump keeps them). If C spends none of A's inputs, the link is lost and A reads as `Evicted`.
- **A replacement whose details vanished.** If B replaced A and then left before its details were read, A reads as `Evicted`.
- **Blocks older than the window.** A transaction confirmed more than six blocks before the cycle that removes it reads as `Evicted`. This needs the tracker to have stalled for six blocks.
- **A descendant of a confirmed or evicted transaction** that leaves gets its own classification, not its parent's.

## The change stream

Every delta has a `seq` that counts up by one from 1. `Tracker::subscribe` runs a closure on the current state and registers the subscriber while holding the state's read lock, and the tracker applies and sends each delta while holding the write lock. So the first delta a subscriber receives is exactly the one after its snapshot: no gap and no duplicate.

Subscribers share one bounded ring of 64 deltas. One that falls further behind receives `Lagged`, never a growing queue; it takes a new snapshot. A large delta, such as the initial sync's, is held only until every subscriber has read it or it is pushed out of the ring.

A delta carries the added transactions with their records, the removed txids with their reasons (both sorted by txid), the records the replaced ones had (so a reader can say what a replacement replaced after it has left), the node's `mempool_sequence` for the listing, and the chain tip read after it. A cycle that changes nothing publishes nothing.

## Failures

| Condition | What happens |
|---|---|
| A call fails, times out or is `Busy` | Nothing from that cycle is published. Details already fetched are kept for the retry, so a long initial sync never starts over. The wait backs off |
| A slow node | The next cycle waits as long as the slow one took. Calls are bounded by the RPC client's timeout and concurrency limit |
| A transaction leaves between the listing and its details | It is skipped and counted as `vanished`. If it is somehow still listed, the next cycle fetches it again |
| Any single transaction answers an error | Treated like a vanished one: one odd transaction can never stall the loop |
| The node restarts, its sequence goes backwards | Reported as an event. The diff does not rely on the sequence, so nothing else changes |
| The mempool empties | Every transaction is removed, classified as above |
| A reorg | Handled by walking the block window from the new tip |

None of these panics, and each leaves the model correct after the next successful cycle.

## What is exposed for logging

The tracker logs nothing itself. `Tracker::events()` yields:

| Event | Fields |
|---|---|
| `Cycle` | per cycle: duration, response bytes, node calls, probes skipped since the last cycle, txids listed, added, vanished, removed by reason, blocks fetched, model size, `mempool_sequence`, tip, published `seq` |
| `Block` | per block applied from its own transactions: the same fields, nothing listed or added, and the time from `block_found` to the delta |
| `Synced` | once, after the first complete cycle: duration from start, transactions, bytes, calls |
| `PollFailed` | the error, consecutive failures, the wait before the retry |
| `SequenceRegressed` | old and new `mempool_sequence` |
| `SubscriberLagged` | how many deltas the subscriber missed |

Byte counts come from response-body counters in the node clients, so give the tracker its own clients if they should count only its traffic.

## With a mempool event stream

An event stream from rbitcoin (added, removed with a reason, a sequence number) is on the upstream wishlist in [`RBITCOIN.md`](RBITCOIN.md). With it:

- The 3.6 MB listing every second goes away, and with it the up-to-one-interval delay before a new transaction appears.
- Removal reasons become the node's own. `Evicted` splits into eviction, expiry and the rest; the cases above where a replacement is lost stop existing.
- The input fingerprints, output counts and block window exist only to classify removals, and could be dropped: 20 bytes plus 8 per input for each transaction, and six held blocks.
- The initial sync stays: an event stream still needs a starting snapshot, and the sequence number would let us join the two without a gap, the same way subscribers join this tracker.

## Projected blocks

`engine::projected` turns the tracked mempool into the blocks that would be mined next: the left half of the block strip, the answer to "which block will my transaction be in", and the basis of fee recommendations and block audits.

```rust
let (projector, task) = Projector::start(tracker.clone(), rpc, Config::default());
let (projection, mut sub) = projector.subscribe(); // Arc<Projection>: blocks, records, totals
while let Ok(delta) = sub.recv().await { /* delta.seq == previous + 1 */ }
projector.position(&txid);   // its block, and the vsize and transactions ahead of it
projector.group(&txid);      // its cluster as the node last told it: chunks, fees, members
projector.expected(&parent); // our first block as it stood on `parent`, for auditing the block mined on it
projector.events();          // statistics and abnormal conditions, for logging
task.abort();
```

| Part | File | Does |
|---|---|---|
| Model | `crates/engine/src/projected/model.rs` | Chunks, their order, packing into blocks, summaries. Pure: no I/O, no clock |
| Shell | `crates/engine/src/projected/projector.rs` | Follows the tracker, asks the node for changed clusters, packs off the async runtime, publishes |
| Proof | `crates/engine/examples/projected_watch.rs` | Runs both against a live node and scores the first block against the node's template and against real blocks |

### What comes from the node and what is computed

rbitcoin mines by cluster-mempool chunks: each cluster is linearized and cut into chunks, and a template takes chunks best first. Its `COMPAT.md` says how its linearization differs from Core's, which is why we ask instead of linearizing ourselves.

**From the node: every chunk.** `getmempoolcluster` answers a transaction's whole cluster as chunks, each with its fee, sigop-adjusted weight and members in mining order. The projector asks it, in batches, for:

- every transaction the tracker adds (its cluster may now include older transactions it lifts);
- the remaining members of the cluster of every transaction the tracker removes (the cluster may have split, or lost the child that lifted a parent);
- a cluster whose answer named members the tracker has not shown us yet, again at the next update;
- every cluster, once per block, right after the block's update is published and once the tracker has nothing more to fold in, so in a burst each block's own update goes out first. Anything we cannot see change, such as a `prioritisetransaction` on the node or a lost recheck, is corrected within a block.

It asks once per cluster as far as it knows them: each answer covers its members, and later rounds ask only for what is still uncovered.

**Computed: packing the chunks into blocks, by the node's own rule.** A template sorts chunks by their rate (in whole sat per 1,000 vB, rounded down, on sigop-adjusted size), then by fee, both descending; takes each chunk that still fits 3,992,000 raw weight units (4,000,000 less 8,000 for the coinbase); skips one that does not and keeps trying the rest, so small cheap transactions fill the gap at the end; leaves out chunks under `-blockmintxfee` (1 sat/kvB); and pulls in a chunk's unselected ancestors. We do the same per block, for `Config::blocks` blocks (8 by default), the last taking everything left: the "+N blocks" tile.

A transaction's **effective fee rate** is the rate of its chunk. A low-fee parent lifted by a child carries the child's chunk rate and lands where the node would put it.

Why not the other calls:

| Call | Why not, on every change |
|---|---|
| `getblocktemplate` | Only the first block, and 450 ms and 4.2 MB a call (measured). At one call per update that is about 175 MB a minute, more than the tracker's own listing, for an answer we already match (below) |
| `getmempoolfeeratediagram` | Every chunk in order, but no txids: it cannot say which transaction is where. Our chunks match it: same count at every check, and the fixture test replays its top 118 points exactly |
| `getmempoolentry` | Has the chunk's fee and weight but not its members or order; it would have to be fetched again for every transaction whose chunk changes |

### Agreement with the node

Measured on mainnet, 2026-11-12, 53,000–56,500 transactions, read-only, with `projected_watch` scoring our first block against `getblocktemplate` every 30 s (only when the node's template sits on our tip) and against every block that arrived.

**Against the node's template, 41 checks over 21 minutes:**

| | By transaction | By weight |
|---|---|---|
| Raw: share of the template in our first block | mean 90.3%, lowest 80.6% | mean 93.9%, lowest 83.2% |
| Tie-aware | mean 99.99%, lowest 99.74% | mean 99.98%, lowest 99.72% |

Everything the two disagree on sits in one fee-rate class at the block boundary: thousands of chunks with the same rate and fee (0.124 sat/vB at the time), among which the node picks in hash-map order. "Tie-aware" counts a template transaction we placed one block later as agreeing when one of ours the template left out has the same rate and fee. Above the boundary our first block is the template, transaction for transaction.

**Against real blocks** (share of the block that was in our first projected block just before it):

| Block | By transaction | By weight | Tie-aware, by weight | Never reached our mempool | We had placed later |
|---|---|---|---|---|---|
| 975,521 | 65.8% | 71.7% | not measured | 0.22% of weight | not measured |
| 975,522 | 94.9% | 95.5% | 97.6% | 0.23% | 2.1% |
| 975,523 | 84.4% | 94.8% | 98.3% | 0% | 1.7% |

A miner builds from its own mempool and its own policy, breaks the same ties its own way, and some take transactions out of band, so a real block is a looser target than the node's template.

### Cost

Same runs, the server and node on one private network.

| | |
|---|---|
| First projection | 55,400 transactions: 12 cluster requests (5,571 clusters asked), 6.0 MB, 0.88 s, on top of the tracker's initial sync |
| A typical update | one request asking about 15 clusters, a few KB, 4–6 ms; then packing, mean 19 ms, worst 172 ms, on a blocking thread |
| Each block | one update for the block (about 17 clusters), then every cluster again: about 12 requests, 6 MB, 0.5 s |
| Calls and bytes added to the node | 43 requests a minute and 340 KB a minute over 21 minutes with one block: about 40 KB a minute between blocks plus 6 MB a block. The tracker reads about 150 MB a minute |
| Deltas | mean 199 records; the block's delta set 13,926 records and removed 2,664 |
| Memory | the tracker alone 39 MB RSS; tracker and projector 80 MB at 56,469 transactions, so about 40 MB, or 700 bytes a transaction, the published projection included |

Packing holds the projector's write lock, so a `position` lookup can wait for one pack. Nothing runs on the async runtime's threads but the network calls.

### The records and the stream

One record per transaction, 49 bytes on the wire:

| Field | Bytes | Is |
|---|---|---|
| txid | 32 | the key |
| block | 1 | projected block, 0 for the next; the last block holds the rest |
| rate | 4 | effective fee rate in sat per 1,000 vB: the rate of its chunk, the node's own number |
| vsize | 4 | sigop-adjusted virtual size |
| fee | 8 | its own base fee in satoshis |

Each block also has totals: transaction count, raw weight, vsize, fees, the lowest, median (at the middle vbyte) and highest effective rate, and the vsize in each of the ten fee bands of `docs/DESIGN.md`. A rate is banded as it prints, rounded to two decimals, so 0.1996 sat/vB takes band 2.

The stream works like the tracker's: `subscribe` returns the current projection and a subscription whose first delta is the next one, deltas are numbered without gaps, and a subscriber more than 64 deltas behind gets `Lagged` and subscribes again. A delta carries the records that entered, moved to another block or changed rate (`set`), the txids that left the mempool (`removed`), both sorted by txid, and every block's new totals. Applying `removed`, then `set`, to the previous projection's records gives the next one's; a test checks this against a running tracker. Order inside a block is not in the delta: it is by rate, and a client sorts by it. Ties keep no meaningful order (see below).

A new block moves nearly every record in the projected blocks one block forward: 14,000 to 25,000 records (0.7 to 1.2 MB at 49 bytes), once per block. A WebSocket hub may prefer to send a fresh snapshot then.

When the tip moves, the first block of the last projection on the old tip is kept as `Expected` (six of them), so the block mined on that tip can be audited against what we expected ([Block audits](#block-audits)). With it are whether that projection was `stale`, the tip it gave way to (`next`) and when (`at`), and `left`: every transaction it held, in any block, that the first projection on `next` no longer holds, with the rate it was placed at. That list is about one block's transactions (some 4,000, 36 bytes each on mainnet), so the six cost under a megabyte.

### Failures and odd cases

| Condition | What happens |
|---|---|
| Empty mempool | Every block is empty, with zero totals |
| Smaller than a block (signet, testnet4, regtest) | Everything in the first block; the rest empty |
| A cluster request fails or answers `Busy` | The affected transactions keep their last chunks, `stale` is set on the projection and in its deltas, and nothing is asked until the backoff ends (2 s, doubling to 30 s), however often the tracker publishes. New transactions wait with the rest, placed by their own fee rate |
| The tracker's own cycles fail (the node is down) | The last good projection stays, marked `stale` until the tracker's next good cycle |
| A transaction left between the tracker's listing and our request | Its slot answers "not in mempool": it keeps its record until the tracker removes it |
| Large clusters | Up to 64 transactions (rbitcoin's limit): one answer, a few KB |
| A reorg | Handled by the tracker; transactions from disconnected blocks come back as additions and are asked about like any other |
| The projector falls behind the tracker by more than 64 deltas | It takes a fresh snapshot, diffs it against what it has, and asks the node for every cluster again |

### Limits

- **Ties at the boundary.** The node breaks ties between chunks of equal rate and fee in hash-map order, which nobody outside the node can know. Mainnet mempools now hold thousands of identical chunks at one rate, and when the block boundary falls inside such a class, which of them make the first block is arbitrary. The miner's own node picks among them by its own order anyway, so no explorer can predict that part. We break ties by age.
- **Sigops are not counted.** The template also stops at 80,000 sigop cost. We do not have each transaction's sigops; a mainnet template uses about 8,000, so the limit does not bind today.
- **Pulling ancestors.** We pull in a chunk's earlier, unplaced chunks of the same cluster, not only its ancestors, since the answer does not say which chunks depend on which. They differ only when an earlier chunk of a cluster did not fit and a later, independent one would.
- **`prioritisetransaction`** deltas are in the node's chunk fees and rates but not in the record's own `fee`, which is the base fee. A delta applied to a transaction whose cluster does not otherwise change shows at the next block's refresh.
- **One update behind.** The projection follows the tracker, so it trails the node by up to one tracker cycle (about 1.5 s) plus one cluster request. A new block is the exception: see [When a block is found](#when-a-block-is-found).

### What is exposed for logging

The projector logs nothing itself. `Projector::events()` yields:

| Event | Fields |
|---|---|
| `Update` | per update: tracker deltas folded in, clusters asked, node requests, response bytes, time waiting for the node, time packing, transactions, chunks, records set and removed, `stale`, the published `seq`, and `refresh` for the once-a-block refresh, whose records set and removed are what it corrected |
| `FetchFailed` | the error, consecutive failures, transactions pending, the wait before the retry |
| `Resynced` | the projector fell behind the tracker and rebuilt from a snapshot: transactions |
| `SubscriberLagged` | how many deltas the subscriber missed |

## In the server

`crates/server/src/ws/mempool.rs` runs one tracker and one projector per enabled network, from startup, unless `{NET}_MEMPOOL_ENABLED=false` ([`CONFIG.md`](CONFIG.md#the-live-mempool)).

- **Their own node clients.** The tracker gets its own RPC and Esplora clients and the projector its own RPC client, so the byte counts in their events are theirs alone. Each RPC client allows four calls in flight: with the shared client the API uses, at most twelve of the node's sixteen work-queue slots.
- **The tip hint.** A task follows the network's Electrum events: it hands every new block to `block_found`, and calls `tip_changed` after a reorg or when it missed events.
- **A node down at boot.** Nothing special: the tracker retries with its backoff (2 s doubling to 30 s), the projector waits for it, and the topics publish nothing until the first complete cycle. When the node comes back the initial sync runs and the topics start. A node lost later leaves the last projection in place, marked `stale`.
- **Shutdown.** When shutdown starts, the engines and their tasks are aborted, so no polling continues while sockets drain.
- **The topics** (`mempool`, `projected`, `projected-txs:N`, `transactions`, in [`WS.md`](WS.md#mempool-topics), and `tx:<txid>`, below) are fed by one task per network each from the latest projection and the tracker's change stream. No subscription asks the node for anything.
- **History.** With a `DATA_DIR`, a sampler per network (`engine::history`) stores the mempool's totals, its fee bands and the minute's flow (entered, left, confirmed, from the change stream) once a minute while the projection is current, and skips minutes when it is not. It asks the node nothing. The table, its retention and why a minute is skipped are in [`STORE.md`](STORE.md#mempool_history); the routes in [`API.md`](API.md#mempool-statistics).
- **The log.** The engines' events become typed lines ([`LOGGING.md`](LOGGING.md#the-live-mempool)): `mempool_synced` once, one `mempool_summary` a minute with the minute's totals, one `mempool_block` per block with its timings, and a line for each abnormal condition, repeats summarised at powers of two.

### Following a transaction

`crates/server/src/ws/track.rs` serves the `tx:<txid>` topic and `crates/server/src/relatives.rs` the relatives route ([`WS.md`](WS.md#following-a-transaction-or-an-address), [`FRONTEND.md`](FRONTEND.md#a-transactions-relatives-and-replacements)). Neither asks the node anything:

- **Where it is** is the tracker's record (accept time, fee, size) and the projected blocks' `position`.
- **Why it left** is the tracker's removal reason, taken from the change stream by one task per network while anyone follows the transaction, with the honesty of [Why a transaction left](#why-a-transaction-left): `evicted` also covers the cases where a replacement could not be seen. A transaction that left before anyone followed it, or while the task had fallen more than 64 deltas behind, reads `unknown` unless the replacement record knows it. A confirmation in a block that Electrum reports gone in a reorg is forgotten, and so is every remembered confirmation when Electrum reports it lost the chain's continuity (a reconnect, a reorg deeper than it follows). The task reads the engines on a blocking thread (a position lookup can wait for a pack) and publishes only from a reading at the tracker delta whose reasons it has folded in.
- **What it will be mined with** is the projected blocks' copy of its cluster (`Projector::group`), the node's own chunks, and which member spends which from the inputs the tracker holds (`Mempool::links`): one keyed hash per output of the cluster's members, at most 64 transactions.

**The replacement record** (`crates/engine/src/replacements/`, run from `crates/server/src/replacements.rs`) keeps every replacement the tracker reports, whether anyone follows it or not: the replaced txid, its replacement, when it was seen, both sides' own figures as the tracker held them (fee, size, weight, total output value, accept time, whether it signalled; the replacement's are missing when it was confirmed in the block that removed the original), and whether the replacement is confirmed, with its block's height and hash (seen leaving the mempool in a block, or found in one of the tracker's recent blocks: a replacement the tracker never held was in one, so its block is always known). A transaction that replaced its own replacement (a miner confirmed the original) ends the history: its old link is dropped, so every history has one newest end, and the figures that link held for it become the new link's replacement figures, so the tree's root keeps them. A transaction back in the mempool (rebroadcast, or returned by a reorg) no longer stands replaced, and what it replaced is no longer mined. Reorgs reach it as below. It is read as chains by the `tx:` topic and `/app`, and as trees (a replacement and everything it removed, recursively) by `/api/v1` ([`API.md`](API.md#transactions)). A child that left with its replaced parent is listed as replaced by the same replacement, so a tree shows everything one replacement removed, not only the transactions it conflicts with. It is bounded by `REPLACEMENTS_KEPT` (10,000 by default, about 3 MB): the oldest replacement goes first. On mainnet the tracker counted 45 replacements in 15 minutes on one run and 197 in 13 minutes on another (2026-11-12, children included, mostly in a burst), so the default reaches back between about half a day and two days; a burst shortens that.

**It survives a restart when the server has a `DATA_DIR`.** Every change is also written to the store's `replacements` table ([`STORE.md`](STORE.md#replacements)), bounded by the same `REPLACEMENTS_KEPT`, and the record is read back from it when the mempool engine starts, before the first tracker delta is folded in. So the transaction page's "replaced by", the `tx:` topic and the `/api/v1` routes still know a replacement after the server restarts. Writing never slows the tracker or the task that follows it: each delta's changes are queued for one writer task per network without waiting, and dropped (memory keeps them, the log says so) if 64 writes are already waiting. Replacements that happen while the server is down are not seen: the node does not report them. Without a `DATA_DIR` the record lives in memory only and a transaction replaced before the start reads `unknown`.

**It follows reorgs from the tracker's own blocks** (`Replacements::follow`, `asks`, `answers`). Every tracker delta carries the window of recent blocks it was classified against (shared, not copied: six `Arc`s), and the task that folds deltas into the record hands it the window first. So the record learns of a reorg exactly when, and from the same chain, the tracker does; there is no second detector. A replacement is *mined* (in a block on the best chain, at the height and hash recorded), *not mined*, or *unknown* (its block left the best chain and where it is now is not known yet). With each window:

- **The usual case, a new block on the chain followed:** the window's tip is compared with the last one, and each new block's transactions are looked up in the record. A replacement there is mined there, though we never saw it leave the mempool (evicted, then mined after all). A replaced transaction there was confirmed itself (the original won, after a reorg or from a miner that never saw the replacement): it no longer stands replaced, and a client following it is told it is confirmed. Nothing is asked of the node.
- **Anything else** (a reorg, the first window after a start, more new blocks than the window) re-examines every mined replacement. Where the chains part is the highest block we knew (the last window, or any replacement's block) that the new window still shows. A replacement whose block the window shows gone is placed in the window's block that holds it; failing that, if the window shows where the chains part, it is not mined, because every best-chain block above that point is in the window; otherwise it is unknown. A one- or two-block reorg is always settled from memory.
- **What the window cannot settle is asked of the node,** one Esplora read at a time, stopping at the first failure: for each unknown replacement, its confirmation status (`/tx/:txid/status`); and, when the window does not show where the chains part, the hash at the height of the highest mined replacement below the window (`/block-height/:h`), walking down until a recorded block is still there, which proves everything below it. One reorg (or start) may cause at most `CHECKS` (100) reads, failed ones included. Past that, what is still unproven is *unknown* and stays so until the next reorg or until it leaves the record. A read that fails leaves the state as it is (unknown stays unknown; a mined replacement below the window is not called anything else until proven otherwise) and is tried again 30 s later. Answers that arrive after another reorg are dropped: that reorg re-examined everything again.
- **Readers can tell the three apart:** `mined` is `true`, `false` or `null` on the `tx:` topic's links and `/app/tx/:txid/replacements` ([`WS.md`](WS.md#following-a-transaction-or-an-address)), and `true`, absent or `null` in the `/api/v1` trees ([`API.md`](API.md#replacement-trees)). A change caused by a reorg is sent to followers at once.
- **Restarts:** the state, block included, is stored, so an unknown replacement is checked after a restart. The first window after a start cannot show where it parts from what was stored, unless a stored replacement's block is in it; then the walk down costs one read, or none.
- **Limits.** Links stored before blocks were recorded (store migration 0008) read mined with no block, and no reorg re-examines them; the record turns over within about two days. A replaced transaction confirmed by a reorg in a block below the window is not noticed: only its replacement's status is asked. Nor is a replacement that was not mined before such a reorg (it had left the mempool without confirming) and that the new chain confirms below the window: it stays not mined, as a replacement mined while the tracker was stalled for more than the window always has ([Where it can only infer](#where-it-can-only-infer)). Asking about every unmined replacement instead would cost up to `CHECKS` reads at every start, which also cannot see where the chains part, and leave the rest unknown. On mainnet a reorg cannot be arranged, so that path rests on the tests and the regtest run.

The mempool totals' `min_fee_rate` is the node's `mempoolminfee` from the tracker's own `getmempoolinfo` probe, which `Tracker::node_info` exposes; no extra call is made for it.


## Fee recommendations

`engine::fees` answers the question most people bring to an explorer: what should I pay to be mined within a given time? It reads the answer off the projected blocks and the recent arrival of competing transactions. The routes are in [`API.md`](API.md#fees), the topic in [`WS.md`](WS.md#mempool-topics).

| Part | File | Does |
|---|---|---|
| Method | `crates/engine/src/fees.rs` | Depth, arrivals, the search, smoothing, the node's fallback. Pure: no I/O, no clock |
| In the server | `crates/server/src/ws/fees.rs` | Feeds it from the tracker and projector, falls back, publishes, holds the answers |
| Proof | `crates/engine/examples/fee_watch.rs` | Records our answers, the node's and the reference's against a live node, and scores each against the blocks that follow |

### Levels

| Level | Mined within | The reference's field |
|---|---|---|
| `high` | the next block | `fastestFee` |
| `medium` | 3 blocks, about half an hour | `halfHourFee` |
| `low` | 6 blocks, about an hour | `hourFee` |
| `no_rush` | 144 blocks, about a day | `economyFee` |
| minimum | accepted by the node now | `minimumFee` |

Rates are in sat per 1,000 vB, the node's unit and resolution, so every answer has three decimals in sat/vB.

### The method

For a level of `N` blocks, the answer is the lowest rate `r` for which

> (virtual size in the mempool paying `r` or more) + (virtual size expected to arrive paying `r` or more before the `N`-th block) < `N` × 998,000 vB

where 998,000 vB is a block's room for transactions (4,000,000 weight units less the 8,000 a template keeps for the coinbase), and every rate is a transaction's **effective** rate: the rate of the chunk the node will mine it in, so a parent lifted by its child counts where the node would put it. Both terms only shrink as `r` grows, so the lowest such `r` is found by bisection over whole sat per 1,000 vB, between the node's minimum and one step above the highest rate anyone pays.

Why each part is as it is:

- **The bottom of a block sets the price, not its middle.** A new transaction gets into block `N` when everything that outbids it fits in `N` blocks: that is the depth term. Reading the median of a block instead would overpay by half a block's spread on every level.
- **Ties count against us.** Transactions at exactly `r` are counted as ahead, because the node orders equal rates in an order nobody outside it can know ([Limits](#limits)). With thousands of chunks at one rate at a block boundary, as mainnet holds today, the answer is one step (0.001 sat/vB) above that rate, never the rate itself. A block exactly full of transactions at `r` likewise has no room at `r`.
- **A block that is not full means the minimum is enough,** because the depth term is already under `N` blocks at the node's minimum; so is an empty or nearly empty mempool, where every level is the minimum. Only arrivals can lift it then.
- **Arrivals.** Blocks come every ten minutes on average, and every transaction that arrives before then paying more than ours goes ahead of it. The method counts, per minute, the virtual size of transactions that entered the mempool at each rate over the last 30 minutes (from the tracker's change stream, at their own rate), and expects that rate to continue. It counts them up to the time by which the `N`-th block has come with 80% probability, block intervals being exponential: 16.1 minutes for `high`, 42.8 for `medium`, 79.1 for `low` and 25.7 hours for `no_rush`. 80% rather than the mean because the cost is lopsided: too low leaves a payment stuck, while too high costs, on today's mempool, a fraction of a sat/vB.
- **One burst is not a trend.** The busiest of the 30 minutes is left out of the arrivals once there are five, so a single burst (a consolidation, an exchange's batch) is not extrapolated over the horizon. It is in the mempool already, where the depth term counts it.
- **Never an absurd rate.** One transaction paying 10,000 sat/vB adds its own size, at most 100 kvB, to the depth of every rate below it: it cannot fill a block. The answer is the minimum or one step above a rate some transaction pays, and only where more than the level's room pays that rate or more; it is never above one step over the highest rate seen. A test puts one in the mempool and one in the arrivals and shows nothing moves.
- **Never below the node's minimum.** Every level is at least `mempoolminfee` and at least 0.1 sat/vB, rbitcoin's relay floor, and a faster level is never below a slower one. The construction already guarantees both; the result is still put in order, as a guard.
- **Stable.** Recomputed every 2 s, each level is the highest of the last 60 seconds since the tip last moved: it rises at once and falls only after a minute, so it does not flicker with every arrival at a block's edge. The cost is freshness on the way down: after the mempool thins, a level can stay up to 60 seconds too high. A new block clears the history, because it changes the picture for real.
- **Five minutes of warm-up.** Until five whole minutes of arrivals were watched, the arrival term cannot be trusted, and the node's estimates are served instead. Only time with a current projection counts: after the projection was stale or missing for a minute or more (the node was down), the arrivals are thrown away and the warm-up starts over, so an outage is never taken for quiet minutes.

### When ours cannot be served

| Condition | Served | Why |
|---|---|---|
| The first five minutes after start | The node's estimates | Arrivals not yet known |
| The projection is stale (the node is failing) | The node's estimates, when it still answers them | An old projection is not the mempool now |
| `{NET}_MEMPOOL_ENABLED=false` | The node's estimates | No projection at all |
| The node's estimates older than 90 s, or never received | Nothing: `503`, and the topic says `unavailable` | An old recommendation presented as current costs money; a wallet that gets nothing uses its own |

The node's estimates are rbitcoin's `/fee-estimates` (a ten-minute inclusion product, not Bitcoin Core's historical estimator, [`RBITCOIN.md`](RBITCOIN.md)): targets 1, 3, 6 and 144 blocks (or the next longer target the node gives), lifted to its minimum (`getmempoolinfo`, asked with them, since the tracker's copy is as old as its last good cycle) and put in order, and marked as the node's everywhere they appear. While they are served they are also lifted to any higher minimum the tracker sees in the meantime. The task waits at most 5 s for them, and withdraws whatever it served before it starts waiting, so a hanging node cannot keep an old answer up; the routes themselves refuse estimates past 90 s, and the topic withdraws them at the next tick after they expire, or at the moment they expire while a refresh is pending. They are asked every 30 s, and only while they are what is served and someone wants them: a fee route was requested in the last ten minutes, or the `fees` topic has a subscriber. So nobody asking means no node call, and the first request after a quiet spell is a `503` that the next one, a moment later, does not get.

### Against the blocks that followed

A recommendation is a claim about the future, so it is scored against what happened. `fee_watch` records, every 30 s, our answer, the node's `/fee-estimates` mapped as in the fallback, and (every ten minutes) the reference backend's `/v1/fees/precise`. For every block mined it takes the **clearing rate**: the 1st percentile by vsize of the effective rates our last projection before the block gave its transactions, so one prioritised or out-of-band transaction does not set it. A level met its horizon when it paid at least one step above the clearing rate of one of the next `N` blocks after the moment it was given; its overpay is how far above the cheapest such rate it was.

What the scorer leaves out, and how it says how sure it is:

- **Outages.** A gap between two recorded blocks longer than two hours (`MAX_GAP_SECS`; on a live chain about once in 450 years) means the recorder's node lost the network. Samples whose horizon reaches into that gap, or into the 30 minutes after it while the method's arrivals still hold the quiet and the mempool refills, are left out and counted.
- **Blocks we never saw.** A block none of whose transactions our projection held has no clearing rate. A sample whose whole horizon is such blocks is counted apart (`unsc.`), not scored as a miss.
- **One block decides many samples.** Every sample waiting for the same block shares its fate, so 2,000 samples over 100 blocks are nearer 100 outcomes than 2,000. Each met share has a 95% interval clustered on the block the samples waited for.
- **Fast and slow horizons.** Each met share is also split by whether the `N`-th block came within `N` × 10 minutes of the sample.

```bash
NET=MAINNET SECS=3900 OUT=fees.jsonl REFERENCE_URL=<reference>/v1 cargo run --release -p engine --example fee_watch
RESCORE=fees.jsonl cargo run --release -p engine --example fee_watch   # score a recorded run again
# INPUTS=1 also records what a replay needs (every arrival, each sample's depth), SAMPLE_SECS=10
REPLAY=fees.jsonl cargo run --release -p engine --example fee_watch    # replay it at several tunings
```

**Run of 2026-11-12,** mainnet, read-only, 65 minutes, 58,000–62,000 transactions, 131 samples (108 with our own answer after the warm-up), blocks 975,530 to 975,533:

| Level | Source | Samples | Met the horizon | Mean overpay when met | Mean shortfall when missed |
|---|---|---|---|---|---|
| next block | ours | 108 | 64.8% | 0.436 sat/vB | 0.462 sat/vB |
| | the node's estimate | 108 | 100% | 2.004 sat/vB | |
| | the reference | 5 | 60% | 1.194 sat/vB | 0.290 sat/vB |
| 3 blocks | ours | 97 | 74.2% | 0.072 sat/vB | 0.035 sat/vB |
| | the node's estimate | 97 | 100% | 0.639 sat/vB | |
| | the reference | 5 | 80% | 0.857 sat/vB | 0.030 sat/vB |

At the five moments the reference was asked, ours met the next block 4 times in 5 (mean overpay 0.331 sat/vB) and 3 blocks 4 times in 5 (0.098 sat/vB).

**How thin this is.** Four blocks came in 65 minutes, so the 108 samples are four outcomes, not 108: every sample waiting for the same block shares its fate. All of our misses are one block: 975,531 came 34 minutes after the one before (a gap that long happens about 3% of the time), and while it was awaited the market rose from about 1.1 to 2.0 sat/vB; the samples taken early in that wait were too low, those taken late were right. For the other three blocks every sample of ours met both horizons. Six blocks never followed a sample inside the run, so the hour and day levels are not scored at all, and the reference was asked only five times. The node's estimate met every horizon by paying two to four times what sufficed. A run of a day or more is the follow-up; `fee_watch` is built for it.

**Run of 2026-11-12 to 13,** mainnet, read-only, 24 hours from 09:44 UTC, 2,881 samples (2,854 with our own answer), 121 blocks (975,537 to 975,658). From about 23:11 to 03:54 UTC the node lost its internet connection: its tip stayed at 975,611 for 4.8 hours, then 16 blocks came within a minute. The scorer leaves that span and the half hour after it out (623 to 730 samples, depending on the level). The 11 blocks without a clearing rate are all from that catch-up: our mempool, cut off from the network, held none of their transactions. None was empty.

| Level | Source | Samples | Met the horizon | 95% interval | Mean overpay when met | Median overpay | Mean shortfall when missed | Met when the blocks came faster / slower than `N` × 10 min |
|---|---|---|---|---|---|---|---|---|
| next block | ours | 2,225 | 72.3% | 63.6–81.0% | 0.659 sat/vB | 0.679 | 0.799 | 99.1% / 28.6% |
| | the node's estimate | 2,225 | 93.3% | 88.2–98.3% | 1.634 | 1.694 | 0.534 | 100% / 82.2% |
| | the reference | 112 | 75.9% | 69.0–82.8% | 1.001 | 0.879 | 0.626 | 94.1% / 47.7% |
| 3 blocks | ours | 2,164 | 71.2% | 60.3–82.1% | 0.442 | 0.314 | 0.678 | 97.0% / 42.0% |
| | the node's estimate | 2,164 | 68.9% | 55.6–82.1% | 0.398 | 0.323 | 0.977 | 89.5% / 45.5% |
| | the reference | 109 | 84.4% | 77.3–91.5% | 0.758 | 0.663 | 0.592 | 100% / 67.9% |
| 6 blocks | ours | 2,063 | 64.7% | 52.0–77.4% | 0.243 | 0.008 | 0.709 | 87.8% / 40.3% |
| | the node's estimate | 2,063 | 82.5% | 70.5–94.5% | 0.428 | 0.329 | 1.085 | 100% / 63.9% |
| | the reference | 103 | 73.8% | 63.2–84.4% | 0.442 | 0.271 | 0.538 | 96.1% / 51.9% |

At the moments the reference was asked, ours met the next block 75.0% of the time, 3 blocks 67.9% and 6 blocks 61.2%. Left in, the outage flatters every level (76.8%, 76.6%, 73.9%): through it ours sat one step above the mempool's floor, and the catch-up blocks cleared at that floor.

**What the misses were.** Each missed sample was put to the first of these that explains it: a block of the horizon that had room but did not take our rate (a miner's floor); our rate tied with the clearing rate (short one step); the mempool already holding `N` blocks above our rate when we gave it (the depth term); the `N`-th block coming later than the time the method allowed for it; and otherwise arrivals, more of which came in time than were counted.

| Level | Misses | Time allowed | Arrivals | Miner's floor | Ties | Depth |
|---|---|---|---|---|---|---|
| next block | 616 | 74% | 24% | 2% | 0% | 0% |
| 3 blocks | 623 | 71% | 22% | 5% | 1% | 0% |
| 6 blocks | 728 | 56% | 27% | 7% | 10% | 0% |

- **The time allowed is most of it.** The method counted arrivals up to the time by which the `N`-th block comes with 80% probability, not the mean. So by design one horizon in five runs past it, and every level was tuned to the same 80%: that is why 3 and 6 blocks were no more reliable than one. In this recording more than one in five ran past (22% for one block, 27% for three and six) because blocks were slow: 10.8 minutes apart on average, a median gap of 7.9 minutes and a 90th percentile of 29.9, against 10.87 minutes over the difficulty period and 11.5 over the week before. A sample whose horizon ran past the allowance was met 8% (next block) to 24% of the time; one within it 91%, 89% and 80%.
- **A block that clears at 3 sat/vB is a full block after a slow gap.** All 17 blocks that came more than 20 minutes after the one before cleared at 1 sat/vB or more, 12 of them at 2 or more; after gaps under five minutes the median clearing rate was 0.123. Those blocks were full (3.99 million weight units) and their pools took 0.12 sat/vB at other times. A steady stream at 1 to 3 sat/vB arrives at about a block every ten minutes, and the cheap end of the mempool is only reached when blocks outpace it.
- **Arrivals are undercounted.** Within the time allowed, the 6-block level still missed one sample in five, mostly while demand rose (16:00 to 18:00 UTC) or a steady stream at one rate (0.125 sat/vB) kept refilling the boundary. The arrival term was the mean of the quieter three quarters of the last 30 minutes, not, as this page said, all but the busiest one.
- **A miner's floor is real but small.** Four of the 100 blocks had room and took nothing under 1.000 sat/vB while transactions at 0.12 waited (Ocean twice, ViaBTC, BTC.com). A rate under 1 sat/vB cannot get into such a block, and nothing in the mempool shows which pool finds the next one ([Limits](#limits-1)).
- **Depth and the 60-second hold never caused a miss.** The hold only raises a level.

### Against the node and the reference

The same moments, the three sources side by side (sat/vB; next block / 3 blocks / 6 blocks / no rush / minimum):

| Moment (tip) | Ours | The node's estimates | The reference |
|---|---|---|---|
| 975,529 | 2.001 / 1.056 / 0.126 / 0.1 / 0.1 | 3.738 / 1.574 / 1.486 / 0.128 / 0.1 | 2.5 / 1.754 / 0.815 / 0.2 / 0.1 |
| 975,530 | 2.034 / 1.099 / 0.126 / 0.1 / 0.1 | 3.263 / 1.556 / 1.47 / 0.128 / 0.1 | 1.641 / 0.971 / 0.423 / 0.2 / 0.1 |
| 975,530 | 1.303 / 1.004 / 0.125 / 0.1 / 0.1 | 3.296 / 1.376 / 1.332 / 0.128 / 0.1 | 1.803 / 1.404 / 0.64 / 0.2 / 0.1 |
| 975,530 | 2.032 / 1.099 / 0.141 / 0.1 / 0.1 | 3.929 / 1.793 / 1.389 / 0.128 / 0.1 | 2.911 / 2.005 / 1.378 / 0.2 / 0.1 |
| 975,530 | 2.626 / 1.193 / 0.141 / 0.1 / 0.1 | 4.425 / 2.001 / 1.644 / 0.128 / 0.1 | 3.527 / 2.367 / 1.56 / 0.2 / 0.1 |
| 975,533 | 1.001 / 0.125 / 0.121 / 0.1 / 0.1 | 2.152 / 0.472 / 0.472 / 0.128 / 0.1 | 1.016 / 0.571 / 0.223 / 0.2 / 0.1 |

Ours and the reference's next-block numbers are close; below that they part. The mempool at the time held about three blocks above 0.125 sat/vB and then some 25 MvB of transactions at 0.12 sat/vB and below, so a transaction at 0.126 sat/vB is ahead of all of those and, unless more than three blocks' worth arrives above it, mined within six blocks: that is our `low`, where the reference said 0.4 to 1.6. `no_rush` is the minimum because the whole mempool plus a day of arrivals at the observed pace fits in 144 blocks; the reference's economy level read 0.2 (twice its minimum) at every moment. Neither longer level could be scored in this run.

A recorded moment of this run (the projection's rates and sizes, 30 minutes of arrivals and the answer) is `fixtures/fees/mainnet.json`, replayed by a test.

## When a block is found

The moment a block is found is when the projected blocks matter most, and when the parts above meet: Electrum announces the block, the tracker learns what left, the projector repacks, and the WebSocket sends the block, its summary and the new projection ([`WS.md`](WS.md#when-a-block-is-found) has the order clients are promised).

**What happens, in order:**

1. Electrum announces block B. The server's hint task calls `Tracker::block_found(B)`.
2. The tracker's loop takes it as soon as the cycle in progress, if any, ends. It reads the tip from Esplora, every 25 ms for up to 3 s, until Esplora serves B or a later block. The node can announce a block some hundreds of milliseconds before it serves it ([`RBITCOIN.md`](RBITCOIN.md#rules-that-follow)). It then fetches the block into its window of six, as a cycle would, and publishes one delta. That delta removes everything the window's blocks rule out: what they contain (`Confirmed`), what spent an outpoint they spend, and those transactions' descendants (`Replaced` by the block's transaction). The delta is published even when nothing left, because a new tip is a change. No listing is made.
3. The projector folds the delta in. It asks the node for the clusters the removed transactions belonged to (a child whose parent was mined now stands alone, or in a smaller chunk), packs, and publishes. Then, once the tracker has nothing more to fold in, it asks for every cluster again (the refresh).
4. The WebSocket feed sends a projection on a new tip at once, without the 2 s rate limit, and the fee task recomputes `fees` from it at once. Both hold one whose tip differs from the block the `blocks` topic last announced, for at most 3 s, so a client always sees the block first and nothing older after it.

**What keeps it right:**

- *A block lands during a poll.* The hint waits for the poll to end, and the two never overlap. A poll whose listing came before the block, but whose tip read came after, also removes what the new blocks rule out, though its listing still held it. So no delta on a new tip keeps a transaction known to be mined. The block step then finds nothing left to do. Electrum is not always first: on mainnet a poll read the new tip from Esplora 300 ms before Electrum announced it, with a listing taken just before the block.
- *The block is not served yet.* The tip is read again until it is, for up to 3 s. If it is not served by then, a full cycle runs at once. Until the tracker's tip reaches the announced height, no probe may skip a listing: an empty or quiet mempool's totals do not move with a block, and before this rule such a block went unseen until the 10 s heartbeat.
- *A burst.* Each announcement is applied in turn, each block fetched once. The refresh waits for the last. A newer announcement replaces one still waited for, which may never be served after a reorg.
- *A refresh in progress.* The projector does one thing at a time, so a block's delta that arrives while the previous block's refresh runs (27 requests and 1.6 s on mainnet) waits for it. That needs two blocks within about two seconds.
- *A reorg, or more blocks than the window.* When the new tip does not extend the chain the tracker held, the block step still removes what the new chain rules out, then a full cycle runs at once. Only a listing shows what came back.
- *The audit record.* The projection on the old tip is archived as `Expected` when the first projection on the new one is published, as before.
- *Gapless deltas and a consistent snapshot.* The block step publishes under the same write lock and with the same numbering as a cycle.

**Early against refreshed.** The projection sent for the block is built from the node's answer for every cluster the block touched. So it differs from the refreshed one only by what the block alone does not reveal: a `prioritisetransaction`, a cluster the tracker had not fully seen, ties. On regtest the refresh corrected 0 records in 19 of 20 blocks and 1 record in the other. On mainnet it moved 4,659 records of 68,000 at block 975,535. Most of those moves are not corrections: the refresh applies every cluster afresh, every chunk gets a new id, and ids break ties. So thousands of chunks of equal rate and fee change places, and some cross a block boundary. How many changed rate was not measured. Clients are not told the first projection is provisional: it is as true as anything the server sends, and the refresh's corrections arrive as ordinary changes.

**Measured on regtest** (release build, 22 blocks a few seconds apart, each mining a random ancestor-closed 60% of a mempool of 8 to 87 transactions, the rest staying behind, children of mined parents included). Times are from Electrum's announcement:

| | Before | After |
|---|---|---|
| Tracker delta that confirms the block | 4 ms median, 17 ms max | 2.8 ms median, 5 ms max |
| Projection without the block's transactions | 4 ms median, 20 ms max | about 1 ms later |
| `projected` message on the socket | 1,016 ms median, 1,921 ms max (4.7 s once) | 3.5 ms median, 10 ms max |
| From the `block` message to the `projected` message | 1,015 ms median | 2 ms median, 6 ms max |
| A `projected` on the old tip after the `block` message | none seen | none, and the server holds it back |

So on regtest the whole delay was the feed's 2 s rate limit. With the mempool's totals unchanged by a block (an empty mempool), the tracker could also miss the block until its 10 s heartbeat: the hint's cycle read the tip before Esplora served the block, and every probe after it found nothing changed. That is what made the browser tests that mine see the projection several blocks behind.

**Measured on mainnet** (2026-11-12, read-only, 68,000 transactions in the mempool, release build, one block on the final block path: 975,535). Times from Electrum's announcement:

| Step | ms |
|---|---|
| The tip read, held by the node until it serves the block (the `blocks` topic's own fetch was held as long: its `block` message left at 487 ms) | 487 |
| The raw block, 1.58 MB | 108 |
| What the window rules out (2,930 transactions) and their reasons, over 68,000 held | 61 |
| Tracker delta published | 657 |
| The projector: two cluster requests for what the block touched, while the node also serves the block summary's `getblock` | about 240 |
| Packing | 182 |
| Mempool topics sent on the block | 1,080, which is 592 ms after the `block` message |
| `summary` sent | 1,974, which is 1,487 ms after the `block` message |
| Refresh of every cluster done (12 requests, 5.9 MB) | 2,799 |

No mempool message on the old tip came after the `block` message, and none on the new tip before it. Before the change the same block would have waited for a poll, about 600 ms for the listing and the block, and then for the feed's rate limit of up to 2 s. Builders measured 3 to 6 s on mainnet. An earlier build in the same run showed a further gap: a poll read the new tip 300 ms before Electrum announced it, while its listing still held the block's 2,948 transactions. That build sent a projection on the new tip still holding them. The rule that a poll which sees a new tip drops what the block rules out closes it.

**Node cost per block,** the WebSocket's block fetch and the summary's block listing left aside (both unchanged by this):

| | Before | After |
|---|---|---|
| Tracker | the cycle that sees the block: probe, listing, tip, the raw block, details | one tip read (more only while Esplora lags: one per 25 ms), the raw block; the next cycle fetches no block |
| Projector | the block's clusters, then every cluster | the same |

So a block costs the node one more tip read (about 100 bytes) and no second fetch of its transactions. On regtest the block step made 2 calls and read 2 to 13 KB per block. On mainnet it made 2 calls and read 1.58 MB, the block the poll used to fetch, and the poll after it fetched none. The tracker's polling stayed at 210 to 255 calls and 185 to 255 MB a minute at 68,000 transactions.

## Block audits

Every block we watch arrive is compared with the block we expected on its parent: which transactions we expected and the miner left out, which it included that we did not expect, and a score for how closely the two agree, read as the block's health. The reference backend cannot do this on rbitcoin; we can, because the projection is packed from the node's own mining chunks ([Projected blocks](#projected-blocks)).

| Part | File | Does |
|---|---|---|
| Method | `crates/engine/src/audit/mod.rs` | The comparison, its categories and the score. Pure: no I/O, no clock |
| Store | `crates/engine/src/audit/db.rs` | The `audits` table ([`STORE.md`](STORE.md#audits)) |
| In the server | `crates/server/src/audits.rs` | Runs it when a new block's summary is computed; the routes ([`API.md`](API.md#block-audits), [`FRONTEND.md`](FRONTEND.md#block-audits)) |

### What is compared

- **Expected:** our first projected block as it stood on the parent, the last projection published before the projector moved onto this block (`Projector::expected`).
- **Mined:** the block's transactions, coinbase left out, with fees and sizes from the block listing the summary reads for every new block anyway: `/block/:hash/txs/summary` where the node serves it, else `getblock` at verbosity 2. Both give the same transactions, fees and sizes ([`RBITCOIN.md`](RBITCOIN.md#listing-a-blocks-transactions)).
- **What left with the block:** every transaction in that last projection, in any block, that the first projection on this block no longer holds, with the rate we had placed it at (`Expected::left`). A mined transaction in it was in our mempool; an expected one in it that is not in the block was knocked out by the block.
- **When the block came:** the moment the projector learnt of it, before it asked the node anything for the projection on it (`Expected::at`), the same for whoever computes the summary first. On mainnet that is about 0.7 s after Electrum's announcement ([When a block is found](#when-a-block-is-found)).
- **When each was seen, and its weight:** the node's accept time and the raw weight from the tracker's records, read right after the block was applied.

Nothing here asks the node anything.

### Categories

Each transaction expected or mined falls in exactly one:

| Category | Expected | Mined | Precisely | Counts against the miner |
|---|---|---|---|---|
| `matched` | yes | yes | In both | No: agreement |
| `conflicted` | yes | no | Left our mempool when the block was applied without being in it: the block spent one of its inputs (a double spend the miner saw first), or, rarely, it was replaced or evicted in that moment | No: excluded |
| `fresh` | yes | no | Still in our mempool, accepted by our node less than 60 s (`RECENT_SECS`) before the block came. An accept time after the block is a newer record (it was evicted and sent again) and is not fresh | No: excluded |
| `displaced` | yes | no | Neither of the above, could not have fitted beside everything the block holds, and the block gave its space to transactions we did not expect (`added` or `unseen`) paying at least its rate | No: agreement |
| `missing` | yes | no | Anything else expected and left out | **Yes** |
| `added` | no | yes | In our projection on the parent, but placed in a later block | No |
| `unseen` | no | yes | Not in our projection on the parent at all: we had not seen it, or it arrived in the last second or so before the block | No |

**The score** (`match_rate`) is a share of virtual size, in percent, rounded down to two decimals:

> match rate = (matched + displaced) / (expected − fresh − conflicted)

with each term the virtual size (sigop-adjusted, as the projection packs) of the expected transactions in that category. When nothing is left to count (an empty expectation, or every expected transaction fresh or conflicted) it is 100: nothing could be left out. Rounded down, so 100 is only ever a block with no `missing` transaction.

Why each choice:

- **By weight, not by count.** Block space is what a miner allocates. One 50 kvB consolidation left out costs the block what 200 small payments do; by count it would weigh as one.
- **Only leaving out lowers it.** A miner who adds transactions we did not expect, in room our block would have left empty, has left nothing out. Additions are listed (`added`, `unseen`) and counted, not scored: a miner putting its own or out-of-band transactions in the place of expected ones shows as `missing` once the replacements pay less.
- **Displacement needs a full block and an equal or better trade.** Mainnet mempools hold thousands of chunks at one rate, and at the block boundary the node and every miner pick among them in an order nobody outside can know ([Limits](#limits)). Two limits apply. First, only what could not have fitted is excused: had the miner added every left-out transaction to its block, the block would have passed its room (3,992,000 weight units, `BLOCK_WEIGHT`) by some amount, and no more than that amount is excused. This is counted in raw weight, the limit's own unit, not in the sigop-adjusted size the score weighs by, which for a sigop-heavy transaction can be many times larger. A left-out transaction the tracker no longer holds at the comparison has no known weight: it adds nothing to that amount and is never displaced. A block with space to spare excuses nothing. Second, within that, highest rates first on both sides, a left-out transaction is displaced when it fits whole in the space the block gave to transactions we did not expect paying at least its rate, less what better-paying left-out ones already took. Whole transactions only, so a small addition never excuses a large omission; the cost is at most one transaction per rate left counted against the miner. An `added` transaction counts at the rate we had placed it at, its chunk's, so a parent mined with the child that pays for it counts at their package rate. An `unseen` one counts at its own fee over its size, since we never saw its package: a parent we never saw is under-counted, never over-counted.
- **Fresh is 60 seconds.** Pools hand out new work about every 30 s, and a transaction must also reach the pool's node. A transaction our node accepted within a minute of the block cannot fairly be expected in it. Measured from when the projector moved onto the block (our clock against the node's accept time, so the two machines' clocks must agree to a second or so), not from the header's timestamp, which the miner sets.
- **The innocent causes this cannot see** are still counted as `missing`, which is why a perfectly honest block can score below 100: a transaction our node accepts but others' policy refuses (a large `OP_RETURN` some miners filter, say), one the miner's node never received, a pool that reserves block space below the 3,992,000 weight units we fill, a package we never saw whose parent pays less than what it displaced, a parent whose child lifted it into the block only seconds before (freshness is each transaction's own arrival, not its chunk's youngest member), and a `prioritisetransaction` that lowers one. Each audit's lists name the transactions, so a reader can look.

**What we do not detect,** and so never report: sigop-limited transactions (we do not count sigops, [Limits](#limits)), prioritised or accelerated transactions (no signal reaches us), and full-RBF replacements (rbitcoin is always full-RBF).

### When there is no audit

An audit exists only for a block we watched arrive with a current projection on its parent; it cannot be computed later or backfilled. The routes say "audit not available", the block facts carry `audit: null`, and the log says why (`block_audit_skipped`, [`LOGGING.md`](LOGGING.md#block-summaries)):

| Case | Why not |
|---|---|
| The projector never moved onto the block from a projection on its parent: older blocks filled in at startup or after a gap. Not logged | We did not watch it arrive |
| `no_projection`: the block is the tip, but there is no projection on its parent, because the server started after it (the first block after a start is always this), or the projector had not reached the block within 5 s of its summary being computed | Nothing to compare with |
| `stale`: the projection on the parent was marked stale (the node had stopped answering) | It was not the mempool when the block came |
| `superseded`: the projection on the parent gave way to a different block (a sibling in a reorg, or a later block in a burst), or the tracker had moved past this block before the comparison | The accept times and `left` would describe another moment |
| A summary computed and cached while the projector was more than 5 s behind the node | Its `audit: null` stays: an audit is attempted once, when the block's summary is computed |
| `store`: the write failed | |
| No `DATA_DIR`, or the network's mempool engine off | Nothing to store or compare; said once at startup, not per block |

The stale flag survives the node's recovery: when the tracker's first good cycle after an outage already carries the new block, the projector publishes that first, so the projection archived for the parent is the stale one. An audit is computed once, when its block arrives, and the first one written stands. A block that leaves the best chain keeps its audit by hash; the pages and the series by height show only the best chain's.

### Cost

None on the node: the block's transactions are the summary's listing, already fetched for every new block, and the rest is in memory. In the server, the comparison runs on a blocking thread under the tracker's read lock (0.2 to 26 ms on regtest, 0.7 and 1.8 ms on mainnet), then one write of about 300 bytes of totals plus 45 bytes per listed transaction, at most 1,000 (`MAX_TXS`, the most telling kinds first). The new block's summary waits for its audit, so its facts carry the audit from the start, at most 5 s. Through `getblock` there was no wait on mainnet: the projector had moved onto the block before the `getblock` returned. The listing route answers in tens of milliseconds, so there the summary waits for the projector instead, about 0.7 s after Electrum's announcement ([When a block is found](#when-a-block-is-found)); not yet measured with the mempool engine on, since the shared nodes run with it off.

### On regtest

With the harness node (`docs/REGTEST.md`), its `generateblock` choosing the block's transactions:

| Block | Score | Categories |
|---|---|---|
| The node's template, taking all 7 | 100 | 7 matched |
| Leaves out the 1 sat/vB one of three that waited over a minute | 66.66 | 2 matched, 1 missing |
| One transaction the node never saw, at 10 sat/vB, beside one it had, the 1 sat/vB one left out again | 50 | 1 matched, 1 unseen, 1 missing: the block had room for it, so the better-paying addition excuses nothing |
| Empty, 3 s after an 8 sat/vB arrival | 0 | 1 fresh, 1 missing (the old 1 sat/vB one) |
| 20 blocks mined at once, the mempool empty | 100 for those audited | 8 audited, 2 `superseded`, 2 `no_projection`; the other 8 were no longer the tip when their summaries were computed |
| The tip when the server started | none | `no_projection` |

### On mainnet

2026-11-12, read-only, release build, 77,000–78,000 transactions in the mempool, two blocks audited (the block current at startup was rightly `no_projection`). mempool.space's public figures for the same blocks beside ours:

| Block | Score | Matched | Missing | Displaced | Fresh | Conflicted | Added | Unseen | mempool.space |
|---|---|---|---|---|---|---|---|---|---|
| 975,537 | 99.93 | 4,130 | 5 | 6 | 21 | 9 | 28 | 9 | 99.76: 10 missing, 23 fresh, 26 added, 1 unseen |
| 975,538 (Foundry USA) | 100 | 3,414 | 0 | 0 | 27 | 1 | 20 | 4 | 100: 0 missing, 28 fresh, 20 added, 0 unseen |

The expected blocks agree: our expected fees and weight were within 0.15% of mempool.space's for both blocks (975,537: 6,401,659 sats and 3,991,807 WU against 6,400,089 and 3,991,797).

**Reading 975,537.** mempool.space's ten missing transactions are our six `displaced` and four of our five `missing`; the fifth, a child, it did not count. The six we excuse were left out for 28 transactions we had placed in the next block and 9 we had not seen, paying at least as much, in a block with no room for both: they are the boundary trade the method excuses, and the whole gap between the two scores. Our five:

- Three 99 vB transactions, each one taproot input to one P2WPKH output at exactly 6 sat/vB, first seen 3.5 to 4.7 minutes before the block, all still unconfirmed when checked. One wallet's batch; both explorers expected them. Most likely the pool's template predated them or its node had not received them.
- A parent at 1 sat/vB and its child at 9 sat/vB, one chunk at 5.03 sat/vB. The parent was 205 s old, but the child that lifted it arrived 87 s before the block, so the package was only worth mining for 87 s: an innocent miss that our per-transaction freshness counts (see the limits above). mempool.space did not count the child.

The nine `conflicted` are exactly the nine transactions the tracker saw the block replace. Of the nine `unseen`, one is a 41-output batch at exactly 5 sat/vB, which looks like a pool's own payout. mempool.space saw the others, so they show where our node was behind.

**Reading 975,538.** 100 in both, with the same 20 added. Its 27 fresh transactions arrived 1 to 7 s before the block. Of our 4 unseen, one pays nothing (0 sat/vB): a miner's own or out-of-band transaction.

**Cost, per block.** The audit made no node call: it took 1.8 ms and 0.7 ms after the summary's `getblock` returned, and that `getblock` (1.8 s and 3.1 s here, under the projector's refresh) and the tracker's and projector's block work were the same calls as without it. The projector had moved onto each block before that `getblock` returned, so nothing waited. Stored: 3.5 KB and 2.3 KB of listed transactions.
