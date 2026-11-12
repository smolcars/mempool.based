# WebSocket

Part of the parity inventory in [`../COMPAT.md`](../COMPAT.md), which defines the columns, status values and the `Live` legend.

mempool.space serves one JSON WebSocket at `/api/v1/ws`. rbitcoin has none (`/ws` and `/v1/ws` are 404 on the node). Our frontend uses `/app/ws`: JSON control messages plus binary projected-block deltas ([`ARCHITECTURE.md`](../ARCHITECTURE.md)). The rows below describe the mempool.space-shaped messages. We serve the `served` ones at `/api/v1/ws` from the same hub; what a client sends and receives there, and every difference, is [`WS.md`](../WS.md#the-mempoolspace-shaped-socket-apiv1ws).

Observed on the reference: after a `want`, one frame about every 2.4 seconds carries `mempoolInfo`, `vBytesPerSecond`, `mempool-blocks`, `fees`, `da` and usually `transactions` together. `live-2h-chart` arrives once a minute. A malformed or unknown message gets no answer and the socket stays open.

Observed on mempool.space's public socket on 2026-11-12 (mainnet, two connections over about 20 minutes and two blocks, read-only, following public data). Trimmed copies of the messages are in [`fixtures/websocket/reference.json`](../../fixtures/websocket/reference.json), which a test holds our messages against:

- `{"action":"ping"}` is answered `{"pong":true}`.
- `want` replaces the wanted set: after `want ["stats"]` the `mempool-blocks` stopped. The newly wanted parts' state comes at once, in one frame; then about one frame a second.
- Text that is not JSON closes the socket (close status 1005). JSON it does not understand, an invalid txid or address, and a wrong type (`"data":"blocks"`) get no answer, and the socket stays.
- `fees` on the socket has decimals (`2.533`), unlike `/api/v1/fees/recommended`.
- `track-tx` is answered at once with `{"txPosition":{"txid","position":{"block","vsize"}}}`, where `vsize` is what is mined before it inside its block; one later frame also carried `cpfp`. The position then rides in every stats frame. `{"track-tx":"stop"}` ends it. A replaced txid tracked after the fact got no answer.
- `track-address` and `track-addresses` get no answer until something happens. More than 10 addresses in `track-addresses` is answered `{"track-addresses-error": …}`.
- On a block, one frame carried `block`, `mempoolInfo`, `vBytesPerSecond`, `fees`, `da`, `mempool-blocks`, `txConfirmed` (the tracked txid, a string) and `block-transactions` (the followed address's confirmed transaction: Esplora JSON with the reference's own fields added).
- A connection that had sent only `track-tx` and `track-addresses`, and no `want`, was dropped after 226 s without a close frame.

## Client to server

| Message | Effect | Source | Status | M | Live | Note |
|---|---|---|---|---|---|---|
| `{"action":"init"}` | One snapshot: the same keys as `GET /api/v1/init-data` | Engine | skipped | — | ref | No answer. The `want` snapshots carry the same parts |
| `{"action":"want","data":[…]}` | Subscribes to topics: `blocks`, `stats`, `mempool-blocks`, `live-2h-chart`, `watch-mempool` | Hub topics | served | M2 | ref | `blocks`, `stats`, `mempool-blocks`; other names are ignored ([`WS.md`](../WS.md#the-mempoolspace-shaped-socket-apiv1ws)) |
| `{"action":"ping"}` | Answered with `pong` | Hub | served | M2 | ref |  |
| `{"refresh-blocks":true}` | Resends the recent block list | Esplora `/blocks`, store for extras | skipped | — | ref | No answer. Want `blocks` again |
| `{"track-tx":"<txid>"}` | Follows one transaction: its projected position, then confirmation | Engine; Electrum outpoint and header pushes | served | M2 | ref | One per socket; `"stop"` ends it |
| `{"track-txs":[…]}` | Follows several transactions | As above | skipped | — | ref | Answered `track-txs-error` |
| `{"track-address":"<address>"}` | Pushes new mempool and confirmed transactions for one address | Shared Electrum scripthash subscriptions | served | M2 | ref | One per socket; `"stop"` ends it. One history page per change of the address, shared by every socket |
| `{"track-addresses":[…]}` | Same for several addresses | As above | served | M2 | ref | At most `WS_MAX_TRACKED_ADDRESSES` (2), with `track-address`; the reference allows 10. `[]` stops |
| `{"track-scripthashes":[…]}` | Same, by scripthash | As above | skipped | — | — | Answered `track-scripthashes-error` |
| `{"track-mempool-block":<index>}` | Full transaction list of one projected block, then deltas | Engine (MP2, MP3) | different | — | ref | Binary on `/app/ws`; answered `track-mempool-block-error` here |
| `{"track-rbf":"all"\|"fullRbf"}` | Recent replacement trees, then updates | Engine | skipped | — | ref | Answered `track-rbf-error` |
| `{"track-rbf-summary":true}` | Short list of recent replacements | Engine | skipped | — | ref | Answered `track-rbf-summary-error` |
| `{"track-mempool":true}` | Every mempool change: added, removed, mined, replaced, with a sequence | Engine (MP1) | skipped | — | ref | Every transaction to every client: answered `track-mempool-error` |
| `{"track-mempool-txids":true}` | Same changes as txids only | Engine | skipped | — | ref | Answered `track-mempool-txids-error`. A bounded form is proposed in the findings below |
| `{"track-accelerations":true}` | Accelerator state | — | skipped | — | ref | Out of scope; answered `track-accelerations-error` |
| `{"track-asset":…}` | Liquid asset | — | skipped | — | ref | Out of scope; no answer |
| `{"track-donation":…}` | Donation status for mempool.space's sponsor page | — | skipped | — | ref | Not ours; no answer |
| `{"track-wallet":…}` | Not observable: no answer on the reference | — | skipped | — | ref | Revisit if a client is found to depend on it |

## Server to client

| Message | Carries | Source | Status | M | Live | Note |
|---|---|---|---|---|---|---|
| `blocks` | The last 8 blocks with `extras` | Esplora; store for extras | served | M2 | ref | The node's Esplora JSON for the last 8, oldest first; no `extras` or `stale` |
| `block` | A new block with `extras` | Electrum header push; store | served | M2 | ref | The node's Esplora JSON, at the same moment as `/app/ws`; no `extras` or `stale` |
| `mempoolInfo` | The node's `getmempoolinfo` | rbitcoin RPC `getmempoolinfo` | served | M2 | ref | The node's `getmempoolinfo` as the tracker last read it |
| `vBytesPerSecond` | Mempool inflow rate | Engine | planned | M3 | ref | Not computed yet |
| `mempool-blocks` | Projected block summaries | Engine | served | M2 | ref | `/api/v1/fees/mempool-blocks`' answer |
| `transactions` | The latest few mempool entries | Engine | served | M2 | ref | `vsize` is the sigop-adjusted integer |
| `fees` | Recommended fees, whole sat/vB | Engine | served | M2 | ref | `/api/v1/fees/precise`' answer: the reference's socket also sends decimals |
| `da` | Difficulty adjustment progress | Engine | served | M2 | ref | `/api/v1/difficulty-adjustment`' answer |
| `conversions` | Fiat prices | Store | planned | M3 | ref | |
| `rbfSummary` | Recent replacements, in `init` | Engine | skipped | — | ref | `init` is not served |
| `live-2h-chart` | One mempool statistics sample a minute | Engine; store | planned | M3 | ref | |
| `loadingIndicators` | Progress of backend indexing jobs | Store backfill progress | different | M3 | ref | Ours reports backfill progress under our own keys on `/app/ws` |
| `backend`, `backendInfo` | Backend kind and build | Server build info | different | M5 | ref | No host name or OS |
| `txPosition` | A tracked transaction's projected block, position and CPFP data | Engine | served | M2 | ref | `position.block` and `position.vsize` (inside the block); no `cpfp` |
| `txConfirmed` | The txid of a tracked transaction that confirmed | Engine; Electrum | served | M2 | ref | The txid, as on the reference, in its own message |
| `tracked-txs` | Updates for several tracked transactions | Engine | skipped | — | ref | `track-txs` is not served |
| `txReplaced` | A tracked transaction was replaced, and by what | Engine | served | M2 | — | `{"txReplaced":{"txid":"<replacement>"}}`: not seen on the reference in the probe windows |
| `rbfTransaction`, `rbfInfo` | The replacement's details | Engine | skipped | — | — | Not seen in the probe windows |
| `utxoSpent` | An output of a tracked transaction was spent | Electrum outpoint push | skipped | — | — |  |
| `address-transactions` | New mempool transactions for a tracked address, with CPFP fields | Electrum scripthash push; Esplora | served | M2 | ref | The node's Esplora JSON, without the reference's own fields |
| `block-transactions` | Confirmed transactions for a tracked address in a new block | As above | served | M2 | ref | As above |
| `multi-address-transactions` | Per-address mempool, confirmed and removed transactions | As above | served | M2 | ref | One address per message; `removed` is always empty |
| `address-removed-transactions` | Transactions dropped for a tracked address | Engine removal reasons | skipped | — | — | A transaction that leaves without confirming is not said |
| `projected-block-transactions` | Full list, then `delta` with added, removed and changed | Engine | different | — | ref | Binary on `/app/ws` |
| `rbfLatest` | Replacement trees | Engine | skipped | — | ref |  |
| `rbfLatestSummary` | Short replacement list | Engine | skipped | — | ref |  |
| `mempool-transactions` | Mempool changes as full transactions, with `sequence` | Engine | skipped | — | ref | `track-mempool` is not served |
| `mempool-txids` | Mempool changes as txids, with `sequence` | Engine | skipped | — | ref | `track-mempool-txids` is not served |
| `accelerations` | Accelerator state | — | skipped | — | ref | Out of scope |
| `pong` | Reply to `ping` | Hub | served | M2 | ref | `{"pong":true}` |
