# Pages and visible features

Part of the parity inventory in [`../COMPAT.md`](../COMPAT.md), which defines the columns, status values and the `Live` legend.

Paths are mempool.space's; ours mirror them under the network prefixes in [`ARCHITECTURE.md`](../ARCHITECTURE.md). `ref` here means the page rendered with its own title or heading on the reference frontend in headless Chromium. An unknown path there falls back to the home page, so a page that rendered as the home page is not counted.

## Explorer

| Page or feature | Shows | Source | Status | M | Live | Note |
|---|---|---|---|---|---|---|
| Home `/` | Projected and recent blocks, fees, mempool size and inflow, difficulty progress, latest transactions and replacements | Engine, Esplora, store | planned | M1 | ref | Blocks in M1 (W2), projected blocks and fees in M2, difficulty and charts in M3 |
| Block strip | Projected blocks to the left of mined blocks, on most pages | Engine, Esplora | planned | M2 | ref | |
| Projected block `/mempool-block/:index` | One projected block's fee range, size, and its transactions drawn | Engine | planned | M2 | ref | W7, W8 |
| Block `/block/:hash` or `/block/:height` | Header, pool, reward, fee range, transactions drawn, paged list, expected-versus-mined view | Esplora; RPC `getblock`; store for audit | planned | M1 | ref | Header and list in M1 (W3), drawing in M2, audit in M3 |
| Blocks `/blocks` | Paged block list with pool, fees, size, health | Esplora; store | planned | M1 | ref | Pool and health columns from M3 |
| Transaction `/tx/:txid` | Inputs and outputs, flow diagram, fee and fee rate, status, ETA, live confirmation, RBF history, CPFP, Lightning channel links | Esplora; engine; Lightning store | planned | M1 | — | Did not finish rendering headlessly. Basics in M1 (W4), ETA, RBF and CPFP in M2, channel links in M4 |
| Address `/address/:address` | Balance, totals, balance chart, paged history, live updates | Esplora; Electrum push | planned | M1 | ref | Live updates in M2. Large addresses hit the node's 503 cap on totals; show history anyway and say why totals are missing |
| Search | Exact txid, block hash, height, address; Lightning node and channel | Esplora; Lightning store | planned | M1 | — | Lightning results in M4. Prefix autocomplete is out of scope |
| RBF `/rbf` | Recent replacement trees, with a full-RBF filter | Engine | planned | M2 | ref | |
| Mempool goggles | Filters on the block drawing by transaction kind | Engine flags | planned | M5 | — | Needs the same per-tx flags as `/v1/block/:hash/summary` |

## Transaction tools

| Page | Does | Source | Status | M | Live | Note |
|---|---|---|---|---|---|---|
| Broadcast `/tx/push` | Broadcasts a transaction or a package, with max fee rate and burn limits | rbitcoin `POST /tx`, `POST /txs/package` | planned | M5 | ref | Same caps and switch as the API |
| Test `/tx/test` | Test-accepts transactions without broadcasting | rbitcoin `POST /txs/test` | planned | M5 | ref | |
| Preview `/tx/preview` | Decodes a pasted transaction or PSBT and shows it as a tx page | Our decoder; prevouts from Esplora | planned | M5 | ref | |

## Mining

| Page | Shows | Source | Status | M | Live | Note |
|---|---|---|---|---|---|---|
| Mining dashboard `/mining` | Reward stats, pool share, hashrate, difficulty, recent blocks and adjustments | Store | planned | M3 | ref | W9 |
| Pool `/mining/pool/:slug` | One pool's share, hashrate chart, blocks | Store | planned | M3 | — | Did not finish rendering headlessly; the reference's API took 58 s for it |
| Pool ranking (`/graphs/mining/pools`) | Pools by blocks over a period | Store | planned | M3 | ref | |

## Graphs

All under `/graphs`, drawn with uPlot (W10).

| Graph | Shows | Source | Status | M | Live | Note |
|---|---|---|---|---|---|---|
| Mempool `/graphs/mempool` | Mempool vsize by fee band, and inflow, over time | Store (MI5) | planned | M3 | ref | |
| Hashrate and difficulty | Network hashrate and difficulty over time | Store | planned | M3 | ref | |
| Pool dominance | Pool shares over time | Store | planned | M3 | ref | |
| Block fees | Average fees per block over time, BTC and fiat | Store | planned | M3 | ref | |
| Block rewards | Average reward per block over time | Store | planned | M3 | ref | |
| Block fee rates | Fee-rate percentiles per block over time | Store | planned | M3 | ref | Sub-sat percentiles |
| Block sizes and weights | Size and weight over time | Store | planned | M3 | ref | |
| Block health | Audit match rate per block over time | Store (audits) | planned | M3 | ref | |
| Lightning graphs | Nodes, channels and capacity over time, by network type | Lightning store | planned | M4 | — | |

## Lightning

| Page | Shows | Source | Status | M | Live | Note |
|---|---|---|---|---|---|---|
| Lightning dashboard `/lightning` | Network totals, top nodes, recent channels, charts | Lightning store | planned | M4 | ref | The reference runs with Lightning off, so only the page shell was checked |
| Node `/lightning/node/:pubkey` | Alias, sockets, features, capacity, channels, fee histogram, history | Lightning store | planned | M4 | — | No location data |
| Channel `/lightning/channel/:id` | Both policies, capacity, funding and closing transactions | Lightning store; Esplora | planned | M4 | — | |
| Rankings `/lightning/nodes/rankings` | Top nodes by liquidity and connectivity, oldest nodes | Lightning store | planned | M4 | ref | |
| Country, ISP and group pages | Nodes by location, hosting provider or curated group | — | skipped | — | — | Needs GeoIP or ASN data, or another project's curation |
| World map and channel map | Nodes and channels on a map | — | skipped | — | — | Out of scope |

## Other pages

| Page | Shows | Source | Status | M | Live | Note |
|---|---|---|---|---|---|---|
| Clock `/clock` | Full-screen tip height, price, fee and mempool size | Engine; store | planned | M5 | ref | Cheap once M3 data exists |
| TV `/tv` | Full-screen block strip and mempool chart for a wall display | Engine; store | planned | M5 | — | Rendered as the home page headlessly |
| Calculator `/tools/calculator` | Converts between BTC, sats and fiat | Store (prices) | planned | M5 | ref | |
| API docs `/docs/api/rest`, `/docs/api/websocket` | Endpoint reference | This inventory | different | M5 | ref | We document our own `/api`, `/api/v1` and WebSocket in our words |
| FAQ `/docs/faq` | Explainers | — | different | M5 | ref | Our own operator guide (Q4) |
| About, privacy, terms | Project, sponsors, policies | — | skipped | — | ref | About another project and its business |
| Accelerator pages | Paid acceleration | — | skipped | — | — | Out of scope |

## Site-wide features

| Feature | What it does | Status | M | Live | Note |
|---|---|---|---|---|---|---|
| Network switch | Mainnet, signet, testnet4 | planned | M1 | ref | Plus regtest for development |
| Light and dark themes | Theme choice | planned | M1 | — | W1 |
| Units | BTC or sats | planned | M1 | — | |
| Fiat currency choice | Currency for converted values | planned | M3 | — | Needs prices |
| Time display | Relative or absolute times, local time zone | planned | M1 | — | |
| Languages | Translated interface | skipped | — | ref | Out of scope; English only |
| Link previews | Social cards for block, tx and address pages | planned | M5 | — | Q1 |
| Liquid | Liquid network and assets | skipped | — | — | Out of scope |
