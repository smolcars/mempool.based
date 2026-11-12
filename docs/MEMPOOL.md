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
tracker.tip_changed();          // from the Electrum header subscription; optional
tracker.read(|m| m.get(&txid)); // a point lookup
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
| accept time | 4 | `getmempoolentry` `time` | "first seen"; it is the node's, so it survives our restarts |
| output count | 4 | the raw transaction | finding children of a removed transaction |
| input fingerprints | 16 + 8 per input | the raw transaction | finding what replaced it |

The value is kept for every transaction rather than only the newest: it costs 8 bytes and arrives free with the inputs, which are needed anyway.

Input fingerprints are 64-bit SipHash values of each spent outpoint under a random key chosen at startup, not the 36-byte outpoints: an attacker cannot grind a collision without the key, and a chance collision across a whole mempool is about one in 10⁸.

Everything else is fetched on demand from the node: ancestors, descendants and cluster chunks (`getmempoolcluster`), scripts and addresses (Esplora).

**Cost.** The hash-map slot is 80 bytes. At 300,000 transactions the table has 524,288 slots (one control byte each), about 42 MB, plus the inputs on the heap: about 50 MB in all, or roughly 165 bytes a transaction. On mainnet at 55,000 transactions the whole process, runtime and HTTP clients included, measured 32–48 MB RSS over a 15-minute run.

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

**Cadence.** A cycle starts at least 1 s after the previous one started, and never sooner after the previous one ended than that one took, so the tracker spends at most half its time talking to a slow node. After a cycle in which a listed transaction's details could not be read, the next cycle lists again whatever the probe says. One loop does everything in order: a cycle can never overlap another, and nothing queues behind a slow node. On failure the wait doubles from 2 s to a cap of 30 s, and comes back to 1 s on the next success. A tip hint skips the wait but never interrupts a cycle in progress.

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

A delta carries the added transactions with their records, the removed txids with their reasons (both sorted by txid), the node's `mempool_sequence` for the listing, and the chain tip read after it. A cycle that changes nothing publishes nothing.

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
- every cluster, once per block, right after the block's update is published. Anything we cannot see change, such as a `prioritisetransaction` on the node or a lost recheck, is corrected within a block.

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

When the tip moves, the first block of the last projection on the old tip is kept as `Expected` (six of them), so the block mined on that tip can be audited against what we expected.

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
- **One update behind.** The projection follows the tracker, so it trails the node by up to one tracker cycle (about 1.5 s) plus one cluster request.

### What is exposed for logging

The projector logs nothing itself. `Projector::events()` yields:

| Event | Fields |
|---|---|
| `Update` | per update: tracker deltas folded in, clusters asked, node requests, response bytes, time waiting for the node, time packing, transactions, chunks, records set and removed, `stale`, the published `seq` |
| `FetchFailed` | the error, consecutive failures, transactions pending, the wait before the retry |
| `Resynced` | the projector fell behind the tracker and rebuilt from a snapshot: transactions |
| `SubscriberLagged` | how many deltas the subscriber missed |

## In the server

`crates/server/src/ws/mempool.rs` runs one tracker and one projector per enabled network, from startup, unless `{NET}_MEMPOOL_ENABLED=false` ([`CONFIG.md`](CONFIG.md#the-live-mempool)).

- **Their own node clients.** The tracker gets its own RPC and Esplora clients and the projector its own RPC client, so the byte counts in their events are theirs alone. Each RPC client allows four calls in flight: with the shared client the API uses, at most twelve of the node's sixteen work-queue slots.
- **The tip hint.** A task follows the network's Electrum events and calls `tip_changed` on every new block or reorg, so a block's removals are seen at once instead of at the next cycle.
- **A node down at boot.** Nothing special: the tracker retries with its backoff (2 s doubling to 30 s), the projector waits for it, and the topics publish nothing until the first complete cycle. When the node comes back the initial sync runs and the topics start. A node lost later leaves the last projection in place, marked `stale`.
- **Shutdown.** When shutdown starts, the engines and their tasks are aborted, so no polling continues while sockets drain.
- **The topics** (`mempool`, `projected`, `projected-txs:N`, `transactions`, in [`WS.md`](WS.md#mempool-topics)) are fed by one task per network from the latest projection and the tracker's change stream. No subscription asks the node for anything.
- **The log.** The engines' events become typed lines ([`LOGGING.md`](LOGGING.md#the-live-mempool)): `mempool_synced` once, one `mempool_summary` a minute with the minute's totals, and a line for each abnormal condition, repeats summarised at powers of two.

The mempool totals' `min_fee_rate` is the node's `mempoolminfee` from the tracker's own `getmempoolinfo` probe, which `Tracker::node_info` exposes; no extra call is made for it.

