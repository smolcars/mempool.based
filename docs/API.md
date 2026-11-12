# Public API

The server serves rbitcoin's Esplora REST API under `/api`: the same routes, status codes, body bytes and content types as the node, behind an allow-list of parsed routes ([`SECURITY.md`](SECURITY.md) rules 1–3). Wallets and tools that speak Esplora can point at it unchanged. This page lists the routes and every way we differ from the node. The code is `crates/server/src/esplora.rs`. The mempool.space-shaped routes under `/api/v1` are [their own section](#mempoolspace-shaped-routes-apiv1).

| Network | Base URL |
|---|---|
| Mainnet | `/api` |
| Signet | `/signet/api` |
| Testnet4 | `/testnet4/api` |
| Regtest | `/regtest/api` |

A disabled network's base is a `404`.

## The contract

Request and response shapes are rbitcoin v0.9.2's: [`docs/esplora-openapi.yaml`](https://github.com/rbitcoin-org/rbitcoin/blob/v0.9.2/docs/esplora-openapi.yaml) and the "Esplora REST surface" section of its [`COMPAT.md`](https://github.com/rbitcoin-org/rbitcoin/blob/v0.9.2/COMPAT.md#esplora-rest-surface). Address and scripthash routes need the node's `--sh-index`.

## Routes

`asof` marks the routes that take `?asof=<blockhash>`.

| Group | Routes |
|---|---|
| Tip | `GET /blocks/tip/height`, `GET /blocks/tip/hash` |
| Blocks | `GET /blocks`, `GET /blocks/:start_height`, `GET /block-height/:height` |
| Block | `GET /block/:hash`, `/header`, `/status`, `/raw`, `/txids`, `/txid/:index`, `/txs`, `/txs/:start_index` |
| Transaction | `GET /tx/:txid`, `/hex`, `/raw`, `/status` (asof), `/merkle-proof`, `/merkleblock-proof`, `/outspend/:vout` (asof), `/outspends` (asof); `GET /txs/outspends?txids=` |
| Output by position | `GET /block-height/:height/tx/:index` (asof), `GET /block-height/:height/tx/:index/out/:vout` (asof), `GET /tx/:txid/out/:vout` (asof), `POST /block-heights/txs/outs` (asof) |
| Address and scripthash | `GET /address/:address` and `GET /scripthash/:hash`, each bare (asof), `/utxo` (asof), `/txs` (asof, `?after_txid=`), `/txs/chain[/:last_seen_txid]` (asof), `/txs/mempool`, `/txs/summary[/:last_seen_txid]` (asof, `?after_txid=` without the path cursor) |
| Mempool | `GET /mempool`, `/mempool/txids`, `/mempool/txids/page[/:last_txid]` (`?max_txs=`), `/mempool/recent` |
| Fees | `GET /fee-estimates` |
| Broadcast | `POST /tx` |

`HEAD` is accepted wherever `GET` is. Any other path under `/api` is a `404`, and a known path with the wrong method is a `405` with `Allow`.

### Not served

| Route | Why |
|---|---|
| `/internal/*` | Bulk routes for the server's own use. They exist only on the node's unix socket and are never reachable from outside |
| `POST /addresses/txs`, `POST /scripthashes/txs` and their `/summary` | One request can ask for up to 300 address-index joins. That is unbounded load for one client to cause (rule 11). Ask per address instead |
| `POST /txs/test`, `POST /txs/package` | Broadcast is the only write we serve (rule 5). Package relay and test-accept may come later with their own limits |
| `GET /block-template` | Off on the node by default, about 5 MB, and the server uses RPC for templates |
| `/blindbit/v1/*` | Silent-payments oracle; not part of the Esplora surface |

## Differences from the node

### Parameters are parsed strictly

Every parameter is parsed into a typed value before the node is asked, and the node is sent a path we wrote from that value. A request that does not parse never reaches the node.

| Input | Node | Here |
|---|---|---|
| Txid or block hash | 64 hex digits, optional `0x`; a bad one is `404` | Exactly 64 hex digits, any case; a bad one is `400` |
| Height, index, vout, `max_txs` | integer | Decimal digits only, at most `u32::MAX`; anything else is `400` |
| Address | Parsed by the node; a bad or wrong-network one is `404` | Parsed for the network in the URL: `400 invalid address` or `400 address is for another network` |
| `?asof=` on a route that does not take it, any other unknown query parameter, or one given twice | `404` for `asof`; others ignored | `400` |
| `/txs/outspends?txids=` | Up to 50; a token that is not a hash gets an empty slot | Up to 50; any token that is not a txid is `400`. `%2C` is accepted as the comma |
| `/block/:hash/txs/:start_index` not a multiple of 25 | `400` | `400`, before asking the node |
| `POST /block-heights/txs/outs` | More than 300 is `422` | Same, and the body is rebuilt from the parsed positions; it must fit in `MAX_BODY_BYTES` |
| Trailing or doubled slash | `404` | `404` |

The node is sent canonical forms: lowercase hex, lowercase bech32 addresses, `/block/:hash/txs` as `/txs/0`, and `/txs/summary?after_txid=X` as `/txs/summary/X`. The node answers each pair the same.

### Error bodies are ours

A successful answer is forwarded byte for byte. Any other answer is replaced by `{"error": "<message>"}` with `Content-Type: application/json`, because the node's error text can name operator flags and internal paths (rule 17). Errors carry no chain-tip headers.

| Cause | Status | Message |
|---|---|---|
| Unknown object, or an `asof` block not on the best chain | `404` | `not found` |
| Bad parameter | `400` | names the parameter, for example `invalid txid` |
| Cursor txid not in that history | `422` | `after_txid not found` |
| Node runs without `--sh-index` | `503` | `address index is disabled on this server` |
| Unpaged address answer too large (`--max-sh-creates`) | `503` | `address history too large for this route; use the paged /txs routes` |
| Tip moved under a pinned read | `503` | `chain tip moved during the request; retry` |
| Node answered `503` for another reason (no tip, no fee estimates yet) | `503` | `node unavailable` |
| Node unreachable, failed, or answered nonsense | `502` | `upstream unavailable` |
| Node too slow | `504` | `request timed out` |

`503` and `429` carry `Retry-After`.

### Headers

- `X-Bitcoin-Chain-Tip` and `X-Bitcoin-Chain-Tip-Height` are copied from the node's answer when it has them.
- `Content-Type` is the node's. No other node header is copied (`X-Powered-By` is dropped).
- **CORS:** every `/api` response carries `Access-Control-Allow-Origin: *` and exposes the two chain-tip headers and `X-Request-Id` ([`LOGGING.md`](LOGGING.md#request-ids)), including refusals (`429`, `503`, `504`), so a web client can read why it was refused. A preflight `OPTIONS` is answered with `204`, methods `GET, HEAD, POST`, header `Content-Type`, cached for a day. Broadcast is included: it needs no credentials and no cookie ever applies, so a cross-origin page can do nothing a plain `curl` cannot, and refusing CORS would only break browser wallets.

### Cache headers

A wrong `immutable` cannot be recalled from a cache, so it is used only where the URL's hash fixes the whole answer and the answer says nothing about confirmation.

| Routes | `Cache-Control` |
|---|---|
| `/block/:hash`, `/header`, `/raw`, `/txids`, `/txid/:index` | `public, max-age=31536000, immutable` |
| `/tx/:txid/hex` and `/raw` of a transaction **without** witness data | `public, max-age=31536000, immutable` |
| `/tx/:txid/hex` and `/raw` of a segwit transaction | `public, max-age=1`. A txid does not commit to the witness, so the bytes can change while unconfirmed or after a reorg |
| Every other successful read | `public, max-age=1` |
| Broadcast, and every error or refusal | `no-store` |

Block JSON has no confirmation status (the node drops a block that leaves the best chain rather than flagging it), so it is immutable. `/block/:hash/status`, `/block/:hash/txs` (each transaction carries a status), `/block-height/:height` and merkle proofs change with a reorg and are not.

## Protection for the node

- **Identical requests share one call** (rule 8). Requests that arrive while the same upstream call is in flight wait for it and get its answer. Once answered, the next request asks the node again. Measured: 300 concurrent identical requests made one upstream call.
- **Hot answers are held for one second:** `/blocks/tip/height`, `/blocks/tip/hash`, `/blocks`, `/mempool`, `/mempool/txids`, `/mempool/recent` and `/fee-estimates`. Their keys are fixed, so this cache holds at most seven answers per network. Measured: 20,303 sequential `/blocks/tip/height` requests in 3 seconds made three upstream calls.
- **Every request** passes the server-wide limits in [`CONFIG.md`](CONFIG.md): body size, timeout, per-client rate and concurrency.

## Broadcast

`POST /tx` with the raw transaction as hex in the body, optionally surrounded by whitespace. The answer is the txid as `text/plain`.

- **Parsed before sending:** the body must decode as exactly one transaction. The node is sent our own serialization of it, never the client's bytes.
- **Size cap:** 400,000 bytes of transaction (800,000 hex characters), the most a standard transaction can be. Larger is `413`.
- **Rate limit,** separate from and stricter than reads: a burst of 5 per client, then one every 10 seconds; past it, `429` with `Retry-After: 10`. Refused bodies count.
- **Rejects** are `400` with a fixed reason: rbitcoin's own reason when it is on our list of known policy and consensus reasons (`min relay fee`, `dust`, `mempool double-spend`, `bad-txns-inputs-duplicate`, …), otherwise `transaction already in the mempool or chain`, `missing or spent inputs`, `script verification failed` or `transaction rejected`.
- **Off switch:** `BROADCAST_ENABLED=false` turns it off for every network; it then answers `403 broadcast is disabled on this server`.

## mempool.space-shaped routes (`/api/v1`)

A subset of mempool.space's own API, for wallets and tools written against it, under each network's `/api/v1` (`/api/v1`, `/signet/api/v1`, …). Which routes exist is the `served` rows of [`compat/api-v1.md`](compat/api-v1.md). Any other path under `/api/v1` is a `404`.

### How every route here works

New routes follow this pattern; the code is `crates/server/src/api_v1.rs`.

- **Same shape, our numbers.** Field names, their order and their JSON types match the reference backend ([`COMPAT.md`](COMPAT.md)). The values are computed by us, from the node; where they differ from the reference's, the route's section says how and why. Clean room: shapes are observed from responses, never read from mempool.space's code or docs.
- **Answered from memory.** A task per network keeps the answer current, woken by the Electrum events the WebSocket hub follows, and does the node fetches. A request never causes a node call (rule 8). Pure logic lives in `engine`, the task in its own server module.
- **Before the first answer:** `503` `{"error": "not ready yet; retry"}` with `Retry-After: 1`.
- **Headers:** `X-Bitcoin-Chain-Tip` and `X-Bitcoin-Chain-Tip-Height` name the tip the answer is as of. When the node cannot confirm the answer is current (Electrum is disconnected, or a newer tip could not be computed yet), `Age` gives the seconds since it was computed; the last answer is still served. `Cache-Control: public, max-age=1`. CORS is that of `/api`.
- **Requests:** `GET` and `HEAD`. Query strings are ignored, so cache-busting parameters work. Errors use the model above.

### Difficulty adjustment

`GET /api/v1/difficulty-adjustment`: where the chain is in its 2016-block difficulty period and what the next retarget is expected to do. Code: `crates/engine/src/difficulty.rs` for the arithmetic, `crates/server/src/difficulty.rs` for the task.

| Field | Type | Meaning |
|---|---|---|
| `progressPercent` | number | Blocks mined after the period's first block, out of 2016 |
| `difficultyChange` | number | Expected change at the next retarget, in percent, if the rest of the period is mined at its average pace so far. Exact once the tip is the period's last block |
| `estimatedRetargetDate` | integer | When the next period's first block is expected, Unix milliseconds: now plus `remainingTime` |
| `remainingBlocks` | integer | Blocks still to mine, up to and including the next period's first block |
| `remainingTime` | integer | `remainingBlocks` × `timeAvg`, milliseconds |
| `previousRetarget` | number | The change at the adjustment that started this period, in percent. `0` in the first period |
| `previousTime` | integer | Timestamp of this period's first block, Unix seconds |
| `nextRetargetHeight` | integer | Height of the next period's first block |
| `timeAvg` | integer | Average time between this period's blocks so far, from their timestamps, in milliseconds. The target spacing (600,000) while the tip is the period's first block |
| `adjustedTimeAvg` | integer | Always equal to `timeAvg` |
| `timeOffset` | integer | Always `0` |
| `expectedBlocks` | number | Blocks the period would have by now at one per 10 minutes |

The projection follows consensus exactly: the retarget measures the time from the period's first block to its last (2015 intervals, so a period at exactly 10 minutes a block still raises difficulty by 0.05%), clamps it to between a quarter and four times two weeks, scales the target and stops at the network's minimum difficulty. Replaying 6 recent retargets on each of mainnet, signet and testnet4 from a synced node gave the bits the next period actually carried, every time; fixtures in `fixtures/difficulty/` keep 10 of them as tests.

The remaining blocks are projected from now, or from the tip's timestamp if that is later, because the retarget measures block timestamps rather than our clock. It is recomputed once per new tip (two small Esplora reads: this period's first block and the previous period's) and on a reorg. A read stamped with a different tip than the one being computed is discarded and retried, so blocks from two branches are never combined.

| Network | What the answer means |
|---|---|
| Mainnet, signet | As above. Signet has its own, lower minimum difficulty |
| Testnet4 | BIP94: a retarget scales the bits of the period's *first* block, because its last is usually a minimum-difficulty block mined after a 20-minute gap. `difficultyChange` and `previousRetarget` describe that real difficulty; minimum-difficulty blocks count toward the period and the average like any other. Miners often stamp blocks up to two hours ahead of the clock, which is why the projection starts from the tip's timestamp |
| Regtest | Never retargets. `difficultyChange` and `previousRetarget` are always `0`; the period fields still count toward height multiples of 2016, where nothing happens. Blocks mined in bulk make `timeAvg` near zero, and the first period's average starts at the 2011 genesis block |

How we differ from the reference backend, measured side by side: progress, heights, block counts and `previousTime` match exactly and `previousRetarget` up to floating-point rounding. The reference's `timeAvg` divides the time from the period's first block to *now* by the blocks mined, so it grows between blocks and is wrong when the tip is ahead of the clock; ours uses the tip's timestamp. Its `difficultyChange` follows from that average: within 0.01 percentage points of ours on mainnet and signet, but about two points off on testnet4, where tips run hours ahead. `estimatedRetargetDate` and `remainingTime` differ accordingly.

### Mining

Who mined each block, each pool's share, the network's hashrate and difficulty over time, and the block fee, reward, fee-rate and size series, all from the block table ([`STORE.md`](STORE.md#blocks)), never from the node. They need `DATA_DIR`; without a table every route answers `503`. Code: `crates/engine/src/mining/` for the pool list, attribution and hashrate arithmetic, `crates/engine/src/blocks/mining.rs` for the table's statements, `crates/server/src/mining.rs` for the routes and their cache.

| Route | Answer | Cached |
|---|---|---|
| `GET /api/v1/mining/pools` | `[{name, slug, unique_id}]`, `Unknown` (id 0) first | yes |
| `GET /api/v1/mining/pools/:interval` | `{pools: [{poolId, name, link, blockCount, rank, emptyBlocks, slug, avgMatchRate, avgFeeDelta, poolUniqueId}], blockCount, lastEstimatedHashrate, lastEstimatedHashrate3d, lastEstimatedHashrate1w}`, most blocks first | yes |
| `GET /api/v1/mining/pool/:slug` | `{pool: {id, name, link, addresses, regexes, slug, unique_id}, blockCount: {all, 24h, 1w}, blockShare: {…}, estimatedHashrate, reportedHashrate, avgBlockHealth, totalReward}` | yes |
| `GET /api/v1/mining/pool/:slug/hashrate` | `[{timestamp, avgHashrate, share, poolName}]`, one a week | yes |
| `GET /api/v1/mining/pool/:slug/blocks[/:height]` | Up to 100 of the pool's blocks strictly below `:height` (from the tip without it), newest first, with `extras` | no: one indexed read |
| `GET /api/v1/mining/hashrate[/:interval]` | `{hashrates: [{timestamp, avgHashrate}], difficulty: [{time, height, difficulty, adjustment}], currentHashrate, currentDifficulty}`; no interval is `all` | yes |
| `GET /api/v1/mining/hashrate/pools[/:interval]` | `[{timestamp, avgHashrate, share, poolName}]`, every pool with a block that week, most blocks first | yes |
| `GET /api/v1/mining/difficulty-adjustments[/:interval]` | `[[time, height, difficulty, adjustment]]`, newest first, the ratio to five decimals | yes |
| `GET /api/v1/mining/reward-stats/:count` | `{startBlock, endBlock, totalReward, totalFee, totalTx}` over the last `count` blocks (1 to 52,560), totals as strings | no: one range sum |
| `GET /api/v1/mining/blocks/fees/:interval` | `[{avgHeight, timestamp, avgFees}]` | yes |
| `GET /api/v1/mining/blocks/rewards/:interval` | `[{avgHeight, timestamp, avgRewards}]` | yes |
| `GET /api/v1/mining/blocks/fee-rates/:interval[/precise]` | `[{avgHeight, timestamp, avgFee_0, avgFee_10, avgFee_25, avgFee_50, avgFee_75, avgFee_90, avgFee_100}]` in sat/vB: whole numbers, or three decimals with `/precise` | yes |
| `GET /api/v1/mining/blocks/sizes-weights/:interval` | `{sizes: [{avgHeight, timestamp, avgSize}], weights: [{avgHeight, timestamp, avgWeight}]}` | yes |
| `GET /api/v1/mining/blocks/timestamp/:timestamp` | `{height, hash, timestamp}`: the newest block stamped at or before a Unix time, its time as ISO 8601 | no: one indexed read |

Hashrates are whole hashes a second and can pass 2⁶⁴ (mainnet's is about 9.5 × 10²⁰), so they are written as integers of up to 39 digits, as the reference writes them. Read them as floating point.

#### Parameters

- **Intervals** are exactly `24h`, `3d`, `1w`, `1m` (30 days), `3m`, `6m`, `1y` (365 days), `2y`, `3y` or `all`, counted back from now. Anything else is `400 invalid interval`.
- **Slugs** are looked up in the list; an unknown one is `404`. **Heights, counts and timestamps** are decimal digits only (`400` otherwise); `reward-stats` takes 1 to 52,560.
- **Points.** The block series average this many blocks a point, so no answer holds more than about a thousand: one up to `1w`, then 6 (`1m`), 18, 36, 72 (`1y`), 144, 216 and 1,008 (`all`), grouped by height. `avgHeight` is the middle height of a point and `timestamp` the middle of its blocks' times. The hashrate chart has a point a day up to `1y` and a week (Monday 00:00 UTC) beyond; the per-pool shares are weekly.

#### Coverage

The table is only as complete as the backfill ([`STORE.md`](STORE.md#backfill)), and on a fresh install it is still filling in, newest first. Every answer rests only on the run of rows from the tip down to the first missing height; rows below a gap are left out, so a chart never shows a hole as a dip. Three headers say how far an answer reaches, exposed to cross-origin callers like the chain-tip headers:

| Header | Value |
|---|---|
| `X-Data-From-Height`, `X-Data-From-Time` | The earliest block the answer includes, and its timestamp (Unix seconds). Absent when it includes none |
| `X-Data-Complete` | `true` when the answer covers the whole period asked for (all of history for `all`, a pool's statistics and its blocks); `false` otherwise |

A chart can then say "data from … onward" instead of drawing a false zero. Fee rates exist only for rated rows, the last year by default: a fee-rate point appears only when every block in it is rated, so an average of some is never presented as the whole, and a gap leaves `X-Data-Complete` `false`. A timestamp before the stored history is `404` (`no indexed block at or before that time`) rather than a wrong block. `reward-stats` starts at the stored history when asked for more (`startBlock` says where).

#### Who mined a block

**The list.** `crates/engine/data/pools/pool-list.json` is [bitcoin-data/mining-pools](https://github.com/bitcoin-data/mining-pools)' generated list (149 pools, 174 tags, 201 addresses), copied unchanged from the `generated` branch at commit `0158b59` (2026-06-30). Its licence is MIT, read in its `LICENSE` file: copyright btc.com (2019–2021), whose list it forked, and 0xB10C and contributors since. MIT allows redistribution in this MIT project on the one condition that the notice travels with it, so `LICENSE` and a `README.md` naming the source and commit sit beside the file. mempool.space's own pool list was not used or opened. Pool ids are the list's; slugs are each name's ASCII letters and digits, lowercased.

**The rules,** in order (`Pools::attribute`):

1. **A payout to a pool's listed address wins.** Only the pool holds that address's key; anyone can write any tag. A block tagged for one pool that pays another's address is the other's. Among several payouts, the first in output order decides.
2. **Otherwise the coinbase tag:** the longest listed tag found anywhere in the coinbase script, ignoring ASCII case. Longest first, so a tag that contains another (`/ViaBTC/TATMAS Pool/` holds `/ViaBTC/`, `/solo.ckpool.org/` holds `ckpool`) wins over it. At equal length, the one earlier in the script, then the lower id.
3. **Otherwise `Unknown`** (id 0), counted and ranked like any pool, never dropped.

A tag is a claim without proof: a block that writes a pool's tag and pays an address not on the list is that pool's. Pools move payout addresses more often than lists follow (in the sample below, 3,615 of the 3,883 blocks attributed by tag belong to pools whose listed addresses they did not pay), so refusing would turn most of them into `Unknown`.

**Hostile input.** The tag and payout scripts are written by whoever mined the block and are stored bounded (at most 100 bytes, eight scripts of 128): matching is a byte comparison of at most 174 tags of at most 30 bytes over 100 bytes and eight map lookups, allocates nothing per block and cannot panic. Invalid UTF-8 is just bytes. Names, tags and addresses in answers are the list's; the only chain bytes in an answer are a pool block's coinbase, as hex and as one Latin-1 character per byte, serialized as JSON strings.

**Stored, not computed per request.** The block table keeps each row's pool (`pool` column, [`STORE.md`](STORE.md#blocks)), set when the row is written. Measured on a 975,000-row table: counting blocks per pool over all of history takes 0.2 s from the column, while attributing at query time means reading and matching every coinbase, 1.8 s, past the store's 2 s read limit under any load. Writing costs 2 µs a row more. The table records which list and rules (`pool_list.version`, a hash of the file and a rules number) its rows were attributed with; when a build ships another list, the first start redoes every row in batches of 10,000 (1.8 s for 975,000 rows, no node call) before the mining routes answer.

**Quality,** over a bounded mainnet sample of 7,851 blocks (heights 967,676 to 975,526, 54 days): 99.69 % identified, 3,944 by address and 3,883 by tag; 24 `Unknown` (blocks with no readable tag, a `j://<uuid>` tag, or `Sv2/MARA`). No block in the sample had a tag and an address naming different listed pools. For the same week the reference ranked 895 blocks and so did we; 891 were attributed alike. The 4 others carry the tag `/trustpool/` but pay ViaBTC's listed address: the reference says TrustPool, we say ViaBTC by rule 1, and the list has no TrustPool. Two pools are named differently from the reference's list: `Ocean.xyz` (`oceanxyz`, theirs `ocean`) and `BTC.com` (`btccom`, theirs `cloverpool`).

#### Hashrate

A block at difficulty *d* takes *d* × 2³² hashes on average, so the hashrate over a stretch of chain is the work of its blocks over the time they took: the sum of their difficulties × 2³², divided by the seconds from the last block before them to the last of them. This is the estimate `getnetworkhashps` makes.

- **Daily points use the trailing 7 days.** A day's ~144 blocks vary by ±8 % from luck alone; a week's 1,008 by ±3 %, and a change still shows within days. A point appears only when its whole window is stored. Weekly points use the week itself.
- `currentHashrate` and `lastEstimatedHashrate1w` are the last 1,008 blocks; `lastEstimatedHashrate` 144 and `lastEstimatedHashrate3d` 432. A pool's `estimatedHashrate` is its share of the last 24 hours' blocks times the network's hashrate over them; its weekly `avgHashrate` is its share of the week times the week's.
- **Against the node:** on mainnet, `getnetworkhashps 1008` gave 9.51382 × 10²⁰ and ours 9.51368 × 10²⁰; over 144 blocks 9.47105 × 10²⁰ and 9.47091 × 10²⁰ (the node times from the earliest timestamp in the window rather than the block before it). The node's value does not fit a `u64` either, and asking it would be a node call per request; ours is computed from the table.
- **Against the reference:** `currentHashrate` within 0.3 % (its window is not stated). Its daily points move 9 % from day to day on average, ours 1.2 %: it uses one day per point.
- **Retargets** come from the table: the first block of each 2,016-block period against the first of the period before (on testnet4 the last block of a period may be a minimum-difficulty one). Identical to the reference for the month compared.

#### Differences from the reference

| Field or route | Here | Why |
|---|---|---|
| `poolId`, `poolUniqueId`, `unique_id`, `pool.id` | All the list's id | The reference numbers its own database rows; we have one id |
| `regexes` | The list's tags, matched as literal text | The list has tags, not patterns |
| `avgMatchRate`, `avgFeeDelta`, `avgBlockHealth`, `reportedHashrate`, `minerNames` | `null` | Block audits are MI4; no pool reports hashrate to us |
| `USD` in the fee and reward series | Absent | No price feed yet (M3). The reference leaves it out the same way on a network without prices (observed on mempool.space's signet) |
| Block series points | A fixed number of blocks a point (above) | The reference merges blocks mined close together into one point (its 715 points for a week's 895 blocks); where its point is one block, the values are equal |
| `avgFee_*` (whole numbers) | Our size-weighted percentiles, rounded | rbitcoin's sub-sat rates round to `0`; use `/precise` |
| Pool blocks | `id`, `height`, `timestamp`, `difficulty`, `tx_count`, `size`, `weight`, and `extras` with `totalFees`, `medianFee`, `feeRange`, `reward`, `pool`, `avgFee`, `avgFeeRate`, `coinbaseRaw`, `coinbaseAddress`, `coinbaseAddresses`, `coinbaseSignature` (the first payout script's assembly) and `coinbaseSignatureAscii` | The block table holds no header fields (version, bits, nonce, merkle root, previous hash, median time) and no per-transaction totals; asking the node would be a call per request. `medianFee` and `feeRange` are `null` until the block is rated, with three decimals |
| `blocks/timestamp` | At or before the time | The reference answered the same way, not with the nearest block |
| Coverage | `X-Data-*` headers | Not on the reference, whose history is complete |

#### Freshness and cost

An answer is computed once per change to the table and served from memory. A task per network recomputes the answers clients have asked for whenever `follow` writes a new block, and at most once a minute while the backfill adds rows; between those, a request is a lookup. An answer nobody has asked for is computed by the first request, one computation at a time per network. A pool's statistics and weekly shares are computed for every pool together. The routes marked "no" above are a single bounded indexed read each. Headers as for every `/api/v1` route; `Age` appears while the node's tip is not the one the answer was computed at.

Measured on the Apple M4 under the load described in [`STORE.md`](STORE.md#measurements), release build, from `curl` on the same machine. The reference backend (a stock mempool backend in front of the same node) was asked once per route, over the network; the pool routes for Foundry USA.

| Route | Reference | Ours, 7,851 mainnet rows: first / then | Ours, 975,000 rows: first / then |
|---|---|---|---|
| `pools` | 0.09 s | 0.8 / 0.4 ms | |
| `pools/1w` | 23.7 s | 1.2 / 0.3 ms | 1.4 / 0.4 ms |
| `pools/all` | | 2.0 / 0.3 ms | 222 / 0.5 ms |
| `pool/:slug` | 54.2 s | 7.1 / 0.4 ms | 999 / 0.3 ms (every pool's at once) |
| `pool/:slug/hashrate` | 24.0 s | 0.5 / 0.4 ms (computed with the above) | 0.5 / 0.5 ms |
| `pool/:slug/blocks` | 13.9 s | 0.9 ms | 0.9 ms |
| `hashrate/1m` | 23.4 s | 3.4 / 0.4 ms | 4.1 / 0.4 ms |
| `hashrate/all` | | 4.0 / 0.3 ms | 597 / 0.4 ms |
| `hashrate/pools/1m` | 24.4 s | 3.2 / 0.3 ms | 3.6 / 0.3 ms |
| `hashrate/pools/all` | | | 836 / 1.8 ms (2 MB) |
| `difficulty-adjustments/1m` | 0.01 s | 0.8 / 0.3 ms | 0.7 / 0.4 ms |
| `reward-stats/144` | 0.08 s | 0.7 ms | 0.6 ms; 31 ms for 52,560 |
| `blocks/fees/1w` | 23.9 s | 1.4 / 0.4 ms | 1.6 / 0.5 ms |
| `blocks/fees/all` | | | 565 / 0.5 ms |
| `blocks/rewards/1w` | 23.9 s | 1.6 / 0.5 ms | 1.7 / 0.4 ms |
| `blocks/fee-rates/1w` | 23.9 s | 1.5 / 0.4 ms | 1.7 / 0.5 ms |
| `blocks/fee-rates/all/precise` | 404 | | 591 / 0.4 ms |
| `blocks/sizes-weights/1w` | 24.0 s | 1.6 / 0.4 ms | 68 / 1.2 ms (a busy moment) |
| `blocks/sizes-weights/all` | | | 560 / 0.6 ms |
| `blocks/timestamp/:t` | 0.02 s | 0.6 ms | 0.4 ms |

The 975,000-row table is synthetic (realistic sizes, the list's tags, the last year rated): `MINING_BENCH_DIR=<dir> cargo test --release -p engine -- --ignored --nocapture mining_benchmark` fills it and times the statements; the server then runs over it as regtest with no reachable node. With every route and interval asked for, a new block costs the refresh a few seconds of reads on one of the four reader connections, once per block.

## Using it from the frontend

- Call `/api/...` (mainnet) or `/<network>/api/...` with `fetch`. Same origin, so CORS does not apply.
- Treat any non-2xx as `{"error": string}` JSON. The message is safe to show; the status says what to do: `404` not found, `400` bad input, `503` retry later (or, for address pages, the index is off).
- Tip and mempool answers are at most a second old by design; poll no faster than that. For live data use the WebSocket when it exists.
- Difficulty progress is `/api/v1/difficulty-adjustment` ([above](#difficulty-adjustment)). It changes once a block; its time fields can be counted down locally between fetches.
- Mining charts read `/api/v1/mining/…` ([above](#mining)) and must check `X-Data-From-Time` and `X-Data-Complete`: while the backfill runs, history starts later than asked.
- Read `X-Bitcoin-Chain-Tip` to detect a tip change between two fetches of one page, and refetch the page.
- Response bodies are chain data: render them as text, never as HTML (rule 14).
