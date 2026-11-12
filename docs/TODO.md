# TODO

The task list. The orchestrator owns this file; builders do not edit it. Milestones are described in [`PLAN.md`](PLAN.md).

Status is one of: `todo`, `doing`, `done` (with the PR), `skipped` (with the reason).

## M0 — Foundation

| ID | Task | Needs | Status |
|---|---|---|---|
| F0 | Rust workspace, skeleton server (`/healthz`, static files), skeleton frontend, CI within budget | — | done ([#1](https://github.com/smolcars/rbitcoin-space/pull/1)) |
| R1 | Parity inventory: fill [`COMPAT.md`](COMPAT.md) from mempool.space's observable API and rbitcoin's surface | — | done ([#6](https://github.com/smolcars/rbitcoin-space/pull/6)) |
| R2 | Regtest harness: rbitcoin v0.9.1 in Docker, scripts to mine and transact, `docs/REGTEST.md` | — | done ([#2](https://github.com/smolcars/rbitcoin-space/pull/2)) |
| D1 | Design language and static prototype; rewrite [`DESIGN.md`](DESIGN.md) | — | done ([#5](https://github.com/smolcars/rbitcoin-space/pull/5)) |

## M1 — Explorer skeleton

| ID | Task | Needs | Status |
|---|---|---|---|
| N1 | Esplora client: TCP and unix socket, typed models, chain-tip stamp handling | F0 | done ([#3](https://github.com/smolcars/rbitcoin-space/pull/3)) |
| N2 | Electrum client: requests, subscriptions (headers, scripthash, outpoint), reconnect | F0 | done ([#8](https://github.com/smolcars/rbitcoin-space/pull/8)) |
| N3 | RPC client: fixed set of typed read-only methods | F0 | done ([#7](https://github.com/smolcars/rbitcoin-space/pull/7)) |
| S1 | Server core: multi-network config, limits and rate-limit middleware, error model, security headers | F0 | done ([#4](https://github.com/smolcars/rbitcoin-space/pull/4)) |
| S2 | Esplora pass-through at `/api`: route allow-list, typed parameters, cache headers | N1, S1 | done ([#9](https://github.com/smolcars/rbitcoin-space/pull/9)) |
| S3 | WebSocket hub: bounded connections and topics, new-block push from Electrum; `docs/WS.md` | N2, S1 | done ([#22](https://github.com/smolcars/rbitcoin-space/pull/22)) |
| W1 | App shell: router with network prefixes, layout, theme and tokens, shared components, HTTP data layer | F0, D1 | done ([#11](https://github.com/smolcars/rbitcoin-space/pull/11)) |
| W2 | Home page: latest blocks, WebSocket client and live block push, mempool summary | W1, S2, S3 | done ([#26](https://github.com/smolcars/rbitcoin-space/pull/26)) |
| W3 | Block page: header details, paged transaction list | W1, S2 | done ([#23](https://github.com/smolcars/rbitcoin-space/pull/23)) |
| W4 | Transaction page: inputs, outputs, fee, status, live confirmation | W1, S2, S3 | done ([#24](https://github.com/smolcars/rbitcoin-space/pull/24)) |
| W5 | Address page: totals, unspent outputs, paged history; large addresses show history without totals and say why | W1, S2 | done ([#25](https://github.com/smolcars/rbitcoin-space/pull/25)) |
| W6 | Search: txid, block hash, height, address (exact match) | W1, S2, LX1 | done ([#32](https://github.com/smolcars/rbitcoin-space/pull/32)) |
| D2 | Bring `docs/DESIGN.md` in line with what was built: the page builders' corrections (address page sketch, transactions listed in block order, the Live indicator's place, arrows the font has, duration tokens the build rewrites) | LX1 | todo |
| FE1 | Frontend follow-ups from the page tasks: let the API client expose the chain-tip header so a page can tell when the tip moved between its requests; stop the footer jumping when a page first mounts | LX1 | done ([#33](https://github.com/smolcars/rbitcoin-space/pull/33)) |
| E1 | End-to-end harness: headless browser against the server on regtest, inside the CI budget; fix the image build broken by the app shell and guard against a repeat | F0, R2, S1, W1 | done ([#15](https://github.com/smolcars/rbitcoin-space/pull/15), [#21](https://github.com/smolcars/rbitcoin-space/pull/21)) |
| P1 | Docker image and compose file beside rbitcoin | S1 | done ([#10](https://github.com/smolcars/rbitcoin-space/pull/10)) |
| A1 | Scheduled dependency audit against the advisory database, outside the PR path | F0 | done ([#60](https://github.com/smolcars/rbitcoin-space/pull/60)); the first run found no advisories |
| CI2 | Get the browser-test job back to 90 s or less: the recorded-minute test waits up to two minutes for a real sample; seed the store instead | FE9 | done ([#58](https://github.com/smolcars/rbitcoin-space/pull/58)); the browser-test job is back to 77 s |
| QA1 | Acceptance check of milestone M1 against its exit criteria: the built server and the Docker image on all three networks in a real browser, numbers cross-checked with the node, hostile input, the performance budgets; findings become tasks | W6 | done: exit criteria met on all three networks; findings became FX3, FE4 and FX5 |
| FX3 | Server fixes from the M1 acceptance check: the per-client limits must not break an ordinary page load (assets were rate-limited); the proxy header must not be trusted by default or from an untrusted peer; a cost limit on address routes; same-origin WebSocket upgrades on `/app/ws`; a wrong RPC password reported as such; the acceptance check's hostile-request scripts turned into tests | QA1 | done ([#39](https://github.com/smolcars/rbitcoin-space/pull/39)) |
| FE4 | Frontend fixes from the M1 acceptance check: retry a failed chunk load; the home page's duplicate requests; overlapping fee labels; the block page's transaction table on phones; the genesis coinbase shown as spendable; amounts without units for screen readers; consistent size units; the smaller wording issues; a browser test with hostile chain data | QA1, W7 | done ([#43](https://github.com/smolcars/rbitcoin-space/pull/43)) |
| FX5 | Show a new block the moment Electrum announces it, from its header, and fill in the rest when the node serves it, so the node's hold of up to 0.4 s is not felt | S3, W2 | done ([#67](https://github.com/smolcars/rbitcoin-space/pull/67)); the node fix made the two-step arrival unnecessary, so it ships the late-block handling only |
| RB1 | Use rbitcoin's new `GET /block/:hash/txs/summary` (upstream PR 1578, after v0.9.2) for block summaries, audits and the fee-rate backfill, with `getblock` as the fallback on v0.9.2 | MI1 | done ([#69](https://github.com/smolcars/rbitcoin-space/pull/69)); raise the pin and delete the `getblock` fallback once an rbitcoin release has the route |
| RN1 | Rename the project to rbitcoin.space (repository `smolcars/rbitcoin-space`): binary, packages, images, docs and the site's name, as one mechanical change | - | done ([#64](https://github.com/smolcars/rbitcoin-space/pull/64)) |
| FX7 | The copy button copies nothing on plain HTTP (the clipboard API needs a secure context) and still says "Copied": add the fallback and tell the truth. Found by the owner on a phone | W1 | done ([#65](https://github.com/smolcars/rbitcoin-space/pull/65)) |
| FX8 | A regression test for the held fee answer being released at its deadline: the reviewer of #67 noted it has none | FX5 | done ([#76](https://github.com/smolcars/rbitcoin-space/pull/76)); writing the test found a real bug: the release timer kept firing after its deadline |
| U1 | Move the regtest harness and compose files to the rbitcoin v0.9.2 image; confirm the scenario's hashes and the node clients are unchanged | R2, P1 | done ([#14](https://github.com/smolcars/rbitcoin-space/pull/14)) |
| LX1 | Switch the frontend to OXC: `oxlint` with strict rules and `oxfmt` as the only formatter, Biome removed, TypeScript options tightened, all enforced in CI. Runs when the page tasks in flight (W2–W5) have merged, before any other frontend task, because it reformats every file | W2, W3, W4, W5 | done ([#29](https://github.com/smolcars/rbitcoin-space/pull/29)) |
| SJ1 | Move the frontend to Solid 2.0 (a pre-release; the owner's decision): plan from the official migration guide, migrate every component, keep behaviour and appearance identical, prove it with the full test suites and before-and-after screenshots. The frontend work in flight (FE6, MI6) lands first or is ported on rebase; no other frontend task starts until this merges | LX1 | done ([#53](https://github.com/smolcars/rbitcoin-space/pull/53)); follow-ups: decide whether pages adopt 2.0's async loading, and make 2.0's development warnings fail the tests |
| FX2 | Harden the WebSocket hub: retry the new-block fetch quickly so a slow Esplora cannot turn a block into a resync; run a WebSocket conformance suite and a no-panic test against our hand-written framing; tag the socket-cap refusal in the request log | S3 | done ([#31](https://github.com/smolcars/rbitcoin-space/pull/31)) |
| L1 | Structured logging: a `logging` crate of typed events, one event per API request, no stray prints; `docs/LOGGING.md` | S1 | done ([#12](https://github.com/smolcars/rbitcoin-space/pull/12)) |

## M2 — Live mempool

Refined when M1 lands.

| ID | Task | Needs | Status |
|---|---|---|---|
| MP1 | Mempool tracker: sequence-aware poll and diff, bulk detail fetch, removal reasons | N1, N3 | done ([#20](https://github.com/smolcars/rbitcoin-space/pull/20)) |
| MP2 | Projected blocks from cluster chunks and the block template | MP1 | done ([#28](https://github.com/smolcars/rbitcoin-space/pull/28)) |
| MP3 | Run the mempool tracker and projected blocks inside the server, and serve them over the WebSocket: mempool totals, projected-block totals, binary per-transaction deltas, the latest transactions | MP2, S3 | done ([#35](https://github.com/smolcars/rbitcoin-space/pull/35)) |
| MP4 | Fee recommendations, and the matching `/api/v1/fees` endpoints | MP2 | done ([#45](https://github.com/smolcars/rbitcoin-space/pull/45)); its evidence is one hour and four blocks (MP8) |
| MP5 | Transaction and address tracking over shared, capped Electrum subscriptions | S3, N2 | done ([#42](https://github.com/smolcars/rbitcoin-space/pull/42)) |
| MP6 | RBF history and CPFP or cluster view | MP1 | done ([#42](https://github.com/smolcars/rbitcoin-space/pull/42)) |
| BS1 | Block summary route in `/app`: every transaction's fee and size from `getblock` verbosity 2, cached, plus the block's median fee rate and fee range | N3, S2 | done ([#27](https://github.com/smolcars/rbitcoin-space/pull/27)) |
| FE3 | Show block summaries and the difficulty adjustment in the frontend: median fee rate and fee range on the block page, fee layers and fee figures in the home page's tiles (batch route on load, the WebSocket's `summary` message for new blocks, retry on 503) | BS1, LX1 | done ([#33](https://github.com/smolcars/rbitcoin-space/pull/33)) |
| W7 | WebGL block renderer for projected and mined blocks; fills the grid slot the block page reserves | W1, MP3, BS1 | done ([#36](https://github.com/smolcars/rbitcoin-space/pull/36)) |
| FE5 | The live mempool on the home page: projected-block tiles with their fee layers, the mempool panel and the latest transactions from the WebSocket topics; the projected tile becomes the mined one when a block arrives | MP3, FE3 | done ([#41](https://github.com/smolcars/rbitcoin-space/pull/41)) |
| FE6 | The transaction and address pages, live: a transaction's place in the projected blocks, what replaced it, the group it will be mined with, and its confirmation, from the tracking topics instead of polling; an address page that refreshes when its history changes | MP5, FE4 | done ([#51](https://github.com/smolcars/rbitcoin-space/pull/51)) |
| FE7 | Home page follow-ups: the fee panel from our own recommendations (MP4); a tile's size in the agreed convention (serialized size in B, kB, MB); tighten the new-block workarounds once the server's order is promised (MP7) | MP4, MP7, FE5 | done ([#56](https://github.com/smolcars/rbitcoin-space/pull/56)) |
| FE8 | Show a block's audit on the block page and its score in block lists: what we expected against what was mined, by category, from `/app/block/:hash/audit` and the `audit` field | MI4, SJ1 | done ([#55](https://github.com/smolcars/rbitcoin-space/pull/55)); it also fixed the address history not loading more when a payment arrives mid-click |
| FE9 | Mempool history chart from `/app/mempool/history/:interval`: size by fee band over time with the flow in and out, gaps drawn as gaps | MI5, SJ1 | done ([#57](https://github.com/smolcars/rbitcoin-space/pull/57)); its recorded-minute browser test added about 45 s to CI (CI2) |
| FE10 | Size units in the cell everywhere: the projected and block pages still show a transaction's virtual size as a bare number under "Size, vB" | FE7 | done ([#75](https://github.com/smolcars/rbitcoin-space/pull/75)) |
| W8 | Projected-block page: one projected block's transactions drawn with the grid and updated live | W7, MP3 | done ([#40](https://github.com/smolcars/rbitcoin-space/pull/40)) |
| MP7 | When a block is found, update the projected blocks at once from that block's transactions; today they follow 3 to 6 s later, at the tracker's next poll, so pages see the new tip before the next block empties | MP3 | done ([#49](https://github.com/smolcars/rbitcoin-space/pull/49)) |
| MI6 | Give each block its miner where pages show it: the pool in the block facts, in the WebSocket's `summary` message and in a recent-blocks answer, then on the block page, the home tiles and the mining page's recent blocks | MI2, FE3 | done ([#47](https://github.com/smolcars/rbitcoin-space/pull/47)) |
| FX6 | Make the browser tests reliable under the real limits: the whole suite runs as one client, and the fixed per-client allowance for page loads refuses it about one local run in four. Each test worker should look like its own client, so the suite runs at the production defaults | E1, FX3 | done ([#46](https://github.com/smolcars/rbitcoin-space/pull/46)) |
| MP8 | A day-long evidence run for the fee recommendations on mainnet, scored with `fee_watch`, and tuning from it: the first run saw only four blocks, and the next-block level met its horizon in three of them. The owner agreed on 2026-11-12 | MP4 | doing: the 24-hour recording started on 2026-11-12 |
| FL1 | Find the cause of two intermittent test failures builders saw once each and could not reproduce (a browser test unable to open the WebSocket; one unidentified Rust test failure) | E1 | done by FX6 ([#46](https://github.com/smolcars/rbitcoin-space/pull/46)): the suite ran as one client, a mining test started before the server had caught up, a hover test aimed at moving squares, and one locator matched two elements |
| DA1 | Difficulty adjustment (two headers and arithmetic), for the home page and `/api/v1/difficulty-adjustment` | N3, S2 | done ([#30](https://github.com/smolcars/rbitcoin-space/pull/30)) |
| WS1 | mempool.space-shaped WebSocket subset at `/api/v1/ws` (`want`, `track-tx`, `track-address`, `ping`) from the same hub | S3, MP5 | done ([#54](https://github.com/smolcars/rbitcoin-space/pull/54)); follow-ups in WS2 |

## M3 — Mining and history

Refined when M2 lands.

| ID | Task | Needs | Status |
|---|---|---|---|
| ST1 | SQLite store: migrations, single writer task, read-only readers | F0 | done ([#13](https://github.com/smolcars/rbitcoin-space/pull/13)) |
| MI1 | Block summaries with backfill: totals from `getblockstats`; our own fee-rate percentiles from per-transaction data, newest first in the background | ST1, N1, N3 | done ([#34](https://github.com/smolcars/rbitcoin-space/pull/34)); the full mainnet backfill started on 2026-11-12 on the owner's instance |
| MI2 | Mining pool attribution from coinbase tags and addresses; needs a pool list under an MIT-compatible licence, or our own | MI1 | done ([#38](https://github.com/smolcars/rbitcoin-space/pull/38)) |
| MI3 | Hashrate history | MI1 | done ([#38](https://github.com/smolcars/rbitcoin-space/pull/38)) |
| MI4 | Block audit: expected template against the mined block | MP2, MI1 | done ([#52](https://github.com/smolcars/rbitcoin-space/pull/52)); per-pool averages are still empty |
| MI5 | Mempool history series, one sample a minute | ST1, MP1 | done ([#50](https://github.com/smolcars/rbitcoin-space/pull/50)) |
| PR1 | Fiat prices with an off switch | ST1 | todo |
| W9 | Mining dashboard and pool pages | MI2, MI3 | done ([#44](https://github.com/smolcars/rbitcoin-space/pull/44)) |
| W10 | Charts with uPlot | MI5 | done ([#44](https://github.com/smolcars/rbitcoin-space/pull/44)): hand-written SVG charts, no library |

## M4 — Lightning

Refined when M3 lands. Design in [`LIGHTNING.md`](LIGHTNING.md).

| ID | Task | Needs | Status |
|---|---|---|---|
| LN1 | Gossip peer with LDK: peer connections, graph sync, graph persistence | F0 | done ([#37](https://github.com/smolcars/rbitcoin-space/pull/37)); the graph is unverified until LN2 |
| LN2 | Funding-output lookups through rbitcoin's position routes: batched, rate-limited | LN1, N1 | done ([#48](https://github.com/smolcars/rbitcoin-space/pull/48)) |
| LN3 | Channel opens and closes from blocks, with close classification | LN2 | done ([#61](https://github.com/smolcars/rbitcoin-space/pull/61)) |
| LN4a | Run the Lightning peer inside the server: settings, lifecycle, logging, the stored channel history and statistics snapshots, and a first read surface | LN3, ST1 | done ([#68](https://github.com/smolcars/rbitcoin-space/pull/68)) |
| LN4b | The Lightning node, channel, search and ranking routes, in `/app` and in the `/api/v1/lightning` shapes | LN4a | done ([#72](https://github.com/smolcars/rbitcoin-space/pull/72)) |
| W11 | Lightning pages: network statistics, node, channel, search | LN4b, W1 | done ([#73](https://github.com/smolcars/rbitcoin-space/pull/73)); no channel closed during its mainnet run, so close kinds were checked against the documented shape only |

## M5 — Parity and polish

Refined when M4 lands.

| ID | Task | Needs | Status |
|---|---|---|---|
| C1a | The `/api/v1` block routes still planned: `blocks`, `block/:hash`, the block summary routes and the tip-height alias, with their extras from the store | MI1, MI4 | done ([#59](https://github.com/smolcars/rbitcoin-space/pull/59)) |
| C1b | The `/api/v1` transaction routes still planned: replacements, CPFP, first-seen times, address validation and prevouts. Also correct the comment in `crates/server/src/limits.rs` that says the heaviest page loads 22 files (it is 25 since FE8) | MP1, MP5 | done ([#62](https://github.com/smolcars/rbitcoin-space/pull/62)); follow-ups in C1c |
| C1c | Replacement trees: keep the figures of a root that a miner confirmed after we saw it replaced, and unmark a replacement whose block is reorged away | C1b | done ([#76](https://github.com/smolcars/rbitcoin-space/pull/76)) for the confirmed root; the reorg half is C1d |
| C1d | Unmark a replacement whose confirming block is reorged away, correctly: it needs the mined height in the record and the store (a migration), a reorg path into the record, and a check of the new chain, since in the usual reorg the replacement is in the competing block too | C1c | todo |
| WS2 | Follow-ups on the compatible WebSocket: send the first `mempool-blocks` with the block instead of about 4 s later, and decide on the bounded `track-mempool-txids` its PR proposes | WS1 | todo; its first half, `mempool-blocks` arriving with the block, was done by [#67](https://github.com/smolcars/rbitcoin-space/pull/67); what remains is the decision on a bounded `track-mempool-txids` |
| C1 | Audit the four compatibility inventories against what is built (many rows still say planned for pages and routes that are live), recount, and list what is really left with a recommendation for each | M4 | done ([#78](https://github.com/smolcars/rbitcoin-space/pull/78)): 139 of 232 rows served, 29 planned, 12 different, 52 skipped; the gaps are G1 to G8 and the owner's decisions |
| G1 | Small server gaps the audit found: `vBytesPerSecond` and `live-2h-chart` on `/api/v1/ws`, `/api/v1/backend-info`, three Lightning paths that answer 400 where 404 is right, a pool's average block health, and the Lightning stop that timed out | C1 | doing |
| G2 | Pool column on the blocks page; the past adjustments table and a block health graph on the mining page | C1 | todo |
| G3 | Lightning node page: the fee histogram and the node's history chart | W11 | todo |
| G4 | The `/rbf` page with the full-RBF filter, and recent replacements on the home page | C1b | todo |
| G5 | Address balance chart | C1 | done ([#79](https://github.com/smolcars/rbitcoin-space/pull/79)) |
| G6 | Later: Lightning nodes by network type over time; a BTC/sats units switch; `/clock` | C1 | todo |
| G7 | After the owner's decision on a broadcast page: `/tx/push`, then `/tx/preview` (needs a transaction and PSBT parser in the browser), then goggles (needs per-transaction flags from the engine) | C1 | todo; waits on the owner |
| G8 | Owner's decisions from the audit: a broadcast page (the route is already served under its rules); package submission and test-accept (the audit recommends skipping both); fiat prices stay parked | C1 | waits on the owner |
| RD1 | Redesign by design council (the owner's process): three independent reviewers score the site in ten categories against the explorer its readers know; a builder acts on their findings; repeat for at most five rounds or until every reviewer gives 9 in every category; one pull request at the end with every round's scores and screenshots, left open for the owner. New frontend tasks (G2, G3, G4, G6, Q1, Q2, D2) wait until it is done | SB1, PF4, G5 | doing: round 1 council scores were 6 to 7 overall (lowest category 5); the builder is on the round 1 build |
| Q1 | Link previews for block, transaction and address pages | W3, W4, W5 | todo |
| Q2 | Accessibility pass | M4 | todo |
| Q3 | Performance pass against the budgets | M4 | done ([#70](https://github.com/smolcars/rbitcoin-space/pull/70)); results in `docs/PERFORMANCE.md`; its PR took 3 min 4 s of CI on a partly cold cache, so check the next Rust PR against the budget |
| PF1 | Count rate-limit refusals instead of logging each one: a single flooding client made the server write 911 MB of log in 30 s | Q3 | done ([#71](https://github.com/smolcars/rbitcoin-space/pull/71)); a 30 s flood now writes 0.96 MB of log, down from 1,080 MB |
| PF2 | Build store-backed answers once per tip instead of per request: they cost 8 ms at 100 clients and 46 ms at 1,000 | Q3 | approved; fixing a store test that fails on Linux CI before it can merge ([#77](https://github.com/smolcars/rbitcoin-space/pull/77)) |
| PF3 | Hold immutable node answers in memory: `/api/block/:hash` misses the 10 ms budget because it is passed through uncached | Q3 | approved; fixing a store test that fails on Linux CI before it can merge ([#77](https://github.com/smolcars/rbitcoin-space/pull/77)) |
| PF4 | Shorten the page's request chain on slow networks: `/app/config` waits behind 15 small chunks on six connections, about 1 to 1.5 s | Q3 | doing |
| PF5 | Find what stops WebSocket delivery past 8,000 sockets (delivery stalled at 10,000; kernel buffers are suspected, not proven), then document host limits in the operator guide | Q3, Q4 | todo |
| PF6 | API `OPTIONS` preflights are answered before the rate limits, so only the log ceiling bounds them: put them under a limit of their own | PF1 | done ([#76](https://github.com/smolcars/rbitcoin-space/pull/76)) |
| Q4 | Operator guide and release workflow with multi-architecture images; the guide must say that the reverse proxy compresses `/api` and `/app` answers, since the server does not | P1 | done ([#63](https://github.com/smolcars/rbitcoin-space/pull/63)); nothing is published: the first release is the owner's decision |
| OP1 | Operator-facing gaps the guide's author found: say at startup when the node's address index is off; make store trouble visible in `/readyz` and name a full disk in the log; report the build's revision beside the version in `/healthz`; correct the 21-hour backfill estimate in `docs/CONFIG.md`; stop the Dockerfile's shared build cache producing a stale local image; and bring three lines of `docs/DEPLOY.md` in line with PF1 (refusals are summarised, `log_events_dropped` can mean the ceiling, a misconfigured proxy shows in `requests_refused`), linking to `docs/LOGGING.md#for-operators` | Q4 | done ([#74](https://github.com/smolcars/rbitcoin-space/pull/74)); it also repaired the image build, broken on master since #70 |
| Q5 | Cross-browser check in Safari and Firefox, including the WebSocket under the Content-Security-Policy | M4 | done ([#66](https://github.com/smolcars/rbitcoin-space/pull/66)); real Safari not covered, hand checklist in the PR; follow-ups SB1 to SB3 |
| SB1 | Safari runs digit groups together ("1293 WU" for "1 293 WU"): the narrow space used for grouping is missing from the font subset | Q5 | done ([#75](https://github.com/smolcars/rbitcoin-space/pull/75)); real Safari not driven: the owner should check digit gaps and recovery on a phone |
| SB2 | Safari never recovers a page whose script failed to download: it stays at "Loading this page" because the retry does not ask again | Q5 | done ([#75](https://github.com/smolcars/rbitcoin-space/pull/75)) |
| SB3 | The mining page scrolls sideways on phones on regtest (its tiny difficulty number); mainnet fits | Q5 | done ([#75](https://github.com/smolcars/rbitcoin-space/pull/75)) |
| SB4 | The blocks table is 321 to 325 px wide in a 288 px space at phone width when the font arrives late or not at all (two columns grow); found on GitHub's runners. Fold into the redesign | SB1 | todo; part of RD1 |
| SB5 | The first transaction-grid tests time out in Firefox on GitHub's runner about half the time (reproduced on a one-CPU Linux container): fix how they wait, without longer timeouts | SB1 | todo |
| FX9 | The node call sharing in the Esplora pass-through has a cleanup race that can run the same computation twice, the same one the review found in the new answer cache | PF2 | todo |
