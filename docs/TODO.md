# TODO

The task list. The orchestrator owns this file; builders do not edit it. Milestones are described in [`PLAN.md`](PLAN.md).

Status is one of: `todo`, `doing`, `done` (with the PR), `skipped` (with the reason).

## M0 — Foundation

| ID | Task | Needs | Status |
|---|---|---|---|
| F0 | Rust workspace, skeleton server (`/healthz`, static files), skeleton frontend, CI within budget | — | todo |
| R1 | Parity inventory: fill [`COMPAT.md`](COMPAT.md) from mempool.space's observable API and rbitcoin's surface | — | todo |
| R2 | Regtest harness: rbitcoin v0.9.1 in Docker, scripts to mine and transact, `docs/REGTEST.md` | — | todo |
| D1 | Design language and static prototype; rewrite [`DESIGN.md`](DESIGN.md) | — | todo |

## M1 — Explorer skeleton

| ID | Task | Needs | Status |
|---|---|---|---|
| N1 | Esplora client: TCP and unix socket, typed models, chain-tip stamp handling | F0 | todo |
| N2 | Electrum client: requests, subscriptions (headers, scripthash, outpoint), reconnect | F0 | todo |
| N3 | RPC client: fixed set of typed read-only methods | F0 | todo |
| S1 | Server core: multi-network config, limits and rate-limit middleware, error model, security headers | F0 | todo |
| S2 | Esplora pass-through at `/api`: route allow-list, typed parameters, cache headers | N1, S1 | todo |
| S3 | WebSocket hub: bounded connections and topics, new-block push from Electrum; `docs/WS.md` | N2, S1 | todo |
| W1 | App shell: router with network prefixes, layout, theme and tokens, HTTP and WebSocket data layer | F0, D1 | todo |
| W2 | Home page: latest blocks, live block push, mempool summary | W1, S2, S3 | todo |
| W3 | Block page: header details, paged transaction list | W1, S2 | todo |
| W4 | Transaction page: inputs, outputs, fee, status, live confirmation | W1, S2, S3 | todo |
| W5 | Address page: totals, unspent outputs, paged history | W1, S2 | todo |
| W6 | Search: txid, block hash, height, address (exact match) | W1, S2 | todo |
| E1 | End-to-end harness: headless browser against the server on regtest, inside the CI budget | F0, R2 | todo |
| P1 | Docker image and compose file beside rbitcoin | S1 | todo |

## M2 — Live mempool

Refined when M1 lands.

| ID | Task | Needs | Status |
|---|---|---|---|
| MP1 | Mempool tracker: sequence-aware poll and diff, bulk detail fetch, removal reasons | N1, N3 | todo |
| MP2 | Projected blocks from cluster chunks and the block template | MP1 | todo |
| MP3 | Binary WebSocket deltas for projected blocks | MP2, S3 | todo |
| MP4 | Fee recommendations, and the matching `/api/v1/fees` endpoints | MP2 | todo |
| MP5 | Transaction and address tracking over shared, capped Electrum subscriptions | S3, N2 | todo |
| MP6 | RBF history and CPFP or cluster view | MP1 | todo |
| W7 | WebGL block renderer for projected and mined blocks | W1, MP3 | todo |
| W8 | Mempool page: live flow, fee bands | W7 | todo |

## M3 — Mining and history

Refined when M2 lands.

| ID | Task | Needs | Status |
|---|---|---|---|
| ST1 | SQLite store: migrations, single writer task, read-only readers | F0 | todo |
| MI1 | Block summaries with backfill: reward, fees, our own fee-rate percentiles | ST1, N1, N3 | todo |
| MI2 | Mining pool attribution from coinbase tags and addresses | MI1 | todo |
| MI3 | Hashrate and difficulty adjustment | MI1 | todo |
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
