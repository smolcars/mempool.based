# Parity with mempool.space

This page tracks every mempool.space feature: whether we serve it, how, and if not, why.

## Policy

1. **Feature parity is the goal; being a drop-in is not.** Where rbitcoin lets us do something better, we do it our way and note the difference here.
2. **Esplora is served as rbitcoin serves it,** under `/api`. Clients written for Esplora or electrs work unchanged.
3. **mempool.space-shaped endpoints under `/api/v1` are served when they are cheap** to derive from what we already compute. Each one is a row in the inventory.
4. **Our own frontend uses `/app`,** a private API with no compatibility promise.
5. **This inventory is written clean-room.** It describes observable behaviour in our own words. No mempool.space source code is read, and no text or example payload is copied from its documentation.

## Out of scope

| Feature | Why |
|---|---|
| Liquid and its assets | Different chain; rbitcoin does not serve it |
| Accelerator and other paid services | A commercial service, not explorer functionality |
| Translations | English only for now |
| Lightning world map | Needs a GeoIP database |
| Address-prefix search | rbitcoin has no prefix index; search is exact match |

## Status values

| Value | Meaning |
|---|---|
| `served` | Works today |
| `planned` | Assigned to a milestone |
| `different` | We offer the feature another way; the row says how |
| `skipped` | Not planned; the row says why |

## Inventory

One table per group, each on its own page:

| Group | Page | Rows | served | planned | different | skipped | Confirmed live |
|---|---|---|---|---|---|---|---|
| Esplora REST (`/api`) | [`compat/esplora.md`](compat/esplora.md) | 56 | 0 | 50 | 0 | 6 | 48 |
| mempool.space REST (`/api/v1`) | [`compat/api-v1.md`](compat/api-v1.md) | 73 | 16 | 40 | 4 | 13 | 72 |
| WebSocket | [`compat/websocket.md`](compat/websocket.md) | 47 | 0 | 40 | 2 | 5 | 43 |
| Pages and visible features | [`compat/pages.md`](compat/pages.md) | 46 | 0 | 38 | 2 | 6 | 29 |
| **Total** | | **222** | **16** | **168** | **8** | **30** | **192** |

Each row gives the feature, what it returns, where our answer comes from (an rbitcoin Esplora route or RPC method, our engine, our store, or Lightning), its status, its milestone from [`PLAN.md`](PLAN.md) and how it was checked.

### The `Live` column

| Value | Meaning |
|---|---|
| `node` | Requested from a synced rbitcoin v0.9.1 node (mainnet; signet and testnet4 spot-checked) |
| `ref` | Requested from the reference backend: a stock mempool backend (v3.3.2, Esplora mode, Lightning off) in front of that node. For pages, rendered from its frontend in headless Chromium |
| `pub` | Requested from mempool.space's public API, used where the reference has the feature off (Lightning, audits) |
| `doc` | Not requested: from rbitcoin's v0.9.1 documentation. Writes and very large responses are in this group |
| `—` | Not confirmed: known from the public site, but not observed during the probe window |

All requests were read-only and sequential. Three `/api/v1` rows (`POST /v1/cpfp`, `/v1/psbt/addparents`, `/v1/txs/package`) confirm only that the route exists: each handler refused a malformed body. A body that is not JSON is refused before routing on the reference, so such a refusal proves nothing; `POST /v1/tx/push` is unconfirmed for that reason.

## Reference backend against rbitcoin's Esplora

What differs between the two answers to the same question, from live requests on mainnet.

1. **Esplora shapes match exactly.** On 12 routes compared key by key (block, block list, block status, tx, tx status, outspends, merkle proof, address, address summary, mempool, recent, fee estimates), rbitcoin's JSON and mempool.space's public Esplora JSON had the same fields. The pass-through needs no reshaping.
2. **The reference stack serves no Esplora under `/api/v1`.** `/api/v1/tx/:txid` and `/api/v1/address/:address` are 404; Esplora lives only under `/api`. Only `/api/v1/blocks/tip/height` is duplicated.
3. **Fee estimates come from different products.** rbitcoin's `/fee-estimates` is a 10-minute inclusion estimate with sub-sat precision. The reference derives its own from projected blocks: `recommended` rounds to whole sat/vB with a floor of 1, while `precise` gave 2.633 / 1.38 / 0.628 / 0.2 / 0.1 at the same moment. The mempool floor was 0.1 sat/vB.
4. **The reference's block extras are degraded on rbitcoin.** `avgFeeRate` is 0 and `feeRange` starts at 0.12, from whole-sat rounding. `utxoSetSize` and `totalInputAmt` are null; rbitcoin has no `gettxoutsetinfo`, so the UTXO set size cannot come from the node.
5. **The reference has no block audits.** `audit-summary` is 404 "Audit not available", `matchRate` is null and `predictions` is empty. mempool.space's public API has all three.
6. **The reference's mining routes are slow.** Most took 15–58 s per request; two unrecognised paths under `/api/v1/mining/` hung until the proxy's 60 s timeout. Block, fee and mempool routes answered in under 0.1 s.
7. **`mempoolInfo` carries rbitcoin's cluster limits** (`limitclustercount`, `limitclustersize`) and `fullrbf: true`, passed straight through from `getmempoolinfo`.
8. **Only rbitcoin offers** the chain-tip headers, `?asof=`, the output-by-position routes and multi-address `POST` history. Address totals and UTXOs answer 503 above the node's `--max-sh-creates` cap (a mainnet payout address with 16,276 outputs), while its paged history still works.
9. **Networks differ by operator settings, not by software.** All three test nodes serve address routes; the signet node's cap is high enough to total an address with 118,307 outputs.

## Findings for the plan

Proposals for [`PLAN.md`](PLAN.md) and [`TODO.md`](TODO.md); this task does not edit them.

### Cheaper than expected

1. **CPFP needs no graph of our own.** `getmempoolcluster` returned a transaction's cluster and chunk fee rate in 20 ms, and `getmempoolancestors` 10 ms. The cluster call lists chunks without edges, so relationships come from `getmempoolancestors`, `getmempooldescendants` or `getmempoolentry`'s `depends`; the effective fee rate is the node's own chunk rate.
2. **The Esplora pass-through is pure routing.** Shapes match (above), so S2 is an allow-list, typed parameters and cache headers. No response rewriting.
3. **Difficulty adjustment is cheap** (two headers and arithmetic). It could move from M3 to M1 or M2 so the home page has it early.
4. **`getblockstats` gives totals in 40 ms,** including `utxo_increase` for the UTXO-change extra. Its fee-rate percentiles are truncated to whole sat/vB (they read `[0,0,0,0,2]` for the probed block), so they cannot feed any fee-rate history.

### More expensive than expected

5. **Per-transaction block data needs `getblock` verbosity 2.** The mined-block drawing (`/v1/block/:hash/summary`, W7) and our own fee percentiles (MI1) need every transaction's fee and size. Over TCP that is 0.8 s and 11 MB per full block; the paged Esplora route would take about 270 requests. A sequential full-history backfill at that rate takes days. Proposal: take totals from `getblockstats`; backfill percentiles from `getblock` verbosity 2 newest first, in the background, with a few requests in parallel; prefer the unix-socket `/internal/block/:hash/txs` when co-located. Add `getblock` to N3's method set.
6. **Block audits depend on keeping the template.** The reference cannot audit at all. MI4 needs MP2 to retain the last template (about 0.4 s and 4.9 MB from `getblocktemplate`) at each block arrival; audits cannot be backfilled for blocks we did not watch.
7. **Mining pool identification needs a dataset.** Pool names come from coinbase tags and payout addresses. We need a list under a licence compatible with MIT, or our own. Decide before MI2. Decided in MI2: bitcoin-data/mining-pools, MIT ([`API.md`](API.md#mining)).
8. **`RBITCOIN.md` overstates the chain-tip stamp.** It says every Esplora response carries the tip headers. `/mempool`, `/mempool/txids`, `/mempool/recent`, `/fee-estimates`, broadcast and the address mempool-history routes carry none (the paged `/mempool/txids/page` is stamped), and address routes stamp the address-index watermark ([`compat/esplora.md`](compat/esplora.md)). N1's tip handling should follow the narrower rule; the owner of `RBITCOIN.md` should correct it.
9. **Large addresses need a product answer.** Totals and UTXOs are refused above the node's cap. W5 must show history without totals and say why. A paged or capped stats route is a candidate for the rbitcoin wishlist in [`RBITCOIN.md`](RBITCOIN.md).

### Cannot be supported, or not worth it

10. **UTXO set size** (`utxoSetSize` in block extras): rbitcoin has no `gettxoutsetinfo`.
11. **Lightning location features** (countries, ISPs, world and channel maps, node groups): nine rows skipped for GeoIP, ASN or curation. Lightning node responses will lack location fields.
12. **`blocks-bulk`** is disabled on both the reference and mempool.space; skip until someone asks.

### Third-party dependence

13. **A compatible WebSocket is worth a task.** [`ARCHITECTURE.md`](ARCHITECTURE.md) defines only `/app/ws`. Wallets and bots that follow transactions or addresses speak `/api/v1/ws` (`want`, `track-tx`, `track-address`, `track-addresses`, `ping`). Proposal: a task in M2, after S3 and MP5, that serves this JSON subset from the same hub, with the heavy `track-mempool` topic rate-limited.
14. **Endpoints most likely relied on by others** (from what wallets and tools are publicly known to call, not measured here): Esplora `/api/tx`, `/api/tx/:txid/status`, `/api/address/*`, `/api/blocks/tip/height`; `/api/v1/fees/recommended`, `/api/v1/fees/mempool-blocks`, `/api/v1/prices`, `/api/v1/historical-price`, `/api/v1/difficulty-adjustment`, `/api/v1/cpfp/:txid`. Deliver these first within their milestones.
15. **Serve both fee shapes.** `fees/recommended` keeps its whole-number contract for old clients; on a 0.1 sat/vB mempool it overstates the minimum tenfold, so `fees/precise` and our frontend use sub-sat values.
