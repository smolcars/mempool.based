# TODO

The task list. The orchestrator owns this file; builders do not edit it. Milestones are described in [`PLAN.md`](PLAN.md).

Status is one of: `todo`, `doing`, `done` (with the PR), `skipped` (with the reason).

## M0 — Foundation

| ID | Task | Needs | Status |
|---|---|---|---|
| F0 | Rust workspace, skeleton server (`/healthz`, static files), skeleton frontend, CI within budget | — | done ([#1](https://github.com/smolcars/mempool.based/pull/1)) |
| R1 | Parity inventory: fill [`COMPAT.md`](COMPAT.md) from mempool.space's observable API and rbitcoin's surface | — | done ([#6](https://github.com/smolcars/mempool.based/pull/6)) |
| R2 | Regtest harness: rbitcoin v0.9.1 in Docker, scripts to mine and transact, `docs/REGTEST.md` | — | done ([#2](https://github.com/smolcars/mempool.based/pull/2)) |
| D1 | Design language and static prototype; rewrite [`DESIGN.md`](DESIGN.md) | — | done ([#5](https://github.com/smolcars/mempool.based/pull/5)) |

## M1 — Explorer skeleton

| ID | Task | Needs | Status |
|---|---|---|---|
| N1 | Esplora client: TCP and unix socket, typed models, chain-tip stamp handling | F0 | done ([#3](https://github.com/smolcars/mempool.based/pull/3)) |
| N2 | Electrum client: requests, subscriptions (headers, scripthash, outpoint), reconnect | F0 | done ([#8](https://github.com/smolcars/mempool.based/pull/8)) |
| N3 | RPC client: fixed set of typed read-only methods | F0 | done ([#7](https://github.com/smolcars/mempool.based/pull/7)) |
| S1 | Server core: multi-network config, limits and rate-limit middleware, error model, security headers | F0 | done ([#4](https://github.com/smolcars/mempool.based/pull/4)) |
| S2 | Esplora pass-through at `/api`: route allow-list, typed parameters, cache headers | N1, S1 | done ([#9](https://github.com/smolcars/mempool.based/pull/9)) |
| S3 | WebSocket hub: bounded connections and topics, new-block push from Electrum; `docs/WS.md` | N2, S1 | done ([#22](https://github.com/smolcars/mempool.based/pull/22)) |
| W1 | App shell: router with network prefixes, layout, theme and tokens, shared components, HTTP data layer | F0, D1 | done ([#11](https://github.com/smolcars/mempool.based/pull/11)) |
| W2 | Home page: latest blocks, WebSocket client and live block push, mempool summary | W1, S2, S3 | done ([#26](https://github.com/smolcars/mempool.based/pull/26)) |
| W3 | Block page: header details, paged transaction list | W1, S2 | done ([#23](https://github.com/smolcars/mempool.based/pull/23)) |
| W4 | Transaction page: inputs, outputs, fee, status, live confirmation | W1, S2, S3 | done ([#24](https://github.com/smolcars/mempool.based/pull/24)) |
| W5 | Address page: totals, unspent outputs, paged history; large addresses show history without totals and say why | W1, S2 | done ([#25](https://github.com/smolcars/mempool.based/pull/25)) |
| W6 | Search: txid, block hash, height, address (exact match) | W1, S2, LX1 | done ([#32](https://github.com/smolcars/mempool.based/pull/32)) |
| D2 | Bring `docs/DESIGN.md` in line with what was built: the page builders' corrections (address page sketch, transactions listed in block order, the Live indicator's place, arrows the font has, duration tokens the build rewrites) | LX1 | todo |
| FE1 | Frontend follow-ups from the page tasks: let the API client expose the chain-tip header so a page can tell when the tip moved between its requests; stop the footer jumping when a page first mounts | LX1 | done ([#33](https://github.com/smolcars/mempool.based/pull/33)) |
| E1 | End-to-end harness: headless browser against the server on regtest, inside the CI budget; fix the image build broken by the app shell and guard against a repeat | F0, R2, S1, W1 | done ([#15](https://github.com/smolcars/mempool.based/pull/15), [#21](https://github.com/smolcars/mempool.based/pull/21)) |
| P1 | Docker image and compose file beside rbitcoin | S1 | done ([#10](https://github.com/smolcars/mempool.based/pull/10)) |
| A1 | Scheduled dependency audit against the advisory database, outside the PR path | F0 | todo |
| QA1 | Acceptance check of milestone M1 against its exit criteria: the built server and the Docker image on all three networks in a real browser, numbers cross-checked with the node, hostile input, the performance budgets; findings become tasks | W6 | done: exit criteria met on all three networks; findings became FX3, FE4 and FX5 |
| FX3 | Server fixes from the M1 acceptance check: the per-client limits must not break an ordinary page load (assets were rate-limited); the proxy header must not be trusted by default or from an untrusted peer; a cost limit on address routes; same-origin WebSocket upgrades on `/app/ws`; a wrong RPC password reported as such; the acceptance check's hostile-request scripts turned into tests | QA1 | done ([#39](https://github.com/smolcars/mempool.based/pull/39)) |
| FE4 | Frontend fixes from the M1 acceptance check: retry a failed chunk load; the home page's duplicate requests; overlapping fee labels; the block page's transaction table on phones; the genesis coinbase shown as spendable; amounts without units for screen readers; consistent size units; the smaller wording issues; a browser test with hostile chain data | QA1, W7 | doing |
| FX5 | Show a new block the moment Electrum announces it, from its header, and fill in the rest when the node serves it, so the node's hold of up to 0.4 s is not felt | S3, W2 | todo |
| U1 | Move the regtest harness and compose files to the rbitcoin v0.9.2 image; confirm the scenario's hashes and the node clients are unchanged | R2, P1 | done ([#14](https://github.com/smolcars/mempool.based/pull/14)) |
| LX1 | Switch the frontend to OXC: `oxlint` with strict rules and `oxfmt` as the only formatter, Biome removed, TypeScript options tightened, all enforced in CI. Runs when the page tasks in flight (W2–W5) have merged, before any other frontend task, because it reformats every file | W2, W3, W4, W5 | done ([#29](https://github.com/smolcars/mempool.based/pull/29)) |
| FX2 | Harden the WebSocket hub: retry the new-block fetch quickly so a slow Esplora cannot turn a block into a resync; run a WebSocket conformance suite and a no-panic test against our hand-written framing; tag the socket-cap refusal in the request log | S3 | done ([#31](https://github.com/smolcars/mempool.based/pull/31)) |
| L1 | Structured logging: a `logging` crate of typed events, one event per API request, no stray prints; `docs/LOGGING.md` | S1 | done ([#12](https://github.com/smolcars/mempool.based/pull/12)) |

## M2 — Live mempool

Refined when M1 lands.

| ID | Task | Needs | Status |
|---|---|---|---|
| MP1 | Mempool tracker: sequence-aware poll and diff, bulk detail fetch, removal reasons | N1, N3 | done ([#20](https://github.com/smolcars/mempool.based/pull/20)) |
| MP2 | Projected blocks from cluster chunks and the block template | MP1 | done ([#28](https://github.com/smolcars/mempool.based/pull/28)) |
| MP3 | Run the mempool tracker and projected blocks inside the server, and serve them over the WebSocket: mempool totals, projected-block totals, binary per-transaction deltas, the latest transactions | MP2, S3 | done ([#35](https://github.com/smolcars/mempool.based/pull/35)) |
| MP4 | Fee recommendations, and the matching `/api/v1/fees` endpoints | MP2 | doing |
| MP5 | Transaction and address tracking over shared, capped Electrum subscriptions | S3, N2 | doing |
| MP6 | RBF history and CPFP or cluster view | MP1 | doing (with MP5) |
| BS1 | Block summary route in `/app`: every transaction's fee and size from `getblock` verbosity 2, cached, plus the block's median fee rate and fee range | N3, S2 | done ([#27](https://github.com/smolcars/mempool.based/pull/27)) |
| FE3 | Show block summaries and the difficulty adjustment in the frontend: median fee rate and fee range on the block page, fee layers and fee figures in the home page's tiles (batch route on load, the WebSocket's `summary` message for new blocks, retry on 503) | BS1, LX1 | done ([#33](https://github.com/smolcars/mempool.based/pull/33)) |
| W7 | WebGL block renderer for projected and mined blocks; fills the grid slot the block page reserves | W1, MP3, BS1 | done ([#36](https://github.com/smolcars/mempool.based/pull/36)) |
| FE5 | The live mempool on the home page: projected-block tiles with their fee layers, the mempool panel and the latest transactions from the WebSocket topics; the projected tile becomes the mined one when a block arrives | MP3, FE3 | doing |
| W8 | Projected-block page: one projected block's transactions drawn with the grid and updated live | W7, MP3 | doing |
| FL1 | Find the cause of two intermittent test failures builders saw once each and could not reproduce (a browser test unable to open the WebSocket; one unidentified Rust test failure) | E1 | likely explained by [#39](https://github.com/smolcars/mempool.based/pull/39): the per-address connection cap closed connections without logging; watch for a recurrence |
| DA1 | Difficulty adjustment (two headers and arithmetic), for the home page and `/api/v1/difficulty-adjustment` | N3, S2 | done ([#30](https://github.com/smolcars/mempool.based/pull/30)) |
| WS1 | mempool.space-shaped WebSocket subset at `/api/v1/ws` (`want`, `track-tx`, `track-address`, `ping`) from the same hub | S3, MP5 | todo |

## M3 — Mining and history

Refined when M2 lands.

| ID | Task | Needs | Status |
|---|---|---|---|
| ST1 | SQLite store: migrations, single writer task, read-only readers | F0 | done ([#13](https://github.com/smolcars/mempool.based/pull/13)) |
| MI1 | Block summaries with backfill: totals from `getblockstats`; our own fee-rate percentiles from per-transaction data, newest first in the background | ST1, N1, N3 | done ([#34](https://github.com/smolcars/mempool.based/pull/34)); the full mainnet backfill has not been run |
| MI2 | Mining pool attribution from coinbase tags and addresses; needs a pool list under an MIT-compatible licence, or our own | MI1 | done ([#38](https://github.com/smolcars/mempool.based/pull/38)) |
| MI3 | Hashrate history | MI1 | done ([#38](https://github.com/smolcars/mempool.based/pull/38)) |
| MI4 | Block audit: expected template against the mined block | MP2, MI1 | todo |
| MI5 | Mempool history series, one sample a minute | ST1, MP1 | todo |
| PR1 | Fiat prices with an off switch | ST1 | todo |
| W9 | Mining dashboard and pool pages | MI2, MI3 | todo |
| W10 | Charts with uPlot | MI5 | todo |

## M4 — Lightning

Refined when M3 lands. Design in [`LIGHTNING.md`](LIGHTNING.md).

| ID | Task | Needs | Status |
|---|---|---|---|
| LN1 | Gossip peer with LDK: peer connections, graph sync, graph persistence | F0 | done ([#37](https://github.com/smolcars/mempool.based/pull/37)); the graph is unverified until LN2 |
| LN2 | Funding-output lookups through rbitcoin's position routes: batched, rate-limited | LN1, N1 | todo |
| LN3 | Channel opens and closes from blocks, with close classification | LN2 | todo |
| LN4 | Lightning store, statistics snapshots and API | LN3, ST1 | todo |
| W11 | Lightning pages: network statistics, node, channel, search | LN4, W1 | todo |

## M5 — Parity and polish

Refined when M4 lands.

| ID | Task | Needs | Status |
|---|---|---|---|
| C1 | Remaining [`COMPAT.md`](COMPAT.md) rows marked for delivery | M4 | todo |
| Q1 | Link previews for block, transaction and address pages | W3, W4, W5 | todo |
| Q2 | Accessibility pass | M4 | todo |
| Q3 | Performance pass against the budgets | M4 | todo |
| Q4 | Operator guide and release workflow with multi-architecture images; the guide must say that the reverse proxy compresses `/api` and `/app` answers, since the server does not | P1 | todo |
| Q5 | Cross-browser check in Safari and Firefox, including the WebSocket under the Content-Security-Policy | M4 | todo |
