# Mempool tracking

rbitcoin v0.9.1 has no mempool push, so `engine::mempool` polls the node, keeps its own copy of the mempool in memory and publishes the changes. Projected blocks, the live feed, fee recommendations, transaction tracking and the mempool charts are built on what it emits.

Measurements below are from a synced mainnet node on a private network, 2026-11-12, with about 55,000 transactions in its mempool. RPC and Esplora costs that other parts of the project rely on are in [`RBITCOIN.md`](RBITCOIN.md).

## Shape

| Part | File | Does |
|---|---|---|
| Model | `crates/engine/src/mempool/model.rs` | The state, the diff, removal classification and delta application. Pure: no I/O, no clock |
| Shell | `crates/engine/src/mempool/tracker.rs` | One loop that probes, lists, fetches and publishes; the subscriber API; the events |
| Proof | `crates/engine/examples/mempool_watch.rs` | Runs the tracker against a live node and prints its events |

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

**Cost.** The hash-map slot is 80 bytes. At 300,000 transactions the table has 524,288 slots (one control byte each), about 42 MB, plus the inputs on the heap: about 50 MB in all, or roughly 165 bytes a transaction. On mainnet at 55,000 transactions the whole process, runtime and HTTP clients included, measured RSS_PLACEHOLDER.

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

**Measured:** SYNC_PLACEHOLDER

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

**Measured:** STEADY_PLACEHOLDER

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
