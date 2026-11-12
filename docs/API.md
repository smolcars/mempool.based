# Public API

The server serves rbitcoin's Esplora REST API under `/api`: the same routes, status codes, body bytes and content types as the node, behind an allow-list of parsed routes ([`SECURITY.md`](SECURITY.md) rules 1–3). Wallets and tools that speak Esplora can point at it unchanged. This page lists the routes and every way we differ from the node. The code is `crates/server/src/esplora.rs`.

| Network | Base URL |
|---|---|
| Mainnet | `/api` |
| Signet | `/signet/api` |
| Testnet4 | `/testnet4/api` |
| Regtest | `/regtest/api` |

A disabled network's base is a `404`.

## The contract

Request and response shapes are rbitcoin v0.9.1's: [`docs/esplora-openapi.yaml`](https://github.com/rbitcoin-org/rbitcoin/blob/v0.9.1/docs/esplora-openapi.yaml) and the "Esplora REST surface" section of its [`COMPAT.md`](https://github.com/rbitcoin-org/rbitcoin/blob/v0.9.1/COMPAT.md#esplora-rest-surface). Address and scripthash routes need the node's `--sh-index`.

## Routes

`asof` marks the routes that take `?asof=<blockhash>`.

| Group | Routes |
|---|---|
| Tip | `GET /blocks/tip/height`, `GET /blocks/tip/hash` |
| Blocks | `GET /blocks`, `GET /blocks/:start_height`, `GET /block-height/:height` |
| Block | `GET /block/:hash`, `/header`, `/status`, `/raw`, `/txids`, `/txid/:index`, `/txs`, `/txs/:start_index` |
| Transaction | `GET /tx/:txid`, `/hex`, `/raw`, `/status` (asof), `/merkle-proof`, `/merkleblock-proof`, `/outspend/:vout` (asof), `/outspends` (asof); `GET /txs/outspends?txids=` |
| Output by position | `GET /block-height/:height/tx/:index` (asof), `GET /block-height/:height/tx/:index/out/:vout` (asof), `GET /tx/:txid/out/:vout` (asof), `POST /block-heights/txs/outs` (asof) |
| Address and scripthash | `GET /address/:address` and `GET /scripthash/:hash`, each bare (asof), `/utxo` (asof), `/txs` (asof, `?after_txid=`), `/txs/chain[/:last_seen_txid]` (asof), `/txs/mempool`, `/txs/summary[/:last_seen_txid]` (asof, `?after_txid=` without the path cursor) |
| Mempool | `GET /mempool`, `/mempool/txids`, `/mempool/txids/page[/:last_txid]` (`?max_txs=`), `/mempool/recent` |
| Fees | `GET /fee-estimates` |
| Broadcast | `POST /tx` |

`HEAD` is accepted wherever `GET` is. Any other path under `/api` is a `404`, and a known path with the wrong method is a `405` with `Allow`.

### Not served

| Route | Why |
|---|---|
| `/internal/*` | Bulk routes for the server's own use. They exist only on the node's unix socket and are never reachable from outside |
| `POST /addresses/txs`, `POST /scripthashes/txs` and their `/summary` | One request can ask for up to 300 address-index joins. That is unbounded load for one client to cause (rule 11). Ask per address instead |
| `POST /txs/test`, `POST /txs/package` | Broadcast is the only write we serve (rule 5). Package relay and test-accept may come later with their own limits |
| `GET /block-template` | Off on the node by default, about 5 MB, and the server uses RPC for templates |
| `/blindbit/v1/*` | Silent-payments oracle; not part of the Esplora surface |

## Differences from the node

### Parameters are parsed strictly

Every parameter is parsed into a typed value before the node is asked, and the node is sent a path we wrote from that value. A request that does not parse never reaches the node.

| Input | Node | Here |
|---|---|---|
| Txid or block hash | 64 hex digits, optional `0x`; a bad one is `404` | Exactly 64 hex digits, any case; a bad one is `400` |
| Height, index, vout, `max_txs` | integer | Decimal digits only, at most `u32::MAX`; anything else is `400` |
| Address | Parsed by the node; a bad or wrong-network one is `404` | Parsed for the network in the URL: `400 invalid address` or `400 address is for another network` |
| `?asof=` on a route that does not take it, any other unknown query parameter, or one given twice | `404` for `asof`; others ignored | `400` |
| `/txs/outspends?txids=` | Up to 50; a token that is not a hash gets an empty slot | Up to 50; any token that is not a txid is `400`. `%2C` is accepted as the comma |
| `/block/:hash/txs/:start_index` not a multiple of 25 | `400` | `400`, before asking the node |
| `POST /block-heights/txs/outs` | More than 300 is `422` | Same, and the body is rebuilt from the parsed positions; it must fit in `MAX_BODY_BYTES` |
| Trailing or doubled slash | `404` | `404` |

The node is sent canonical forms: lowercase hex, lowercase bech32 addresses, `/block/:hash/txs` as `/txs/0`, and `/txs/summary?after_txid=X` as `/txs/summary/X`. The node answers each pair the same.

### Error bodies are ours

A successful answer is forwarded byte for byte. Any other answer is replaced by `{"error": "<message>"}` with `Content-Type: application/json`, because the node's error text can name operator flags and internal paths (rule 17). Errors carry no chain-tip headers.

| Cause | Status | Message |
|---|---|---|
| Unknown object, or an `asof` block not on the best chain | `404` | `not found` |
| Bad parameter | `400` | names the parameter, for example `invalid txid` |
| Cursor txid not in that history | `422` | `after_txid not found` |
| Node runs without `--sh-index` | `503` | `address index is disabled on this server` |
| Unpaged address answer too large (`--max-sh-creates`) | `503` | `address history too large for this route; use the paged /txs routes` |
| Tip moved under a pinned read | `503` | `chain tip moved during the request; retry` |
| Node answered `503` for another reason (no tip, no fee estimates yet) | `503` | `node unavailable` |
| Node unreachable, failed, or answered nonsense | `502` | `upstream unavailable` |
| Node too slow | `504` | `request timed out` |

`503` and `429` carry `Retry-After`.

### Headers

- `X-Bitcoin-Chain-Tip` and `X-Bitcoin-Chain-Tip-Height` are copied from the node's answer when it has them.
- `Content-Type` is the node's. No other node header is copied (`X-Powered-By` is dropped).
- **CORS:** every `/api` response carries `Access-Control-Allow-Origin: *` and exposes the two chain-tip headers, including refusals (`429`, `503`, `504`), so a web client can read why it was refused. A preflight `OPTIONS` is answered with `204`, methods `GET, HEAD, POST`, header `Content-Type`, cached for a day. Broadcast is included: it needs no credentials and no cookie ever applies, so a cross-origin page can do nothing a plain `curl` cannot, and refusing CORS would only break browser wallets.

### Cache headers

A wrong `immutable` cannot be recalled from a cache, so it is used only where the URL's hash fixes the whole answer and the answer says nothing about confirmation.

| Routes | `Cache-Control` |
|---|---|
| `/block/:hash`, `/header`, `/raw`, `/txids`, `/txid/:index` | `public, max-age=31536000, immutable` |
| `/tx/:txid/hex` and `/raw` of a transaction **without** witness data | `public, max-age=31536000, immutable` |
| `/tx/:txid/hex` and `/raw` of a segwit transaction | `public, max-age=1`. A txid does not commit to the witness, so the bytes can change while unconfirmed or after a reorg |
| Every other successful read | `public, max-age=1` |
| Broadcast, and every error or refusal | `no-store` |

Block JSON has no confirmation status (the node drops a block that leaves the best chain rather than flagging it), so it is immutable. `/block/:hash/status`, `/block/:hash/txs` (each transaction carries a status), `/block-height/:height` and merkle proofs change with a reorg and are not.

## Protection for the node

- **Identical requests share one call** (rule 8). Requests that arrive while the same upstream call is in flight wait for it and get its answer. Once answered, the next request asks the node again. Measured: 300 concurrent identical requests made one upstream call.
- **Hot answers are held for one second:** `/blocks/tip/height`, `/blocks/tip/hash`, `/blocks`, `/mempool`, `/mempool/txids`, `/mempool/recent` and `/fee-estimates`. Their keys are fixed, so this cache holds at most seven answers per network. Measured: 20,303 sequential `/blocks/tip/height` requests in 3 seconds made three upstream calls.
- **Every request** passes the server-wide limits in [`CONFIG.md`](CONFIG.md): body size, timeout, per-client rate and concurrency.

## Broadcast

`POST /tx` with the raw transaction as hex in the body, optionally surrounded by whitespace. The answer is the txid as `text/plain`.

- **Parsed before sending:** the body must decode as exactly one transaction. The node is sent our own serialization of it, never the client's bytes.
- **Size cap:** 400,000 bytes of transaction (800,000 hex characters), the most a standard transaction can be. Larger is `413`.
- **Rate limit,** separate from and stricter than reads: a burst of 5 per client, then one every 10 seconds; past it, `429` with `Retry-After: 10`. Refused bodies count.
- **Rejects** are `400` with a fixed reason: rbitcoin's own reason when it is on our list of known policy and consensus reasons (`min relay fee`, `dust`, `mempool double-spend`, `bad-txns-inputs-duplicate`, …), otherwise `transaction already in the mempool or chain`, `missing or spent inputs`, `script verification failed` or `transaction rejected`.
- **Off switch:** `BROADCAST_ENABLED=false` turns it off for every network; it then answers `403 broadcast is disabled on this server`.

## Using it from the frontend

- Call `/api/...` (mainnet) or `/<network>/api/...` with `fetch`. Same origin, so CORS does not apply.
- Treat any non-2xx as `{"error": string}` JSON. The message is safe to show; the status says what to do: `404` not found, `400` bad input, `503` retry later (or, for address pages, the index is off).
- Tip and mempool answers are at most a second old by design; poll no faster than that. For live data use the WebSocket when it exists.
- Read `X-Bitcoin-Chain-Tip` to detect a tip change between two fetches of one page, and refetch the page.
- Response bodies are chain data: render them as text, never as HTML (rule 14).
