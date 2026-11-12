# Performance

How fast the server and the site are, how that was measured, how they compare with the stock mempool backend and with mempool.space, and where we miss a budget. The budgets themselves are in [`ARCHITECTURE.md`](ARCHITECTURE.md#performance-budgets). Measured on 2026-11-12 from `master` at `b58df2b`, release build. The scripts are in [`dev/perf/`](../dev/perf/) and [`dev/load/`](../dev/load/), and [How to reproduce](#how-to-reproduce) gives one command for each table.

## Summary

| Budget | Measured | Verdict |
|---|---|---|
| First-load JavaScript, home page: 100 KB gzipped | 67.6 KB gzipped (`bun run size`); 67 KB on the wire with Brotli in Chromium | **Pass** |
| New block, from the node being able to serve it to the WebSocket send: 50 ms | Regtest: 0.7 to 6.1 ms from the node's notification to the last subscriber's queue, 6.1 ms with 9,962 subscribers. Mainnet, after the node's upgrade: 12 and 91 ms (the only two blocks). Before it, on rbitcoin v0.9.2: 13 ms for a block the node served at once; the other four waited 0.40 to over 3 s on the node's Esplora, which the budget excludes ([details](#the-new-block-on-mainnet)) | **Pass** |
| Cached or immutable API response, server time: 10 ms at p99 | Answers from memory: p99 0.04 ms (1 client), 0.4 ms (100), 0.8 ms (1,000). Immutable `/api/block/:hash`, which we pass through to the node uncached: p99 1.8 ms (1 client), 16 ms (100), 49 ms (1,000) on regtest; 75 to 89 ms on mainnet over the network to the node (v0.9.2) | **Pass from memory; miss for immutable pass-through** under concurrency and whenever the node is a network hop away ([proposed task 1](#1-hold-immutable-node-answers-in-memory)) |
| Frame rate during block and mempool animations: 60 fps on a mid-range laptop | 60 fps, no frame over 16.8 ms, through a new block on the home page and through a 601-transaction grid loading, on the machine's GPU. At 4× CPU slowdown: the block still 60 fps; the grid load dropped 1 or 2 frames per run, and one run of three had a 224 ms long task that four profiled runs did not reproduce | **Pass** |

| Comparison | In one line |
|---|---|
| The stock mempool backend, same node, same routes | Ours answers its `/api/v1` routes in 0.2 to 0.7 ms (median, client-side, over loopback); the reference in 10 to 17 ms over a network with a 9 to 10 ms connect time, so about 1 to 8 ms of its own, with p99 from 450 to 820 ms. Its mining routes took 14 to 54 s, ours under 2 ms (recorded earlier, [`API.md`](API.md#mining)) |
| mempool.space's public site, page loads | Ours moves 115 to 231 KB per cold page, 61 to 72 KB of it JavaScript; mempool.space 838 to 1,351 KB, 714 to 1,220 KB of it JavaScript. As a slow phone (4× CPU, Slow 4G): our first contentful paint at 1.7 s and data at 3.5 to 4.2 s (6.2 s for the next block's 3,400 transactions); mempool.space's first contentful paint at 6.3 to 9.1 s. Unthrottled: ours paints in 32 to 48 ms on loopback, mempool.space in 0.5 to 1.1 s across the internet |

The three results that matter most:

1. **Answers from memory hold up; answers that touch the store or the node do not stay under 10 ms with many clients.** One process served 70,000 requests a second from memory at 100 concurrent clients with server time under 0.5 ms at p99. Routes that read the store (`/api/v1/blocks`, a block's summary) cost 8 ms of server time at 100 clients and 46 ms at 1,000, almost all of it building and serializing the same answer again for every request ([proposed task 2](#2-build-store-answers-once-per-tip)).
2. **The WebSocket fan-out is not the limit; the host is.** Queueing a block to 9,962 sockets took 5.2 ms of server time at under one core, and up to 8,000 sockets the last reader had it within 56 ms. At 10,000 sockets on this machine, with clients and server sharing its kernel, delivery stalled for up to 13 s because macOS ran short of network buffers. The server's part stayed at 5 ms.
3. **A single client that floods the API costs us three cores and 25 MB/s of log.** The limits work: the flood's 103,000 requests a second were refused in 0.005 ms each, ordinary clients beside it kept their median and were never refused. But every refusal is an `http_request` log line, 772 MB in 30 s ([proposed task 3](#3-count-refusals-instead-of-logging-each)).

## Where we miss, and what to do

No budget miss had a fix small and clear enough to make in this change: the one budget miss needs a new cache in the Esplora pass-through, and the other findings are not budgets. Each is a proposed task with its evidence. The first is a bug the mainnet run found.

### 0. Follow the block table when a new block's JSON comes late

- **Evidence.** At 13:54:23 UTC the node (still rbitcoin v0.9.2, before the upgrade below) announced block 975,560 over Electrum and its Esplora did not serve the block within the hub's 3 s (`block_unavailable`), so the hub told subscribers to `resync`. The block table follows the tip only when the hub reports a new tip to the summaries ([`blocks.rs`](../crates/server/src/blocks.rs), `follow` waits on `summaries.tips()`), and the hub reports it only after a successful fetch ([`ws/hub.rs`](../crates/server/src/ws/hub.rs), `block`). So the table stayed one block behind until the next block, 13 minutes later: at 14:01, `/api/v1/block/:hash` for the tip answered `503 not ready yet` to all 51 requests, `/api/v1/blocks` left the tip out, and the block got no summary and no audit. The upgraded node fixes the stall that made its Esplora late, so this should now be rare, but any block the hub cannot fetch within 3 s still leaves the table behind.
- **Expected gain.** The table catches up within its retry pause (1 to 5 s) instead of a block interval.
- **Touches.** `crates/server/src/ws/hub.rs` (on the failure path, still wake the table's follower, without starting a summary for a block that was not announced, which the hub's ordering rules out), a test in `crates/server/src/ws/tests.rs`.

### 1. Hold immutable node answers in memory

- **Evidence.** `/api/block/:hash` is immutable (served `immutable`, [`API.md`](API.md#cache-headers)) but every request is a node call. Server p99 on regtest: 1.8 ms at 1 client, 16 ms at 100, 49 ms at 1,000. On mainnet (node on v0.9.2), paced at 5 a second: median 9.2 ms and p99 75 ms, where the same block from memory would be 0.2 ms. The `v1` block routes already keep these headers by hash (`v1_blocks::Headers`, 4,096 of them).
- **Expected gain.** Immutable answers at memory speed: p99 under 1 ms at 100 clients, and no node round trip. The design leaves immutable answers to a CDN in front ([`ARCHITECTURE.md`](ARCHITECTURE.md#http-surface)); without one, the budget is missed.
- **Touches.** `crates/server/src/esplora.rs` (a bounded cache for the routes it marks immutable, keyed by path), its tests, [`API.md`](API.md#protection-for-the-node).

### 2. Build store answers once per tip

- **Evidence.** `/api/v1/blocks` and `/api/v1/block/:hash` make four store reads each (the table's top, the page, the audits, the top again) and serialize 15 blocks (19 KB) per request; a block's summary makes two. At 100 concurrent clients their server median was 8 ms against 0.01 ms for routes answered from memory, and the server spent 4 cores for 22,000 requests a second. A sample of the process under that load shows the time in `serde_json` and hex formatting of hashes, with the runtime threads mostly idle. Giving the store 10 reader connections instead of 4 changed nothing at 100 clients and gained 30% throughput at 1,000 (35 against 46 ms median), so the pool is not the cause.
- **Expected gain.** These routes at memory speed between blocks: the answer for the tip list changes once a block, like the mining answers, which are already computed once and served from memory (0.01 ms).
- **Touches.** `crates/server/src/v1_blocks.rs` and `summary.rs` (keep the serialized answer keyed by the table's top), [`API.md`](API.md#blocks).

### 3. Count refusals instead of logging each

- **Evidence.** One client at 64 connections sent 103,000 requests a second. All but 32 a second were refused with `429` in 0.005 ms of server time, and 100 ordinary clients beside it kept a median of 2 ms and were never refused. But the server used 3 cores (busiest second 4) and wrote 3.1 million `http_request` events, 772 MB in 30 s, and dropped some (`log_events_dropped`). `LOG_REQUESTS=failures` would log the same lines, since a `429` is a failure.
- **Expected gain.** A flood costs the refusal only; the log stays useful. Count `client_rate` refusals per client and log the count at 1, 2, 4, 8… as `connections_refused` already does.
- **Touches.** `crates/server/src/request_log.rs`, `limits.rs`, the event catalogue in [`LOGGING.md`](LOGGING.md).

### 4. Shorten the page's request chain on slow networks

- **Evidence.** As a slow phone every page shows its data 1.9 to 2.5 s after the load event. The home page's waterfall: the HTML (0.58 s), then the entry script, font and styles (to 1.66 s), then the home chunk and 15 small JavaScript and CSS chunks requested together at 1.68 s, which fill the browser's six HTTP/1.1 connections, so `/app/config`, requested at 1.70 s, is answered only at 3.46 s. The socket opens after it and its first message arrives at 4.09 s, then the block summaries at 4.69 s. Each step costs a round trip of about 0.56 s on this profile.
- **Expected gain.** About 1 to 1.5 s on a slow network: fetch `/app/config` from `index.html` (a preload, or the values inlined at build time), and merge the home page's small chunks. Behind a proxy that speaks HTTP/2, which a TLS deployment usually does, the six-connection queue disappears without code; these measurements are plain HTTP/1.1 to the server.
- **Touches.** `web/index.html`, `web/vite.config.ts`, [`FRONTEND.md`](FRONTEND.md).

### 5. Document the host limits for many sockets

- **Evidence.** The server scales past 10,000 sockets (5 ms to queue a block to all of them), but the host did not: at 10,000 sockets on this Mac, with the clients on the same machine, kernel network buffers went from 10,900 to 80,000 of 89,450 in use, above macOS's 80% pressure threshold, and delivery stalled. Connection storms also overflow the listen backlog (`kern.ipc.somaxconn` 128 on macOS): 37 to 354 of 10,000 connections opened 200 at a time were reset.
- **Expected gain.** Operators who expect thousands of sockets size the kernel for them. Not measured on Linux, where the defaults differ.
- **Touches.** [`DEPLOY.md`](DEPLOY.md).

## The machine and the method

- **Hardware.** Apple M4 (10 cores: 4 performance, 6 efficiency), 24 GB, APFS SSD, macOS 27.0.1. Server, node, load generator and browser all on this machine.
- **Shared.** Other builders compiled and tested on the same machine throughout: the 1-minute load average was 5 to 50 (median 16) during the first pass of the server measurements. Every server figure here is therefore pessimistic and noisier than on a quiet machine; where two passes were run, tables give the range across them.
- **Builds.** `cargo build --release --locked`, Rust 1.99.1; the frontend from `bun run build`, served with its precompressed `.br` files.
- **Nodes.** Load ran only against a private rbitcoin v0.9.2 regtest node in Docker ([`REGTEST.md`](REGTEST.md)) with transactions arriving from [`activity.py`](../dev/perf/activity.py). Mainnet was used for the 30-minute footprint, light paced timings (5 requests a second, 51 per route) and page loads, read-only. **The shared mainnet node changed during the measurements:** it ran rbitcoin v0.9.2 until 14:03 UTC, then was restarted on a build of rbitcoin's `master` that fixes the node stalling Electrum and Esplora after a new tip. The paced timings (13:30 and 14:00 UTC), our mainnet page loads (13:33 to 13:41 UTC) and the first footprint run were against v0.9.2; the second footprint run was against v0.9.2 for its first 13 minutes and the new build after.
- **Percentiles** are nearest-rank. Client times are measured by the load generator from sending a request to reading the whole body; server times are the server's own `duration_ms` (until the response head is ready) from its log, read by [`serverlog.py`](../dev/perf/serverlog.py).
- **Limits.** Clients name themselves in `X-Forwarded-For`, trusted from 127.0.0.1 only, so each virtual client is a client of its own to the limiter, as it would be from its own address. The HTTP throughput runs raise `CLIENT_RATE_PER_SEC` and `CLIENT_BURST` to 1,000,000 through settings, so the server, not one client's allowance, is measured; every other limit is at its default, including `MAX_CONCURRENT_REQUESTS` (256). The WebSocket runs raise `WS_MAX_SOCKETS` to 20,000. The limits runs use every default. Defaults in the code are unchanged.
- **When.** Load bursts were at most two minutes, 15 s apart, one at a time: pass 1 from 12:47 to 13:10 UTC, pass 2 from 13:56 to 14:18 UTC (load average 4 to 12), and the A/B and flood experiments between and after them. The mainnet footprint ran twice: 13:20 to 13:47 UTC (stopped by a time limit at 27 minutes) and 13:50 to 14:23 UTC (33 minutes).

## The server under load

### Route classes

Each run is 15 s of closed-loop clients, each on its own connection, cycling through the class's routes:

| Class | Routes |
|---|---|
| memory | `/api/v1/fees/mempool-blocks`, `/api/v1/difficulty-adjustment`, `/api/blocks/tip/height` (held for 1 s), `/app/block/:hash/summary` (the summary is cached in memory, but the route reads the store twice) |
| immutable | `/api/block/:hash` |
| store | `/api/v1/blocks`, `/api/v1/mining/pools/1w`, `/api/v1/statistics/24h`, `/api/v1/block/:hash` |
| node | `/api/tx/:txid`, `/api/block/:hash/txs`, `/api/tx/:txid/outspends`, `/api/address/:address` |

Two passes (12:47 and 13:56 UTC); each cell is the range across them. Client times are over loopback; CPU is the server's mean over the run, in cores.

| Class | Clients | Answered per second | Refused | Client p50 / p99 ms | Server CPU | Server RSS MB |
|---|---|---|---|---|---|---|
| memory | 1 | 11,948–21,604 | 0% | 0.03–0.05 / 0.14–0.54 | 0.6–0.8 | 35–36 |
| memory | 100 | 70,399–83,435 | 0% | 0.74–0.87 / 5.2–6.5 | 4.2–4.8 | 20–42 |
| memory | 1,000 | 29,026–45,126 | 56–58% | 7.9–11.8 / 46–104 | 2.7–4.5 | 40–65 |
| immutable | 1 | 2,101–4,862 | 0% | 0.19–0.30 / 0.51–2.8 | 0.2–0.3 | 14–65 |
| immutable | 100 | 27,093–94,225 | 0% | 1.0–2.2 / 2.8–26 | 1.2–4.2 | 18–44 |
| immutable | 1,000 | 18,459–27,999 | 76–81% | 13–19 / 43–68 | 3.2–3.9 | 41–46 |
| store | 1 | 7,546–11,106 | 0% | 0.08–0.09 / 0.26–0.71 | 0.8–0.9 | 15–47 |
| store | 100 | 22,645–26,446 | 0% | 3.8–4.9 / 10–13 | 4.0–4.4 | 21–50 |
| store | 1,000 | 10,594–11,047 | 89% | 32–36 / 84–92 | 4.1–4.2 | 45–53 |
| node | 1 | 1,419–1,580 | 0% | 0.35–0.39 / 1.9–2.3 | 0.1 | 45–51 |
| node | 100 | 41,851–51,289 | 0% | 1.6–1.7 / 7.1–13 | 2.0–2.3 | 35–49 |
| node | 1,000 | 14,060–24,403 | 78–84% | 14–23 / 50–92 | 2.8–3.6 | 42–51 |

Server time per route, p50 / p99 ms, at 1, 100 and 1,000 clients (answered requests only):

| Route | 1 | 100 | 1,000 |
|---|---|---|---|
| `/api/v1/difficulty-adjustment` | 0.00–0.01 / 0.02–0.04 | 0.01 / 0.40–0.42 | 0.01 / 0.49–0.82 |
| `/api/v1/fees/mempool-blocks` | 0.00 / 0.02–0.04 | 0.01 / 0.39–0.42 | 0.01 / 0.46–0.80 |
| `/api/v1/mining/pools/1w` | 0.01 / 0.01–0.04 | 0.01 / 0.09–0.14 | 0.01 / 0.62–0.74 |
| `/api/v1/statistics/24h` | 0.00 / 0.01–0.04 | 0.01 / 0.08–0.12 | 0.01 / 0.69–0.90 |
| `/api/blocks/tip/height` (held 1 s) | 0.00 / 0.02–0.04 | 0.01 / 0.46 | 0.01 / 1.5–6.5 |
| `/app/block/:hash/summary` | 0.04 / 0.13–0.54 | 2.0–2.2 / 5.8–7.5 | 22–31 / 38–96 |
| `/api/v1/blocks` | 0.15 / 0.27–0.73 | 7.2–8.3 / 11–14 | 46 / 72–87 |
| `/api/v1/block/:hash` | 0.07 / 0.15–0.51 | 7.0–8.0 / 10–13 | 45–46 / 71–85 |
| `/api/block/:hash` (node) | 0.16–0.25 / 0.44–1.8 | 0.53–1.2 / 1.8–16 | 6.8–11 / 31–49 |
| `/api/tx/:txid` (node) | 0.25–0.28 / 0.59–0.87 | 0.79–0.80 / 4.4–8.5 | 7.5–14 / 37–73 |
| `/api/block/:hash/txs` (node) | 0.32–0.35 / 0.65–0.99 | 0.84–0.86 / 4.8–7.0 | 7.5–14 / 39–64 |
| `/api/tx/:txid/outspends` (node) | 0.22–0.25 / 0.46–0.73 | 0.74 / 4.7–7.6 | 7.2–14 / 33–66 |
| `/api/address/:address` (node, cost-limited) | 1.4–1.5 / 2.3–2.9 | 1.7–1.9 / 6.0–10 | 9.9–16 / 41–81 |

The regtest node is on the same machine; on mainnet a node call adds the network to the node and the node's own work ([`RBITCOIN.md`](RBITCOIN.md#measurements)).

At 1,000 clients every class is refused in part: with 1,000 requests in flight and `MAX_CONCURRENT_REQUESTS` at 256, the rest get `503` at once, as designed. The answered throughput then falls below the 100-client figure because the refusals and their log lines take a share of the cores. Address routes are additionally held to four running and eight waiting per network ([`API.md`](API.md#protection-for-the-node)).

### What the limits do

Every limit at its default; two passes, ranges across them.

| Run, 30 s each | Sent per second | Answered per second | Refused (`429`) | Client p50 / p99 ms, answered | Server CPU |
|---|---|---|---|---|---|
| A heavy user: one address, 6 connections, 4 a second each | 24 | 24 | 7–8 | 1.0–1.1 / 10–12 | under 0.01 |
| 100 ordinary clients, 2 a second each, alone | 203 | 203 | 0 | 1.3–1.4 / 8.7–8.8 | under 0.01 |
| The same 100, beside a flood | 203 | 203 | 0 | 1.3–2.0 / 6.2–11 | 3.0–3.8 |
| The flood: one address, 64 connections, no pause | 103,000–121,000 | 28–32 | 3.1–3.6 million | 0.6–0.7 / 2.6–12 | (in the line above) |

- **An honest heavy user**, a script sending 24 requests a second in all, just over `CLIENT_RATE_PER_SEC` (20): 7 or 8 of about 730 requests in 30 s were refused, after the burst of 120 was spent, each with `Retry-After: 1`; every other answer came at full speed. A browser user does not come near this: the heaviest page makes 13 API requests ([`CONFIG.md`](CONFIG.md#per-client-defaults)).
- **A client that hits the limits** is refused in 0.005 ms of server time (the server's own `duration_ms`, p50) and leaves everyone else's answers alone: the 100 ordinary clients beside it were never refused and kept their median within 0.7 ms. It costs us CPU: about 28 µs per refusal, so the flood took 3.4 to 3.8 cores. With `LOG_REQUESTS=off` the same flood took 3.4 cores against 3.8 with the default, so most of that is the HTTP work of refusing and about 0.4 cores is the log, which grew by 911 MB in 30 s ([proposed task 3](#3-count-refusals-instead-of-logging-each)). Across the internet one client rarely reaches loopback rates; a proxy or firewall in front is the place to stop a flood before it costs anything.

### The WebSocket

Every socket subscribes to `blocks`, `mempool`, `projected` and `transactions` (the home page), and one in ten also to `projected-txs:0`. Regtest takes 20 transactions a second and a block every 30 s; each step runs 90 s. Each message is identified by its bytes, which every subscriber shares, so the load generator knows when the first and the last socket read it; for blocks it joins that with the server's `ws_fanout` event, which says when the message was queued.

Two passes of 90 s for 100, 1,000 and 10,000 sockets. The 2,000, 5,000 and 8,000 rows are single 40-second runs made afterwards to find the knee, under the same activity. "Queued" is the server's own time to put a block on every subscriber's queue (`fanout_ms`); the reader times are as the load generator saw them.

| Sockets | Block queued to all, ms | Block, queued → last reader, ms (max) | Mempool update, first → last reader, ms (median / max) | Server CPU, mean / busiest second | Server RSS MB |
|---|---|---|---|---|---|
| 100 | 0.01–0.07 | 1.3 | 0.5 / 2.0–4.5 | 0.003 / 0.02 cores | 17–35 |
| 1,000 | 0.12–1.1 | 7.3–7.5 | 6.2 / 12–15 | 0.02 / 0.10 | 23–35 |
| 2,000 | 1.0 | 11 | 16 / 29 | | |
| 5,000 | 2.4 | 34 | 28 / 49 | | |
| 8,000 (7,929 opened) | 9.3 | 57 | 45 / 69 | 0.13 / 0.65 | 76 |
| 10,000 (9,646 to 10,000 opened) | 2.3–5.8 | 7,000 to 26,000 | 2,000–3,100 / 9,000–26,000 | 0.06–0.15 / 0.6–1.0 | 81–96 |

A `projected-txs:0` delta reached its last reader within 7 ms at 100 sockets and 24 ms at 1,000. Resident memory is the process's high-water mark and carries over from earlier steps on the same server; at 9,962 sockets it was 81 MB against 17 MB at 100, about 6.5 KB a socket. The load generator used under 0.1 core throughout.

- **Where it stops scaling, and why.** Not in the server: at 10,000 sockets it queued a block to every one in 2.3 to 5.8 ms, using under a sixth of one core on average and one core at its busiest second. Up to 8,000 sockets the last reader had a block 57 ms after it was queued, the time growing with the sockets, each of which has its own writer task and its own write. At 10,000 sockets the host ran out of network buffers ([proposed task 5](#5-document-the-host-limits-for-many-sockets)): the same messages reached the last readers 6 to 13 s late, whether one client process held all the sockets or two held 5,000 each. A server with its clients elsewhere holds half as many sockets on its host; this was not measured.
- **A slow client.** 50 sockets that subscribed to the two largest topics and never read, beside 1,000 that did. Nobody else was slowed (the block reached the last of the 1,000 within 12 ms). The 50 were not closed for a full send queue: the kernel's socket buffers took what was sent to them, so their queues never filled. They were closed after 61 s for silence (`4004`, `WS_IDLE_TIMEOUT_SECS`), since a client that reads nothing answers no ping.

### Footprint

| | CPU, share of one core | Resident memory |
|---|---|---|
| Regtest, idle, no clients | 0.0% | 10 MB |
| Regtest, 20 transactions a second, no clients | 0.1% | 13 MB |
| Mainnet, mempool engine off, 3 minutes, two runs | 0.2 to 0.3% | 78 to 122 MB |
| Mainnet, live mempool, 33 minutes, across the node's upgrade (59,000 to 63,000 transactions) | 4.0% mean; 0.2 to 6.2% by the minute | 107 to 175 MB; 145 to 175 MB in the last 10 minutes |
| Mainnet, live mempool, the 27-minute run, rbitcoin v0.9.2 (60,000 to 66,000 transactions) | 4.2% mean; 3.6 to 5.5% by the minute | 105 to 186 MB |

On mainnet the engine read 103 to 230 MB a minute from the node (25 to 54 tracker cycles a minute), more at times than the 150 MB [`CONFIG.md`](CONFIG.md#the-live-mempool) states for a 54,000-transaction mempool; the initial sync read 258 to 263 MB in 11 to 15 s for 61,000 to 64,000 transactions. Memory did not grow over the 33 minutes beyond the mempool's own size. The node's upgrade restarted it during the second run (14:03 to 14:04 UTC); the server reported it unreachable, recovered on its own and went on, so the footprint covers both node versions.

### The new block on mainnet

The server's `ws_fanout` event times a block from the node's Electrum notification to the last subscriber's queue. After the node's upgrade two blocks arrived, at 12 and 91 ms. Before it, on rbitcoin v0.9.2 (five blocks over the two runs), 13 ms when the node's Esplora served the block at once, and 0.40, 1.10 and 1.61 s, and once over 3 s, when it did not (the blocks already there when the server started are not counted): the hub asks again every 25 ms while the node answers 404, and the fetch itself costs about one round trip to the node (9 to 10 ms from here). We could not record how many retries each block took, so the split between the node's wait and ours is inferred from that round trip, not measured. The v0.9.2 waits of up to 1.6 s, and the one over 3 s, are longer than the 320 to 470 ms [`RBITCOIN.md`](RBITCOIN.md#rules-that-follow) records for signet; that is the stall the upgrade fixes. Two blocks are too few to say how the upgraded node behaves; the 91 ms block probably waited on it for a few retries.

## The site in a browser

Each page is loaded cold (a fresh browser context) in headless Chromium 156 at 1280×800, five times on regtest and three on mainnet, by [`pages.mjs`](../dev/perf/pages.mjs). "Data shown" is when the page's main content shows its data (the stats and projected blocks on the home page, a known value on the others); "JS run" is Chromium's `ScriptDuration`. The slow-phone profile is Lighthouse's Slow 4G as DevTools applies it (562.5 ms per request, 1.47 Mbps down, 675 kbps up) with the CPU slowed 4×. WebGL runs on the machine's GPU through ANGLE's Metal backend; without flags headless Chromium draws it in software.

**Regtest, unthrottled** (five loads each; median, with the range):

| Page | First contentful paint ms | Data shown ms | Largest contentful paint ms | KB transferred | of it JavaScript | Requests | JS run ms |
|---|---|---|---|---|---|---|---|
| home | 40 (32–64) | 63 (48–64) | 60 (56–76) | 132 | 67 | 26 | 10 (9–13) |
| block | 36 (32–40) | 59 (49–68) | 68 (56–84) | 134 | 72 | 31 | 12 (10–13) |
| tx | 40 (32–40) | 47 (42–85) | 56 (40–84) | 126 | 70 | 22 | 9 (8–9) |
| address | 40 (32–44) | 63 (57–64) | 76 (68–80) | 150 | 62 | 27 | 11 (8–13) |
| mining | 32 (32–36) | 46 (43–49) | 60 (52–64) | 140 | 71 | 32 | 7 (7–8) |
| projected | 36 (32–44) | 42 (39–46) | 52 (44–64) | 121 | 70 | 26 | 3 (3–4) |
| mempool | 36 (36–60) | 44 (42–50) | 52 (48–72) | 115 | 61 | 20 | 3 (3–4) |

**Regtest, as a slow phone:**

| Page | First contentful paint ms | Data shown ms | Largest contentful paint ms | KB transferred | of it JavaScript | Requests | JS run ms |
|---|---|---|---|---|---|---|---|
| home | 1,736 (1,728–1,740) | 4,145 (4,141–4,165) | 4,088 (4,072–4,108) | 132 | 67 | 26 | 53 (48–61) |
| block | 1,736 (1,724–1,760) | 4,148 (4,141–4,204) | 4,168 (4,144–4,224) | 134 | 72 | 31 | 58 (49–75) |
| tx | 1,724 (1,716–1,740) | 3,631 (3,611–3,642) | 2,996 (2,976–3,008) | 126 | 70 | 22 | 44 (38–52) |
| address | 1,732 (1,712–1,736) | 4,079 (4,067–4,094) | 4,084 (4,072–4,100) | 112 | 62 | 27 | 45 (42–51) |
| mining | 1,724 (1,724–1,728) | 4,062 (4,061–4,082) | 3,500 (3,496–3,508) | 140 | 71 | 32 | 36 (33–43) |
| projected | 1,732 (1,724–1,744) | 4,128 (4,121–4,129) | 3,492 (3,476–3,512) | 121 | 70 | 26 | 24 (23–25) |
| mempool | 1,724 (1,708–1,732) | 3,553 (3,540–3,571) | 3,560 (3,552–3,588) | 115 | 61 | 20 | 37 (35–38) |

**Mainnet, unthrottled** (our server on the mainnet node, three loads each; the block is 840,000 with 3,050 transactions, the next block about 3,400):

| Page | First contentful paint ms | Data shown ms | Largest contentful paint ms | KB transferred | of it JavaScript | Requests | JS run ms |
|---|---|---|---|---|---|---|---|
| home | 36 (32–60) | 60 (50–67) | 60 (52–68) | 131 | 67 | 27 | 13 (11–16) |
| block | 48 (32–64) | 48 (47–60) | 216 (156–296) | 231 | 72 | 31 | 23 (18–36) |
| tx | 36 (32–36) | 65 (62–150) | 60 (36–64) | 124 | 70 | 22 | 8 (7–8) |
| address | 36 (32–36) | 59 (43–66) | 76 (52–76) | 118 | 62 | 27 | 5 (5–6) |
| mining | 32 (32–36) | 45 (43–50) | 64 (60–64) | 146 | 71 | 33 | 8 (8–11) |
| projected | 36 (32–36) | 121 (108–159) | 88 (80–96) | 121 | 70 | 26 | 94 (86–134) |
| mempool | 40 (36–56) | 42 (30–43) | 52 (40–68) | 114 | 61 | 20 | 3 (3–4) |

**Mainnet, as a slow phone:**

| Page | First contentful paint ms | Data shown ms | Largest contentful paint ms | KB transferred | of it JavaScript | Requests | JS run ms |
|---|---|---|---|---|---|---|---|
| home | 1,752 (1,716–1,764) | 4,191 (4,145–4,196) | 3,528 (3,500–3,536) | 131 | 67 | 27 | 107 (95–119) |
| block | 1,732 (1,728–1,748) | 3,505 (3,505–3,517) | 4,672 (4,648–4,708) | 231 | 72 | 31 | 102 (90–118) |
| tx | 1,720 (1,712–1,724) | 3,603 (3,603–3,611) | 2,988 (2,976–2,996) | 124 | 70 | 22 | 39 (37–43) |
| address | 1,728 (1,724–1,728) | 4,064 (4,043–4,065) | 4,068 (4,044–4,068) | 120 | 62 | 27 | 30 (25–32) |
| mining | 1,724 (1,720–1,736) | 4,061 (4,051–4,081) | 4,064 (4,056–4,084) | 146 | 71 | 33 | 45 (45–47) |
| projected | 1,724 (1,724–1,732) | 6,234 (6,135–6,245) | 3,476 (3,476–3,484) | 121 | 70 | 26 | 434 (293–602) |
| mempool | 1,716 (1,712–1,736) | 3,548 (3,524–3,564) | 3,556 (3,532–3,576) | 114 | 61 | 20 | 39 (36–42) |

Every page shows its data within 2.5 s of its load event as a slow phone, and the chain of requests is why ([proposed task 4](#4-shorten-the-pages-request-chain-on-slow-networks)). The mainnet next-block page is the slowest: its 166 to 378 KB snapshot of about 3,400 transactions arrives over the socket and is decoded and listed, 94 ms of JavaScript unthrottled and 434 ms at 4×.

**Frame times.** `requestAnimationFrame` intervals for 6 s, three times each: from mining a block while the home page is open (the new block slides in, the projected blocks and stats update), and from an in-app navigation to a block of 601 transactions until its grid has drawn.

| | Through a new block on the home page | Through a block's grid loading (601 transactions) |
|---|---|---|
| Unthrottled | 60.7 fps / max 16.8 ms / 0, 60.7 fps / max 16.8 ms / 0, 61 fps / max 16.8 ms / 0 | 60 fps / max 16.8 ms / 0, 59.8 fps / max 16.8 ms / 0, 59.8 fps / max 16.8 ms / 0 |
| CPU 4× slower | 60.8 fps / max 16.8 ms / 0, 60.7 fps / max 16.8 ms / 0, 60.8 fps / max 16.8 ms / 0 | 59 fps / max 83.3 ms / 2, 59.3 fps / max 50.1 ms / 1, 57.3 fps / max 250 ms / 1 |

Each cell is three runs: frames a second, the longest frame, and the frames of 33 ms or more. In every run the new block or the grid was on the page at the end (checked). One 4× grid run had one long task of 224 ms; four further profiled runs had none.

`requestAnimationFrame` in headless Chromium ticks at 60 Hz, so a dropped frame shows as an interval of 33 ms or more. The M4 is faster than a mid-range laptop; the 4× slowdown stands for something slower than one. The largest JavaScript costs of a grid navigation at 4× were reading computed styles (22 to 30 ms) and focusing the heading (14 ms).

## Side by side

### The stock mempool backend

[`COMPAT.md`](COMPAT.md) calls it the reference backend: a stock mempool backend (v3.3.2, Esplora mode) in front of the same mainnet node. It was not run on this machine: its images and a database for it are not here, and its first start would sync the whole mainnet mempool from the shared node. The instance the compatibility work used, already running beside the node, was asked the same routes in the same way as ours instead: one client, 5 requests a second, 51 per route, at 13:30 and 13:31 UTC, with the node on v0.9.2. Ours ran on this machine against the node over the network, with the mempool engine on.

| Route | Ours, p50 / p99 ms | Reference, p50 / p99 ms |
|---|---|---|
| `/api/v1/fees/recommended` | 0.19 / 0.63 | 10.44 / 801.49 |
| `/api/v1/fees/mempool-blocks` | 0.21 / 1.25 | 10.84 / 642.23 |
| `/api/v1/difficulty-adjustment` | 0.21 / 0.51 | 13.59 / 686.36 |
| `/api/v1/blocks` | 0.65 / 27.5 | 13.05 / 448.35 |
| `/api/v1/block/:hash` | 0.4 / 1.18 | 11.02 / 819.5 |
| `/api/v1/statistics/2h` | 0.23 / 0.88 | 16.98 / 514.68 |
| `/api/blocks/tip/height` | 0.21 / 20.11 | 6.97 / 100.37 |
| `/api/block/:hash` | 9.16 / 75.07 | 6.32 / 141.35 |

All 816 requests, 51 per route on each side, were answered `200`. Recorded earlier on the same pair, the reference's mining routes took 14 to 54 s each where ours answer from memory in under 2 ms, and two unrecognised mining paths hung until its proxy's 60 s timeout ([`API.md`](API.md#mining), [`COMPAT.md`](COMPAT.md#reference-backend-against-rbitcoins-esplora)); those figures are older and were not taken again.

The reference is a network hop away (median TCP connect 9.2 ms) and ours is on loopback (0.2 ms), so subtract about one connect time from the reference's median to compare server time; its p99s of 450 to 820 ms are its own. `/api/block/:hash` on its host is answered by the node's Esplora directly, not by the backend. With 51 requests a route, p99 is the slowest request.

### mempool.space

Its public site, the same seven pages with the same data (block 840,000, a transaction in it and its address), the same browser and profiles, two cold loads each: 28 page loads between 13:42 and 13:45 UTC, and no API requests. Its JavaScript differs, so "data shown" is not measured there; compare first contentful paint, largest contentful paint, bytes and JavaScript run.

**Unthrottled:**

| Page | First contentful paint ms | Largest contentful paint ms | KB transferred | of it JavaScript | Requests | JS run ms |
|---|---|---|---|---|---|---|
| home | 580 (580–1,120) | 580 (580–1,120) | 1,281 | 1,172 | 25 (25–39) | 120 (120–213) |
| block | 596 (596–620) | 596 (596–620) | 838 | 714 | 45 | 98 (98–135) |
| tx | 952 (952–972) | 952 (952–972) | 1,335 | 1,220 | 47 | 99 (99–106) |
| address | 656 (656–880) | 756 (756–900) | 1,351 | 1,144 | 29 | 155 (155–165) |
| mining | 752 (752–808) | 752 (752–808) | 1,265 | 1,144 | 44 | 131 (131–132) |
| projected | 512 (512–828) | 1,128 (1,128–1,152) | 1,245 | 1,144 | 38 | 204 (204–214) |
| mempool | 616 (616–980) | 616 (616–980) | 1,195 | 1,144 | 24 | 107 (107–114) |

**As a slow phone:**

| Page | First contentful paint ms | Largest contentful paint ms | KB transferred | of it JavaScript | Requests | JS run ms |
|---|---|---|---|---|---|---|
| home | 8,588 (8,588–8,616) | 8,588 (8,588–8,616) | 1,507 | 1,398 | 39 | 545 (545–549) |
| block | 6,288 (6,288–6,300) | 6,288 (6,288–6,300) | 927 | 805 | 45 | 430 (430–437) |
| tx | 9,012 (9,012–9,108) | 9,012 (9,012–9,108) | 1,512 | 1,398 | 47 | 456 (456–474) |
| address | 8,532 (8,532–8,556) | 9,688 (9,688–9,700) | 1,445 | 1,398 | 29 | 311 (311–321) |
| mining | 8,568 (8,568–8,572) | 8,568 (8,568–8,572) | 1,518 | 1,398 | 44 | 584 (584–651) |
| projected | 8,620 (8,620–8,676) | 9,452 (9,452–9,628) | 1,498 | 1,398 | 38 | 527 (527–531) |
| mempool | 8,540 (8,540–8,548) | 8,540 (8,540–8,548) | 1,448 | 1,398 | 24 | 462 (462–466) |

With two loads the median shown is the lower of the two.

How far the comparison can be trusted:

- **mempool.space is a fleet behind a CDN; we are one process on loopback.** Unthrottled, its times include the internet round trips from this machine and ours include none, which flatters us. As a slow phone both pay the same 562.5 ms a request on top, and the difference is dominated by the bytes: 1.4 MB of JavaScript at 1.47 Mbps takes about 8 s, our 70 KB under half a second. That part holds whatever the hosting.
- **Its CDN holds every file close and serves HTTP/2 or 3;** we served HTTP/1.1 with no proxy, which costs us the six-connection queue in [proposed task 4](#4-shorten-the-pages-request-chain-on-slow-networks). A deployment of ours behind a TLS proxy would load faster than measured here.
- **Under load the comparison runs the other way.** The fleet absorbs traffic one process cannot; [The server under load](#the-server-under-load) is all we can say about ours, and nothing here says how many clients mempool.space serves.
- Two loads per page is a small sample; the spread between them is in the table.

## Regression guard

`bun run size` already fails a PR whose first-load JavaScript passes 100 KB, at no cost. A latency check in the PR path would fail on noise: GitHub's shared runners, like this machine, vary by tens of percent from run to run, and the [rules](CI.md#rules-for-tests) treat a flaky check as a failing one. Proposed instead, not added: run `dev/perf/bench.sh OUT http` weekly or by hand in a scheduled workflow outside the PR path (about 8 minutes of a runner, nothing added to a PR's 3 minutes) and post the summary, so a regression shows as a trend.

## What was not measured

- A quiet machine: every server number carries the noise of other builds.
- Linux, a server whose clients are on other machines, and TLS or HTTP/2 through a proxy.
- The reference backend under load, or on this machine: see above.
- A mid-range laptop's GPU, or a real phone; the browser profiles stand in for them.
- The `/api/v1/ws` socket, the Lightning pages, and Firefox or Safari.
- Signet and testnet4.

## How to reproduce

From the repository root, after `cargo build --release --locked -p mempool-based -p load` and `(cd web && bun install && bun run build)`:

| Measures | Command |
|---|---|
| The server under load, on a private regtest node (about 22 minutes) | `dev/perf/bench.sh OUT [idle] [http] [limits] [ws]`, then `dev/perf/summary.py OUT [OUT2 …]` |
| Page loads and frame times on regtest | `dev/regtest/regtest.py scenario > manifest.json`, `dev/perf/server.sh DIR`, then `cd web && node ../dev/perf/pages.mjs regtest http://ADDR --manifest ../manifest.json --frames --grid-block HASH`, then `dev/perf/summary.py --pages FILE` |
| Mainnet footprint and paced timings (reads `.env`) | `dev/perf/mainnet.sh OUT MINUTES [REFERENCE_URL]` |
| Another site's pages | `cd web && node ../dev/perf/pages.mjs reference https://SITE --runs 2 --tx TXID --address ADDRESS` |

`bench.sh` and `server.sh` use the regtest node of `REGTEST_PROJECT` and remove it at the end; set `REGTEST_IMAGE` where the pinned image cannot be pulled ([`REGTEST.md`](REGTEST.md)). The load generator is `dev/load` (`cargo run --release -p load -- http|ws …`); its options are in its header.
