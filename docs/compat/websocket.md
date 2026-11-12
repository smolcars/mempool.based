# WebSocket

Part of the parity inventory in [`../COMPAT.md`](../COMPAT.md), which defines the columns, status values and the `Live` legend.

mempool.space serves one JSON WebSocket at `/api/v1/ws`. rbitcoin has none (`/ws` and `/v1/ws` are 404 on the node). Our frontend uses `/app/ws`: JSON control messages plus binary projected-block deltas ([`ARCHITECTURE.md`](../ARCHITECTURE.md)). The rows below describe the mempool.space-shaped messages; `planned` means we send them at `/api/v1/ws` from the same hub, and the milestone is when the data behind them exists. Whether to serve `/api/v1/ws` at all is a proposal in the findings.

Observed on the reference: after a `want`, one frame about every 2.4 seconds carries `mempoolInfo`, `vBytesPerSecond`, `mempool-blocks`, `fees`, `da` and usually `transactions` together. `live-2h-chart` arrives once a minute. A malformed or unknown message gets no answer and the socket stays open.

## Client to server

| Message | Effect | Source | Status | M | Live | Note |
|---|---|---|---|---|---|---|
| `{"action":"init"}` | One snapshot: the same keys as `GET /api/v1/init-data` | Engine | planned | M2 | ref | Blocks and fees alone are possible in M1 |
| `{"action":"want","data":[…]}` | Subscribes to topics: `blocks`, `stats`, `mempool-blocks`, `live-2h-chart`, `watch-mempool` | Hub topics | planned | M2 | ref | `blocks` alone in M1 |
| `{"action":"ping"}` | Answered with `pong` | Hub | planned | M1 | ref | |
| `{"refresh-blocks":true}` | Resends the recent block list | Esplora `/blocks`, store for extras | planned | M1 | ref | |
| `{"track-tx":"<txid>"}` | Follows one transaction: its projected position, then confirmation | Engine; Electrum outpoint and header pushes | planned | M2 | ref | MP5 |
| `{"track-txs":[…]}` | Follows several transactions | As above | planned | M2 | ref | Cap per connection ([`SECURITY.md`](../SECURITY.md) rule 9) |
| `{"track-address":"<address>"}` | Pushes new mempool and confirmed transactions for one address | Shared Electrum scripthash subscriptions | planned | M2 | ref | rbitcoin pushes per script; no address polling |
| `{"track-addresses":[…]}` | Same for several addresses | As above | planned | M2 | ref | Cap per connection |
| `{"track-scripthashes":[…]}` | Same, by scripthash | As above | planned | M2 | — | Accepted silently on the reference; no traffic in the probe window |
| `{"track-mempool-block":<index>}` | Full transaction list of one projected block, then deltas | Engine (MP2, MP3) | planned | M2 | ref | Ours is binary on `/app/ws`; the JSON form is for compatibility |
| `{"track-rbf":"all"\|"fullRbf"}` | Recent replacement trees, then updates | Engine | planned | M2 | ref | |
| `{"track-rbf-summary":true}` | Short list of recent replacements | Engine | planned | M2 | ref | |
| `{"track-mempool":true}` | Every mempool change: added, removed, mined, replaced, with a sequence | Engine (MP1) | planned | M2 | ref | Full transactions; heavy. Rate-limit or require opt-in |
| `{"track-mempool-txids":true}` | Same changes as txids only | Engine | planned | M2 | ref | Cheap; a good third-party feed |
| `{"track-accelerations":true}` | Accelerator state | — | skipped | — | ref | Out of scope |
| `{"track-asset":…}` | Liquid asset | — | skipped | — | ref | Out of scope; no answer |
| `{"track-donation":…}` | Donation status for mempool.space's sponsor page | — | skipped | — | ref | Not ours; no answer |
| `{"track-wallet":…}` | Not observable: no answer on the reference | — | skipped | — | ref | Revisit if a client is found to depend on it |

## Server to client

| Message | Carries | Source | Status | M | Live | Note |
|---|---|---|---|---|---|---|
| `blocks` | The last 8 blocks with `extras` | Esplora; store for extras | planned | M1 | ref | `extras` filled from M3 |
| `block` | A new block with `extras` | Electrum header push; store | planned | M1 | ref | 50 ms budget from node notification |
| `mempoolInfo` | The node's `getmempoolinfo` | rbitcoin RPC `getmempoolinfo` | planned | M2 | ref | Includes rbitcoin's cluster limits |
| `vBytesPerSecond` | Mempool inflow rate | Engine | planned | M2 | ref | |
| `mempool-blocks` | Projected block summaries | Engine | planned | M2 | ref | |
| `transactions` | The latest few mempool entries | Engine | planned | M2 | ref | |
| `fees` | Recommended fees, whole sat/vB | Engine | planned | M2 | ref | Our `/app/ws` sends sub-sat values |
| `da` | Difficulty adjustment progress | Engine | planned | M3 | ref | |
| `conversions` | Fiat prices | Store | planned | M3 | ref | |
| `rbfSummary` | Recent replacements, in `init` | Engine | planned | M2 | ref | |
| `live-2h-chart` | One mempool statistics sample a minute | Engine; store | planned | M3 | ref | |
| `loadingIndicators` | Progress of backend indexing jobs | Store backfill progress | different | M3 | ref | Ours reports backfill progress under our own keys on `/app/ws` |
| `backend`, `backendInfo` | Backend kind and build | Server build info | different | M5 | ref | No host name or OS |
| `txPosition` | A tracked transaction's projected block, position and CPFP data | Engine | planned | M2 | ref | |
| `txConfirmed` | The txid of a tracked transaction that confirmed | Engine; Electrum | planned | M2 | ref | |
| `tracked-txs` | Updates for several tracked transactions | Engine | planned | M2 | ref | |
| `txReplaced`, `rbfTransaction`, `rbfInfo` | A tracked transaction was replaced, and by what | Engine | planned | M2 | — | Not seen in the probe window |
| `utxoSpent` | An output of a tracked transaction was spent | Electrum outpoint push | planned | M2 | — | Not seen in the probe window |
| `address-transactions` | New mempool transactions for a tracked address, with CPFP fields | Electrum scripthash push; Esplora | planned | M2 | ref | |
| `block-transactions` | Confirmed transactions for a tracked address in a new block | As above | planned | M2 | ref | |
| `multi-address-transactions` | Per-address mempool, confirmed and removed transactions | As above | planned | M2 | ref | |
| `address-removed-transactions` | Transactions dropped for a tracked address | Engine removal reasons | planned | M2 | — | Not seen in the probe window |
| `projected-block-transactions` | Full list, then `delta` with added, removed and changed | Engine | planned | M2 | ref | Binary on `/app/ws` |
| `rbfLatest` | Replacement trees | Engine | planned | M2 | ref | |
| `rbfLatestSummary` | Short replacement list | Engine | planned | M2 | ref | |
| `mempool-transactions` | Mempool changes as full transactions, with `sequence` | Engine | planned | M2 | ref | |
| `mempool-txids` | Mempool changes as txids, with `sequence` | Engine | planned | M2 | ref | Our poll already carries `getrawmempool`'s `mempool_sequence` |
| `accelerations` | Accelerator state | — | skipped | — | ref | Out of scope |
| `pong` | Reply to `ping` | Hub | planned | M1 | ref | |
