# What we rely on from rbitcoin

Pinned to **rbitcoin v0.9.2**, still the minimum. This page was written against v0.9.1: its facts were read from that tag's source and docs, or measured against a synced v0.9.1 mainnet node. When the pin moves, re-check this page.

A newer node, built from rbitcoin's `master` after v0.9.2, unlocks `GET /block/:hash/txs/summary` ([below](#listing-a-blocks-transactions)): the server uses it where the node serves it and `getblock` where it does not, so v0.9.2 loses nothing. It also announces a tip only once it can serve the block (PR 1577, [the wishlist](#upstream-wishlist)). No release has either yet.

## What v0.9.2 changed for us

Nothing we depend on. It is a bug-fix release (32 commits) and the Esplora crate is untouched. The changes that reach our interfaces at all:

| Area | Change | Effect on us |
|---|---|---|
| RPC authentication | The auth scheme name is matched in any case | None; we send `Basic` |
| `getindexinfo` | Lists the block filter index | None; we do not call it |
| `submitpackage`, `submitblock` | Fixes for same-txid members and a mid-check sibling block | None for the explorer; regtest mining goes through the same code, so the harness is re-checked on the new image |
| Electrum silent payments | The subscribe address carries its version | None; we do not use it |

Schema stays 27, so a v0.9.1 data directory opens in place. One operator note from the release: a data directory that already connected a block with a re-spent output keeps the wrong spent slot and needs a re-sync.

Upstream references: `COMPAT.md`, `docs/rpc.md`, `docs/lightning.md` and `docs/esplora-openapi.yaml` in the rbitcoin repo.

## Interfaces

| Interface | Flag | We use it for |
|---|---|---|
| Esplora REST | `--esplora-listen` | Blocks, transactions, addresses, outspends, broadcast, fee estimates, block listings (`/block/:hash/txs/summary`, newer than v0.9.2) |
| Electrum TCP | `--electrum-listen` | Push: new headers, tracked scripthashes, tracked outpoints |
| JSON-RPC | `--rpc-listen` | Mempool listing, cluster chunks, block template, block stats, block listings where the node has no listing route (`getblock` verbosity 2) |
| Address index | `--sh-index` | Address and scripthash routes. Off by default in rbitcoin; required for address pages |

RPC accepts a Bearer token or a Core-style cookie over HTTP Basic. There is no `--rpcuser` / `--rpcpassword`.

## What the node gives us for free

- **Transaction and address index.** No separate indexer.
- **Spend records.** `/tx/:txid/outspend(s)` answers who spent an output.
- **Cluster mempool.** `getmempoolcluster` and `getmempoolfeeratediagram` expose the same chunks mining selection uses, so projected blocks need no linearization of our own ([`MEMPOOL.md`](MEMPOOL.md#projected-blocks)).
- **Per-transaction fee, size and weight on disk** (`txstat`), which makes `getblockstats` cheap.
- **Output lookup by position:** `GET /block-height/:height/tx/:index/out/:vout`, and a batch form `POST /block-heights/txs/outs` taking up to 300. Built for Lightning gossip validation and needs no address index.
- **Chain-tip stamps.** Block, transaction and tip routes carry `X-Bitcoin-Chain-Tip` and `X-Bitcoin-Chain-Tip-Height`. If two sequential fetches disagree, the tip moved between them: discard and retry. Address and scripthash routes are stamped with the address index's position, which can trail the tip. Mempool, fee-estimate and broadcast routes carry no stamp (the paged `/mempool/txids/page` does). `?asof=<blockhash>` pins a read to an ancestor.

## Listing a block's transactions

The block summaries, the block audit and the fee-rate backfill need each transaction of a block with its txid, fee, weight and output value. `crates/engine/src/blocks/listing.rs` gets them from one of two sources and gives the same value from either.

| Source | Node | Calls | A recent full mainnet block |
|---|---|---|---|
| `GET /block/:hash/txs/summary` ([rbitcoin PR 1578](https://github.com/rbitcoin-org/rbitcoin/pull/1578)), with `/block/:hash` for the height, times, size and parent, and `/tx/:coinbase/raw` for who mined it | after v0.9.2 | three Esplora | 61 ms and 322 KB |
| `getblock` at verbosity 2 | every version | one JSON-RPC | 1,315 ms and 10.3 MB |

Both measured on mainnet the same afternoon, read-only, paced at 50%, while the node also ran another server's full backfill: the route over 1,000 consecutive blocks, `getblock` over 50 (it is too heavy for more on a shared node). The route answers one `[txid, fee, weight, value]` row per transaction in block order, coinbase first with fee 0; fee and value in sats, BIP141 weight. Its virtual size is `ceil(weight / 4)`, the same as `getblock`'s `vsize`, which is not sigop-adjusted either.

**Which one.** Each network asks its node once Electrum connects, at startup and after every reconnect, since a node can be upgraded or downgraded under us: it lists the genesis block through the route. A `200`, or a `404` (the route's own answer for a block it cannot list, such as a pruned one), means the node has it. v0.9.2 refuses the path itself with ``400 Invalid URL: Cannot parse value at index 1 with value `summary` to a `u32` ``, because it falls through to `/block/:hash/txs/:start_index`; any `4xx` but `404` means `getblock`. No answer leaves it open, and the next block listed tries the route. The answer is logged as `block_txs_source` ([`LOGGING.md`](LOGGING.md#block-summaries)). No setting chooses it.

**When the route fails mid-run.** A refusal (a `4xx` other than `404`) switches the network to `getblock` until the next reconnect asks again. Anything else, a `404` for a block off the best chain, a timeout, a malformed answer, or a row count or coinbase that disagrees with the block's header, lists that one block through `getblock` and keeps the route for the next (`block_txs_fell_back`). A malformed answer is an error value, never a panic: the rows are parsed into typed values, an answer over 4 MiB or over 20,000 rows (a block holds at most 16,666 transactions) is refused, and the header and coinbase must match the rows.

**Identical results.** `cargo run --release -p engine --example listing_equality` lists blocks through both and compares what we derive from them byte for byte: the fee-rate percentiles, the ten fee bands, the miner, the block view's binary records, the `/api/v1` summary rows' txids and weights, and the audit's fees and sizes. On 2026-11-12 it found no difference in 203 mainnet blocks (spread over the whole chain and the newest 20; 31 holding only their coinbase; genesis, block 170, the BIP30 duplicate coinbases at 91,812 and 91,842, a coinbase short of its reward at 124,724 and one claiming nothing at 501,726, the SegWit and Taproot activation blocks, 774,628 with its near-4 MB transaction, the 2024 halving; 326,157 transactions), 50 signet blocks (10 empty, 9,738 transactions) and 50 testnet4 blocks (25 empty, 1,657 transactions).

**What the route does not give.** The coinbase's script and outputs, for who mined the block, and the block's height, times, size and parent: those come from the two small Esplora calls beside it (about 1 KB). Every input's previous output, which the `/api/v1` summary's `flags` would need: neither source gives it ([`API.md`](API.md#blocks)).

When a release contains the route, raising the pin and deleting the `getblock` fallback (marked `shortcut:` in `listing.rs`) is a task of its own.

## Address history cost

What the address and scripthash routes cost the node as an address grows. Read from rbitcoin v0.9.2's `crates/rbitcoin-query/src/scripthash.rs` and its operator docs (`docs/operator/storage.md`, "`--max-sh-creates`"), and measured on mainnet on 2026-11-12 against a build of `master` after v0.9.2 (tip 975,591), read-only, one request at a time, while the node also served another server's backfill. A "create" is a transaction that pays the script, counted once however many of its outputs pay it: the index keeps one transaction key per create and finds the outputs at query time (rbitcoin `SCHEMA.md`, "Class B — scripthash": "create_tx_fk only (no vout in the index)"; `ScriptHashTable::create_count` in `crates/rbitcoin-store/src/scripthash.rs` counts those keys, the same in v0.9.2 and on `master`). So the count in the node's refusal is transactions paying the address, not outputs. For `1HckjUpRGcrrRAtFaaCAUaGjsPx9oYmLaZ` the node counts 3,665,660 creates, where another public explorer reports 5,179,486 funded outputs and 3,700,928 transactions: consistent with that reading (fewer paying transactions than outputs paid, and fewer than all its transactions, which include those that only spend from it), though the two counts were taken at different times and not reconciled.

| Route | Above `--max-sh-creates` (10,000 by default) | What its cost grows with |
|---|---|---|
| `/address/:a`, `/utxo` (and the scripthash forms) | Refused at once: `503 scripthash join exceeds --max-sh-creates (<n> creates, cap <N>)`, in 10–35 ms for every address tried (29,133 to 2,366,550 creates). Counting the creates is cheap | Below the cap, one join of every create and its spend: 0.08 to 0.46 s for addresses of 213 to 701 creates |
| `/txs`, `/txs/chain[/:last]`, `/txs/summary[/:last]` | Served, however long it takes: the cap never applies to a page | See below |
| `/txs/mempool` | Served | The mempool's rows for the script only (at most 50): cheap |

**A page of history** (25 confirmed rows, newest first) checks every create the script has against the best chain (an in-memory lookup each, so a few seconds at most even for millions), then finds the spends that land on the page. Where its 25 rows span at most 8,064 blocks (`MAX_HISTORY_WINDOW`), it reads the inputs of every transaction in those blocks; where they span more (a dormant address), it reads the spend of every create the script has ever had. So a page costs in proportion to the blocks it spans or, for a dormant address, grows with the address's whole size (though far less than in proportion: 41.7 s for 93,135 creates, 95 to 102 s for 3,665,660, below), and a page after a cursor costs the same as the first. The first page is not special.

| Address (creates) | Newest page spans | First `/txs` page |
|---|---|---|
| 213 | 26,122 blocks | 0.33 s, 2.3 MB |
| 594 | about 900 blocks | 0.47 s, 0.7 MB |
| 29,133 | 423 blocks | 10.9 s, 0.15 MB |
| 93,135 | 129,640 blocks | 41.7 s, 0.86 MB |
| 2,366,550 | 1,706 blocks | 10.2 s, 0.64 MB; `/txs/summary` 4.05 s straight after, its blocks then cached |
| 3,665,660 (`1HckjU…`), the largest found | about 29,000 blocks (newest confirmed transaction at 961,655) | `/txs/summary`: 102.4 s, then 95.3 s asked again straight after (3,590 bytes each); measured 2026-11-14 on the same node, otherwise idle. An rbitcoin developer reports its full history in 45 s from cold caches on another machine |

Earlier the same day the address balance chart's builder saw the first summary page of a large address run past 60 s, and a 261-transaction address's fifth page take over 10 s while the node also served a backfill. An address of 1.6 million creates (`1dice8…`, refused at the cap like the rest) was not asked; an earlier estimate of about 12 minutes for it, scaled from the 93,135-create rate, is contradicted by the 3,665,660-create address above, which also reads the spend of every create and took about 100 s. Whether the node stops a page when its client hangs up is not known; the server assumes it does not ([`API.md`](API.md#protection-for-the-node)). What the server does about it: [`API.md`](API.md#very-large-addresses).

## What it does not give us

| Missing | Consequence |
|---|---|
| Any mempool push (no ZMQ, WebSocket or SSE) | We poll the mempool |
| `/internal/*` bulk routes over TCP | They exist only on a unix-socket listener, so bulk fetches need the server on the node's host. All seven routes were verified over a shared socket on regtest ([`DEPLOY.md`](DEPLOY.md)); access is controlled by the socket's group |
| Address-prefix search | Search is exact match only |
| Outpoint pushes for mempool spends | `blockchain.outpoint.subscribe` notifies on each new block only. A spend shows up at the next block, or through `blockchain.outpoint.get_status` |
| A large recent-transactions feed | `/mempool/recent` returns 10 entries; it cannot serve as a change feed |
| The mempool.space `/api/v1` and WebSocket surface | A permanent non-goal upstream. This project is that surface |

## Measurements

Mainnet, 54,297 transactions in the mempool, client and node on the same private network.

| Call | Time | Response |
|---|---|---|
| `getmempoolinfo` | 10 ms | tiny |
| Electrum request (version, headers, mempool info) | 4–5 ms | tiny |
| Position lookup, single | 28 ms | 0.5 KB |
| Position lookup, batch of 3 | 14 ms | 1 KB |
| `getblockstats` | 97 ms | tiny |
| `getblockstats`, every stat, recent block not in the node's cache | about 1 s | tiny |
| `getblockstats` asking for a few totals (`totalfee`, `subsidy`, `total_weight`), batch of 20 | 20 to 30 ms a block | tiny |
| Electrum `mempool.get_fee_histogram` | 151 ms | 5 KB |
| `GET /fee-estimates` | 200 ms | 0.3 KB |
| `getrawmempool` (txids) | 260 ms | 3.6 MB |
| `GET /mempool` | 320 ms | 6 KB |
| `getmempoolfeeratediagram` | 330 ms | 309 KB, 9,441 chunks |
| `getblocktemplate` | 390 ms | 4.9 MB, 7,501 transactions |
| `getblock` verbosity 2 | 550–860 ms | 10.4 MB, 4,200 transactions |
| `/block/:hash/txs/summary` (newer than v0.9.2) | 55 ms | 234 KB, 2,792 transactions (measured later, at the tip; [above](#listing-a-blocks-transactions) for both under the same load) |
| `getrawmempool` verbose | 5.7 s | 39.5 MB |

## Rules that follow

- **Never poll verbose `getrawmempool`.** Use it at most once at startup. Diff txids and fetch details for new ones.
- **Batch RPC calls.** A JSON-RPC array is one HTTP request and one work-queue slot, and answers in order. 1,000 `getrawtransaction` calls in one array take about 300 ms ([`MEMPOOL.md`](MEMPOOL.md)).
- **Ask `getblockstats` for the stats you need.** With no list it also computes its UTXO statistics, about a second per recent block; any short list of totals costs 20 to 30 ms.
- **Compute fee-rate percentiles ourselves,** from the block listing. `getblockstats` rounds to whole sat/vB, and most transactions now pay under 1 sat/vB, so its percentiles come back as zeros.
- **Expect 503 on huge addresses, and slow pages on any.** Unpaged scripthash joins above `--max-sh-creates` (default 10,000) are refused at once. Their paged history is served but can take the node tens of seconds a page (about 100 s measured for the largest found), and so can a sparse small address's ([above](#address-history-cost)): the server refuses the history of an address whose totals the node refused and gives up on a page after 5 s ([`API.md`](API.md#very-large-addresses)). Surface both to the user.
- **Electrum's `mempool.get_fee_histogram` is in sat/kvB with cumulative vsize,** not the protocol's sat/vB per band.
- **Fee estimates are a 10-minute inclusion product,** not Core's historical estimator.
- **rbitcoin's mempool policy is libre-relay class:** 0.1 sat/vB minimum, no dust limit, full RBF. Do not assume Core's standardness rules in the UI.
- **A malformed entry in `/internal/txs/outspends/by-outpoint` is answered as unspent,** not as an error (200 with `{"spent":false}`). Build the entries from typed values only, so a bad one cannot be sent.
- **`getmempoolfeeratediagram` returns one point per chunk,** in mining order, not Core's running total.
- **`getblock` verbosity 2 carries every transaction's fee** (the coinbase has none). Its `vsize` is the plain BIP141 size, not adjusted for sigops. `/block/:hash/txs/summary` gives the coinbase a fee of 0 and weight instead of vsize; the listing turns both into `getblock`'s shape.
- **`getmininginfo.networkhashps` does not fit a `u64`** on mainnet.
- **`getblocktemplate` on signet needs the `signet` rule;** mainnet and testnet4 accept it too, so we always send it.
- **v0.9.2 can announce a tip over Electrum before Esplora serves the block, and not to every subscriber at once.** Right after a new block its async workers, which also serve Electrum and Esplora, are busy deciding how to announce the tip to each peer, rebuilding whole blocks to read one header field ([rbitcoin PR 1577](https://github.com/rbitcoin-org/rbitcoin/pull/1577)). On signet (five blocks, three subscribers each), one subscriber heard of each block 320–470 ms before another, and a request for the block made in that window was held, not refused, for 357–475 ms. On mainnet (2026-11-12, three blocks) our `block` message left 118, 206 and 1,486 ms after the announcement, and one block (975,560) was not served for over 3 s. Testnet4 and regtest showed no gap. A build with PR 1577 (merged after v0.9.2, in no release yet) removes it: Esplora served each new block at the first request, 11–25 ms after our own Electrum subscription heard of it, over three mainnet and five signet blocks, and our `block` message left 11–43 ms after the server's announcement. What we do either way: the hub asks for a new block every 25 ms at first and less often as the wait grows, for up to `WS_BLOCK_WAIT_SECS` (60 s), and the block table keeps looking after that ([`WS.md`](WS.md#blocks)), so a late block costs time, never the block.
- **P2P is BIP324 v2 only.** Irrelevant to us unless something tries to peer with the node.

## Upstream wishlist

Not required. Each would make this project faster or simpler, and is recorded here instead of being patched in.

| Wish | Why |
|---|---|
| A mempool event stream (added, removed with reason, sequence number) | Removes the polling lag of up to one interval and the 260 ms diff |
| `/internal/*` over an authenticated TCP listener | Bulk fetches when the server is not on the node's host |
| Sub-sat precision in `getblockstats` percentiles | We could stop recomputing them |
| A compact per-block transaction listing (txid, fee, weight and value for each transaction) | Delivered upstream by [rbitcoin PR 1578](https://github.com/rbitcoin-org/rbitcoin/pull/1578), in no release yet, as `GET /block/:hash/txs/summary`, and used where the node has it ([above](#listing-a-blocks-transactions)). A year of fee-rate backfill went from about 550 GB read to about 17 GB, measured ([`STORE.md`](STORE.md#backfill)) |
| A new tip announced only once the block can be served | Delivered upstream by [rbitcoin PR 1577](https://github.com/rbitcoin-org/rbitcoin/pull/1577), in no release yet: a just-announced block is served at the first request instead of being held 0.3 to 1.5 s |
| Outpoint pushes for mempool spends | A transaction page could show an output being spent as it happens, without polling |
| Totals, unspent outputs and recent history for any address | Addresses above `--max-sh-creates` get only the count of transactions that paid them, the cap and their unconfirmed transactions today: no balance, no unspent outputs, no confirmed transactions. The most looked-up addresses are these. Specified [below](#totals-and-history-for-any-address) |

## Totals and history for any address

What this explorer would need from rbitcoin to show an address above `--max-sh-creates` like any other, written to be built upstream. It asks only for what the address page and the `/api` pass-through use.

**Why.** Measured on mainnet, read-only ([above](#address-history-cost); 2026-11-14 for the last row):

| Address | Transactions paying it (creates) | Totals | Newest page of history |
|---|---|---|---|
| A busy address | 29,133 | refused, 10–35 ms | 10.9 s |
| A dormant address | 93,135 | refused | 41.7 s |
| The largest found | 2,366,550 | refused | 10.2 s |
| `1HckjUpRGcrrRAtFaaCAUaGjsPx9oYmLaZ`, an exchange | 3,665,660 | refused, 58 ms | 102 s, then 95 s asked again (`/txs/summary`, the owner's node, otherwise idle); 45 s from cold caches reported by an rbitcoin developer on another machine |

So the explorer refuses all of their history ([`API.md`](API.md#very-large-addresses)) and shows the count of paying transactions from the refusal, the cap and the unconfirmed transactions.

**1. A running total per script.** Stored per scripthash beside the address index, and updated in the same write batch as the index:

| Field | On connect | On disconnect |
|---|---|---|
| `funded_txo_count`, `funded_txo_sum` | add each output paying the script | subtract them |
| `spent_txo_count`, `spent_txo_sum` | add each input spending one of its outputs | subtract them |
| `tx_count` | add each transaction touching it, once | subtract it |
| `unspent_count` | funded minus spent, kept so `/utxo` can say how many there are | the same |

That is 6 integers per script ever used, about 50 bytes, and constant work per output and input a block adds or removes. With it, `GET /address/:a` and `/scripthash/:h` answer the same JSON as today, in constant time, for any address: the cap no longer applies to them. A reorg is the disconnect path, so the totals are exact at every tip the index stamps.

**2. Unspent outputs in pages.** `GET /address/:a/utxo/page[/:last_outpoint]`, newest first, at most 100 a page, each `{txid, vout, value, status}` as `/utxo` writes them, and a header or field giving `unspent_count`. Its cost must follow the page: an index of unspent outputs per script by height, or the creates index walked newest first skipping spent ones, bounded by a work limit (point 4). `/utxo` itself can keep its cap.

**3. A history page whose cost follows the page.** `/address/:a/txs/chain[/:last_seen_txid]` and `/txs/summary[...]` answer the same rows as today, but find the spends that land on the page from the page's own creates (a spend pointer per create, or a per-script list of spend heights) instead of reading every input of the blocks the page spans or the spend of every create the script ever had. Then the newest 25 rows of any address would cost about 25 creates' and 25 spends' lookups: an expectation, not measured, but it no longer grows with the address's size or age.

**4. A bound the client sets.** An optional `?max_work=` (rows or index reads) on every paged route, past which the node stops and answers `503` with a fixed message, or the rows it has with a flag, and spends nothing more on it. This is what lets a server promise that one reader costs the node a bounded amount, which nothing today can.

**How the explorer would use it, and fall back.** At connect, as for the block listing ([above](#listing-a-blocks-transactions)), each network asks once: the totals of a known above-cap scripthash cannot be named in advance, so the probe is the new paged `/utxo/page` route on the genesis coinbase's script (any `200` or `404` means the node has the routes; v0.9.2 answers `400` for the unknown path). A node without them keeps today's behaviour: the size refusal, the count of paying transactions and the unconfirmed list. A node with them: the totals route is no longer refused, so the page shows the balance, totals and chart as for any address; history pages are asked with `max_work`, and the server's 5 s history deadline and size verdicts stay as the guard for older nodes. Nothing in the explorer's answers changes shape.
