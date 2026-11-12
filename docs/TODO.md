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
| W2 | Home page: latest blocks, WebSocket client and live block push, mempool summary | W1, S2, S3 | doing |
| W3 | Block page: header details, paged transaction list | W1, S2 | doing |
| W4 | Transaction page: inputs, outputs, fee, status, live confirmation | W1, S2, S3 | doing |
| W5 | Address page: totals, unspent outputs, paged history; large addresses show history without totals and say why | W1, S2 | doing |
| W6 | Search: txid, block hash, height, address (exact match) | W1, S2 | todo |
| E1 | End-to-end harness: headless browser against the server on regtest, inside the CI budget; fix the image build broken by the app shell and guard against a repeat | F0, R2, S1, W1 | done ([#15](https://github.com/smolcars/mempool.based/pull/15), [#21](https://github.com/smolcars/mempool.based/pull/21)) |
| P1 | Docker image and compose file beside rbitcoin | S1 | done ([#10](https://github.com/smolcars/mempool.based/pull/10)) |
| A1 | Scheduled dependency audit against the advisory database, outside the PR path | F0 | todo |
| U1 | Move the regtest harness and compose files to the rbitcoin v0.9.2 image; confirm the scenario's hashes and the node clients are unchanged | R2, P1 | done ([#14](https://github.com/smolcars/mempool.based/pull/14)) |
| LX1 | Switch the frontend to OXC: `oxlint` with strict rules and `oxfmt` as the only formatter, Biome removed, TypeScript options tightened, all enforced in CI. Runs when the page tasks in flight (W2–W5) have merged, before any other frontend task, because it reformats every file | W2, W3, W4, W5 | todo |
| FX2 | Harden the WebSocket hub: retry the new-block fetch quickly so a slow Esplora cannot turn a block into a resync; run a WebSocket conformance suite and a no-panic test against our hand-written framing; tag the socket-cap refusal in the request log | S3 | todo |
| L1 | Structured logging: a `logging` crate of typed events, one event per API request, no stray prints; `docs/LOGGING.md` | S1 | done ([#12](https://github.com/smolcars/mempool.based/pull/12)) |

## M2 — Live mempool

Refined when M1 lands.

| ID | Task | Needs | Status |
|---|---|---|---|
| MP1 | Mempool tracker: sequence-aware poll and diff, bulk detail fetch, removal reasons | N1, N3 | done ([#20](https://github.com/smolcars/mempool.based/pull/20)) |
| MP2 | Projected blocks from cluster chunks and the block template | MP1 | doing |
| MP3 | Binary WebSocket deltas for projected blocks | MP2, S3 | todo |
| MP4 | Fee recommendations, and the matching `/api/v1/fees` endpoints | MP2 | todo |
| MP5 | Transaction and address tracking over shared, capped Electrum subscriptions | S3, N2 | todo |
| MP6 | RBF history and CPFP or cluster view | MP1 | todo |
| BS1 | Block summary route in `/app`: every transaction's fee and size from `getblock` verbosity 2, cached, plus the block's median fee rate and fee range | N3, S2 | todo |
| W7 | WebGL block renderer for projected and mined blocks; fills the grid slot the block page reserves | W1, MP3, BS1 | todo |
| W8 | Mempool page: live flow, fee bands | W7 | todo |
| DA1 | Difficulty adjustment (two headers and arithmetic), for the home page and `/api/v1/difficulty-adjustment` | N3, S2 | todo |
| WS1 | mempool.space-shaped WebSocket subset at `/api/v1/ws` (`want`, `track-tx`, `track-address`, `ping`) from the same hub | S3, MP5 | todo |

## M3 — Mining and history

Refined when M2 lands.

| ID | Task | Needs | Status |
|---|---|---|---|
| ST1 | SQLite store: migrations, single writer task, read-only readers | F0 | doing |
| MI1 | Block summaries with backfill: totals from `getblockstats`; our own fee-rate percentiles from per-transaction data, newest first in the background | ST1, N1, N3 | todo |
| MI2 | Mining pool attribution from coinbase tags and addresses; needs a pool list under an MIT-compatible licence, or our own | MI1 | todo |
| MI3 | Hashrate history | MI1 | todo |
| MI4 | Block audit: expected template against the mined block | MP2, MI1 | todo |
| MI5 | Mempool history series, one sample a minute | ST1, MP1 | todo |
| PR1 | Fiat prices with an off switch | ST1 | todo |
| W9 | Mining dashboard and pool pages | MI2, MI3 | todo |
| W10 | Charts with uPlot | MI5 | todo |

## M4 — Lightning

Refined when M3 lands. Design in [`LIGHTNING.md`](LIGHTNING.md).

| ID | Task | Needs | Status |
|---|---|---|---|
| LN1 | Gossip peer with LDK: peer connections, graph sync, graph persistence | F0 | todo |
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
| Q4 | Operator guide and release workflow with multi-architecture images | P1 | todo |
| Q5 | Cross-browser check in Safari and Firefox, including the WebSocket under the Content-Security-Policy | M4 | todo |
