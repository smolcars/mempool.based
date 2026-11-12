# mempool.space REST (`/api/v1`)

Part of the parity inventory in [`../COMPAT.md`](../COMPAT.md), which defines the columns, status values and the `Live` legend.

rbitcoin serves none of these; this project is that surface ([`RBITCOIN.md`](../RBITCOIN.md)). "Engine" means our in-memory mempool tracker and projected blocks; "store" means our SQLite file. A route marked `planned` is served at `/api/v1/…` in the shape observed on the reference backend. Our own frontend may read the same data through `/app` in another shape.

## General

| Endpoint | Returns | Source | Status | M | Live | Note |
|---|---|---|---|---|---|---|
| `GET /v1/difficulty-adjustment` | Progress through the epoch, expected change, estimated retarget time, blocks left | Esplora: the first blocks of this period and the last, once per new tip; engine | served | M2 | ref | Same fields, order and types. Projects from block timestamps, so `timeAvg` and `difficultyChange` differ from the reference; regtest reports no change ([`API.md`](../API.md#difficulty-adjustment)) |
| `GET /v1/prices` | Latest BTC price in seven fiat currencies | Store, from a price feed | planned | M3 | ref | Off switch per [`PLAN.md`](../PLAN.md) |
| `GET /v1/historical-price[?currency=&timestamp=]` | Price history, or the price nearest a time, plus exchange rates | Store | planned | M3 | ref | The unfiltered form is 300 KB |
| `GET /v1/validate-address/:address` | Validity, script, witness version and program | Our own address parser; rbitcoin `validateaddress` matches field for field | planned | M5 | ref | Parse locally; no node call needed |
| `GET /v1/statistics/:interval` | Mempool samples over 2h, 24h, 1w … all: count, vB/s, fee-band vsizes, min fee | Store | planned | M3 | ref | Long intervals drop `total_fee` and `mempool_byte_weight`. Sampled once a minute (MI5) |
| `GET /v1/transaction-times?txId[]=` | First-seen time for each txid, `0` when unknown | Engine | planned | M2 | ref | Confirmed transactions read `0` on the reference too. Persisting first-seen needs the store (M3) |
| `GET /v1/backend-info` | Backend kind, version, commit, and the host's name and OS | Server build info | different | M5 | ref | We return version and commit only. Host name and OS stay private ([`SECURITY.md`](../SECURITY.md) rule 17) |
| `GET /v1/init-data` | One snapshot of everything the home page needs | Engine | different | M2 | ref | Our frontend gets the same from the first WebSocket message on `/app/ws` |
| `GET /v1/address-prefix/:prefix` | Address autocomplete | — | skipped | — | ref, pub | Out of scope. 404 on the reference |
| `/v1/donations`, `/v1/contributors`, `/v1/translators` | mempool.space's own sponsor and contributor lists | — | skipped | — | ref | Data about another project |

## Fees

| Endpoint | Returns | Source | Status | M | Live | Note |
|---|---|---|---|---|---|---|
| `GET /v1/fees/recommended` | Five fee levels in whole sat/vB: fastest, half hour, hour, economy, minimum | Engine (projected blocks and recent arrivals) | served | M2 | ref | Whole numbers rounded up, at least 1: on a 0.1 sat/vB mempool the floor alone overstates the minimum tenfold. Kept for old clients; `X-Fee-Source` says when it is the node's estimate ([`API.md`](../API.md#fees)) |
| `GET /v1/fees/precise` | The same five levels with sub-sat precision | Engine | served | M2 | ref | Three decimals, the node's resolution. The honest one on rbitcoin. Our own method; values beside the reference's in [`MEMPOOL.md`](../MEMPOOL.md#against-the-node-and-the-reference) |
| `GET /v1/fees/mempool-blocks` | Projected blocks: size, vsize, tx count, total fees, median fee, fee range | Engine (projected blocks from cluster chunks) | served | M2 | ref | Eight blocks at most, the last holding the rest of the mempool; empty ones left out. `feeRange` is always seven rates by vsize |

## Mempool, RBF and CPFP

| Endpoint | Returns | Source | Status | M | Live | Note |
|---|---|---|---|---|---|---|
| `GET /v1/replacements` | Recent replacement trees: new tx, time, what it replaced | Engine | planned | M2 | ref | Removal reasons from MP1 |
| `GET /v1/fullrbf/replacements` | Only trees where a replaced tx did not signal | Engine | planned | M2 | ref | rbitcoin is always full-RBF, so this is a filter on signalling, not on policy |
| `GET /v1/tx/:txid/rbf` | The replacement tree around one transaction | Engine; store for history | planned | M2 | ref | `null` fields when there is none |
| `GET /v1/cpfp/:txid` | Ancestors, descendants, best descendant, effective fee rate, sigops | rbitcoin `getmempoolancestors`, `getmempooldescendants`, `getmempoolentry` (`depends`); `getmempoolcluster` for the chunk fee rate | planned | M2 | ref | Better on rbitcoin: the effective fee rate is the node's own chunk fee rate, not an estimate. The cluster call has no edges, so relationships come from the ancestor and descendant calls. Confirmed txs read empty on the reference |
| `POST /v1/cpfp` | CPFP data for several transactions at once | Engine | planned | M5 | ref | Listed on the public docs page; the reference rejected our body format with 400, so the request shape is unconfirmed |
| `GET /v1/tx/:txid/cached` | Whether the backend holds the tx in its cache | — | skipped | — | ref | A frontend helper with no meaning outside it; 204 on the reference |
| `POST /v1/prevouts` | Prevout and unconfirmed flag for a list of outpoints | rbitcoin `GET /tx/:txid/out/:vout`, mempool for unconfirmed | planned | M5 | ref | Backs the transaction preview page |
| `POST /v1/psbt/addparents` | A PSBT with its parents' data filled in | rbitcoin Esplora `/tx/:txid/hex` | planned | M5 | ref | Route exists: the reference's handler refused an invalid PSBT. PSBT parsing is untrusted input |
| `POST /v1/tx/push` | Broadcast through the `/v1` prefix | rbitcoin `POST /tx` | different | — | — | Not confirmed: a malformed body fails JSON parsing before routing on the reference, and a valid one would broadcast. Use `POST /api/tx` |
| `POST /v1/txs/package` | Package submit through the `/v1` prefix | rbitcoin `POST /txs/package` | different | — | ref | Route exists on the reference. Use `POST /api/txs/package` |

## Blocks

| Endpoint | Returns | Source | Status | M | Live | Note |
|---|---|---|---|---|---|---|
| `GET /v1/blocks[/:start_height]` | 15 blocks with `extras`: pool, reward, fee range, median fee, totals, coinbase, audit scores | Esplora for headers; store for extras | planned | M3 | ref | Reference shows `avgFeeRate: 0` and a `feeRange` floor of 0.12: whole-sat rounding. We compute our own |
| `GET /v1/block/:hash` | One block with `extras` | As above | planned | M3 | ref | Reference leaves `utxoSetSize`, `totalInputAmt`, `matchRate` null |
| `GET /v1/block/:hash/summary` | Every tx in the block: txid, fee, vsize, value, rate, flags | rbitcoin `getblock` verbosity 2; store | planned | M2 | ref | Feeds the mined-block view. 1.2 MB for a full block. `getblock` v2 has per-tx fees: 0.8 s and 11 MB per full block over TCP |
| `GET /v1/block/:hash/tx/:txid/summary` | One tx's summary row in a block | As above | planned | M2 | ref | |
| `GET /v1/block/:hash/audit-summary` | Expected template against the mined block: added, missing, fresh, sigop, prioritized txs, match rate | Engine at block time; store | served | M3 | pub | 404 `Audit not available` on the reference. Our categories, no `template`, and only the lists we can detect ([`API.md`](../API.md#block-audits)) |
| `GET /v1/block/:hash/tx/:txid/audit` | Whether one tx was expected, seen, added, conflicting, prioritized | Store | served | M3 | pub | `prioritized` and `accelerated` always `false`; a txid in none of our lists reads as matched |
| `GET /v1/mining/blocks/timestamp/:timestamp` | The newest block stamped at or before a Unix time | Store | served | M3 | ref | Not the nearest: the reference answered the block 252 s before rather than the one 17 s after. `404` before the stored history ([`API.md`](../API.md#mining)) |
| `GET /v1/blocks/tip/height` | Tip height | rbitcoin Esplora | planned | M5 | ref | An alias of `/api/blocks/tip/height` |
| `GET /v1/blocks-bulk/:min[/:max]` | Many blocks with extras | — | skipped | — | ref, pub | Disabled on the reference and on mempool.space. Revisit if asked for |

## Mining

Every mining route is served from the block table in our store (MI1, MI2), never from the node. The reference backend took 14–54 s for most of these; ours answer in under 2 ms once computed, and in under a second cold on a mainnet-sized table ([`API.md`](../API.md#mining)). The audit routes are served from the audits table (MI4), for the blocks we watched arrive ([`API.md`](../API.md#block-audits)).

| Endpoint | Returns | Source | Status | M | Live | Note |
|---|---|---|---|---|---|---|
| `GET /v1/mining/pools` | Every known pool: name, slug, id | Pool list (bitcoin-data/mining-pools, MIT) | served | M3 | ref | Ids and some slugs differ from the reference's own list (`oceanxyz` for `ocean`, `btccom` for `cloverpool`) |
| `GET /v1/mining/pools/:interval` | Pool ranking for a period, block count, estimated hashrates | Store | served | M3 | ref | 24 s on the reference. Same 895 blocks over a week; 891 attributed alike ([`API.md`](../API.md#mining)) |
| `GET /v1/mining/pool/:slug` | One pool's share, blocks, hashrate, reward, block health | Store | served | M3 | ref | 54 s on the reference. Block health is `null` until audits |
| `GET /v1/mining/pool/:slug/hashrate` | One pool's hashrate share over time | Store | served | M3 | ref | Weekly |
| `GET /v1/mining/pool/:slug/blocks[/:height]` | One pool's blocks, with extras, paged by height | Store | served | M3 | ref | 100 a page, strictly below `:height`. Header and per-transaction fields the table lacks are left out |
| `GET /v1/mining/hashrate[/:interval]` | Network hashrate and difficulty over time | Store | served | M3 | ref | Our own estimate; within 0.01 % of the node's `getnetworkhashps` over the same blocks. A 7-day window per daily point |
| `GET /v1/mining/hashrate/pools[/:interval]` | Hashrate share per pool over time | Store | served | M3 | ref | Weekly |
| `GET /v1/mining/difficulty-adjustments[/:interval]` | Past retargets: time, height, difficulty, change | Store | served | M3 | ref | Identical to the reference for the month compared |
| `GET /v1/mining/reward-stats/:block_count` | Total reward, fees and tx count over the last N blocks | Store | served | M3 | ref | N from 1 to 52,560 |
| `GET /v1/mining/blocks/fees/:interval` | Average block fees over time, with USD | Store | served | M3 | ref | No `USD` until prices exist (M3), as the reference answers on a network without prices |
| `GET /v1/mining/blocks/rewards/:interval` | Average block reward over time, with USD | Store | served | M3 | ref | No `USD`, as above |
| `GET /v1/mining/blocks/fee-rates/:interval` | Fee-rate percentiles per block over time | Store | served | M3 | ref | Our own percentiles rounded to whole sat/vB; only points whose every block is rated |
| `GET /v1/mining/blocks/fee-rates/:interval/precise` | The same percentiles with sub-sat precision | Store | served | M3 | pub | 404 on the reference (older release). The useful form on rbitcoin's sub-sat mempool. Three decimals, as mempool.space |
| `GET /v1/mining/blocks/sizes-weights/:interval` | Block size and weight over time | Store | served | M3 | ref | |
| `GET /v1/mining/blocks/predictions/:interval` | Audit match rate per block over time | Store (audits) | served | M3 | ref, pub | Empty on the reference (no audits). Only blocks we watched arrive |
| `GET /v1/mining/blocks/audit/score/:hash` | One block's match rate, expected fees and weight | Store (audits) | served | M3 | ref, pub | `404` without an audit, where the public API answers `"null"` |
| `GET /v1/mining/blocks/audit/scores[/:height]` | Match rates for a page of blocks | Store (audits) | served | M3 | ref, pub | 16 heights ending at `:height`, lowest first |
| `GET /v1/mining/blocks/audit/:hash` | Per-block audit detail | Store (audits) | served | M3 | ref, pub | 204 with no body on the reference; on the public API the same body as `audit-summary`, and so here |

## Lightning

The reference backend runs with Lightning off, so shapes here were observed on mempool.space's public API. All come from our gossip peer and Lightning store (LN1–LN4).

| Endpoint | Returns | Source | Status | M | Live | Note |
|---|---|---|---|---|---|---|
| `GET /v1/lightning/statistics/latest` | Network totals: nodes by network type, channels, capacity, fee-rate and base-fee averages and medians | Lightning store | planned | M4 | pub | |
| `GET /v1/lightning/statistics/:interval` | Network totals over time | Lightning store | planned | M4 | pub | History starts when our peer starts. Empty on the public site at probe time, whose `latest` was dated 2026-08-30 |
| `GET /v1/lightning/search?searchText=` | Matching nodes and channels | Lightning store | planned | M4 | pub | Alias substring and exact pubkey or channel id |
| `GET /v1/lightning/nodes/rankings` | Top nodes by capacity and by channel count | Lightning store | planned | M4 | pub | |
| `GET /v1/lightning/nodes/rankings/liquidity` | Top 100 by capacity | Lightning store | planned | M4 | pub | Location columns come back empty without GeoIP |
| `GET /v1/lightning/nodes/rankings/connectivity` | Top 100 by channels | Lightning store | planned | M4 | pub | As above |
| `GET /v1/lightning/nodes/rankings/age` | Oldest 100 nodes | Lightning store | planned | M4 | pub | "First seen" is bounded by our own history |
| `GET /v1/lightning/nodes/:pubkey` | One node: alias, color, sockets, features, capacity, channel counts | Lightning store | planned | M4 | pub | Location fields (`city`, `country`, `as_number`) are GeoIP and will be absent |
| `GET /v1/lightning/nodes/:pubkey/statistics` | One node's capacity and channels over time | Lightning store | planned | M4 | pub | |
| `GET /v1/lightning/nodes/:pubkey/fees/histogram` | Distribution of one node's incoming and outgoing fee rates | Lightning store | planned | M4 | pub | |
| `GET /v1/lightning/channels?public_key=&status=` | A node's channels, open or closed, paged | Lightning store | planned | M4 | pub | |
| `GET /v1/lightning/channels/:short_id` | One channel: both ends, policies, capacity, funding and closing tx | Lightning store; rbitcoin position routes | planned | M4 | pub | |
| `GET /v1/lightning/channels/txids?txId[]=` | Channels opened or closed by given transactions | Lightning store | planned | M4 | pub | Lets the tx page mark channel opens and closes |
| `GET /v1/lightning/penalties` | Channels closed by a penalty transaction | Lightning store (LN3 close classification) | planned | M4 | pub | |
| `GET /v1/lightning/nodes/countries` | Node counts and capacity per country | — | skipped | — | pub | Needs GeoIP |
| `GET /v1/lightning/nodes/country/:code` | Nodes in a country | — | skipped | — | pub | Needs GeoIP |
| `GET /v1/lightning/nodes/isp-ranking` | Capacity per hosting provider, Tor share | — | skipped | — | pub | Needs an ASN database |
| `GET /v1/lightning/nodes/isp/:asn` | Nodes at one provider | — | skipped | — | pub | Needs an ASN database |
| `GET /v1/lightning/nodes/world` | Node coordinates for the world map | — | skipped | — | pub | World map is out of scope |
| `GET /v1/lightning/channels-geo[/:pubkey]` | Channel endpoints with coordinates | — | skipped | — | pub | World map is out of scope |
| `GET /v1/lightning/nodes/group/:name` | A curated group of nodes | — | skipped | — | pub | The groups are mempool.space's own curation |

## Paid services and Liquid

| Endpoint | Returns | Status | Live | Why |
|---|---|---|---|---|
| `/v1/accelerations*`, `/v1/services/*` | Accelerator state and quotes | skipped | ref | Out of scope. The reference answers 400 "not available" |
| `/v1/assets/*` | Liquid assets | skipped | ref | Out of scope |
