# Configuration

The server reads its configuration from the environment once, at startup. Invalid or half-set configuration stops it with a message naming the variable; values are never echoed, so a mistyped password does not land in a log. The error is written as a `config_invalid` event ([`LOGGING.md`](LOGGING.md)). An empty value counts as unset.

[`.env.example`](../.env.example) is a starting point. The server does not read `.env` itself; load it into the environment (`set -a; . ./.env; set +a`) or pass the variables through your process manager.

## Server

| Variable | Default | What it does |
|---|---|---|
| `LISTEN_ADDR` | `127.0.0.1:8080` | Address and port to listen on. Port `0` picks a free one; the `listening` event shows which |
| `WEB_DIR` | `web/dist` | The built frontend. Nothing outside it is served |
| `DATA_DIR` | unset | Where each network's SQLite file lives (`<DATA_DIR>/mainnet.sqlite`, …). Created if missing; must be writable. It holds only derived and observed data ([`STORE.md`](STORE.md)). Unset, the server runs without a store: everything is asked of the node, and the block table below and the mining routes ([`API.md`](API.md#mining)) do not exist. A file that will not open is a `store_open_failed` event, and that network runs without one |

## Networks

A network is enabled when all six of its variables are set, and disabled when none are. Setting only some is an error. `{NET}` is one of `MAINNET`, `SIGNET`, `TESTNET4`, `REGTEST`. Mainnet is served at `/`, the others under `/signet`, `/testnet4` and `/regtest`; a disabled network's paths are a 404.

| Variable | Format |
|---|---|
| `{NET}_RPC_HOST` | Host name or IP address of rbitcoin's JSON-RPC (`--rpc-listen`) |
| `{NET}_RPC_PORT` | Port, 1 to 65535 |
| `{NET}_RPC_USER` | HTTP Basic user |
| `{NET}_RPC_PASSWORD` | HTTP Basic password. Held in a type that prints as `<redacted>` and never leaves the server |
| `{NET}_ESPLORA_URL` | `http://` base URL of rbitcoin's Esplora (`--esplora-listen`), or an absolute unix socket path. The node speaks plain HTTP, so `https://` is refused. Credentials in the URL are refused, since URLs end up in logs |
| `{NET}_ELECTRUM_ADDR` | `host:port` of rbitcoin's Electrum server (`--electrum-listen`), plain TCP |

### The live mempool

| Variable | Default | What it does |
|---|---|---|
| `{NET}_MEMPOOL_ENABLED` | `true` | `false` stops the network's mempool tracker and projected blocks ([`MEMPOOL.md`](MEMPOOL.md#in-the-server)). The mempool topics then answer `off` ([`WS.md`](WS.md#mempool-topics)); blocks, the API and everything else keep working. Optional, unlike the six above |

**Turn it off when you share a node.** With it on, every running server polls its node continuously: on mainnet about 150 MB a minute of RPC answers (one 3.6 MB `getrawmempool` a second), plus an initial sync of about 250 MB and 10 s. Several developers each running a server against the same node multiply that. Leave it on only on the server that serves the mempool, or while you work on it.

## Broadcast

| Variable | Default | What it does |
|---|---|---|
| `BROADCAST_ENABLED` | `true` | `false` turns off `POST /api/tx` on every network (it answers `403`). Broadcast is the only write the server makes to the node; its size cap and per-client rate limit are fixed ([`API.md`](API.md#broadcast)) |

## Logging

What each setting changes, and the event catalogue, are in [`LOGGING.md`](LOGGING.md).

| Variable | Default | What it does |
|---|---|---|
| `LOG_FORMAT` | `json`, or `text` when stdout is a terminal | `json` or `text` |
| `LOG_LEVEL` | `info` | Lowest level written: `debug`, `info`, `warn` or `error`. Events below it are not formatted at all |
| `LOG_REQUESTS` | `all` | Which requests get an `http_request` event: `all`, `failures` (status 400 and above, which includes every refusal) or `off`. A passing `/healthz` is never logged |
| `LOG_CLIENT_ADDR` | `false` | `true` adds the client address, as the rate limiter keys it (IPv4, or an IPv6 /64), to `http_request`. Off by default for privacy; the trade-off is in [`LOGGING.md`](LOGGING.md#why-client-addresses-are-off-by-default) |


## Limits

Every request passes these before any handler runs ([`SECURITY.md`](SECURITY.md) rules 6 and 7). Each must be a positive integer. Refusals are small JSON errors: `429` with `Retry-After: 1` when one client is over its share, `503` when the server as a whole is, `413` for a body that is too large and `504` for a request that ran out of time.

| Variable | Default | Protects against |
|---|---|---|
| `MAX_BODY_BYTES` | `16384` | Memory exhaustion from large request bodies. Enforced when a handler reads the body (axum's body extractors); a route that never reads its body ignores it, and the server discards it unread and closes the connection. A route that needs more raises it for itself; broadcast takes up to 800,000 hex characters ([`API.md`](API.md#broadcast)) |
| `REQUEST_TIMEOUT_SECS` | `10` | Slow handlers and slow uploads holding a slot forever. The same value bounds how long a connection may take to send its request headers or sit idle between requests, so half-sent requests (slowloris) and idle keep-alives are closed, and how long a write to the client may make no progress, so a client that stops reading its responses is disconnected |
| `MAX_CONNECTIONS` | `1024` | Running out of file descriptors. Past it the server stops accepting until a connection closes. Keep it below the process's open-file limit |
| `MAX_CONCURRENT_REQUESTS` | `256` | Overload. Requests past it are refused at once with `503`, not queued |
| `CLIENT_MAX_CONNECTIONS` | `32` | One address holding many open connections. Not applied when `TRUSTED_PROXY_HEADER` is set, because every connection then comes from the proxy |
| `CLIENT_MAX_CONCURRENT_REQUESTS` | `16` | One address occupying many request slots with slow requests |
| `CLIENT_RATE_PER_SEC` | `20` | One address sending a sustained flood. Refilled continuously |
| `CLIENT_BURST` | `60` | Size of a client's token bucket: how many requests it may send at once. A page load with its assets is about ten |
| `MAX_TRACKED_CLIENTS` | `65536` | The rate limiter's own memory. A client that has been quiet long enough to refill its bucket is forgotten. If more clients than this are active within one refill period (`CLIENT_BURST / CLIENT_RATE_PER_SEC` seconds), new ones get `503` until some go quiet, and clients already tracked are still served |
| `TRUSTED_PROXY_HEADER` | unset | Where the client address comes from. Unset: the socket peer. Set (for example `X-Real-IP` or `X-Forwarded-For`): the last address in the last instance of that header, falling back to the socket peer when it does not parse. Set it only when every request reaches the server through a proxy that writes the header, and make the server unreachable any other way; otherwise clients choose their own address |

Fixed limits: a request head (request line and headers) may be at most 16 KiB. A malformed or oversized request head is refused by the HTTP parser itself with an empty `400` or `431` and the connection is closed; those responses carry no body and so no error JSON or security headers.

Clients are counted by IPv4 address or by IPv6 /64, since one host routinely holds a whole /64. Many users behind one NAT share a single client's limits; raise `CLIENT_RATE_PER_SEC`, `CLIENT_BURST` and `CLIENT_MAX_CONCURRENT_REQUESTS` if that is your audience.

### Block summaries

The block summary routes ([`FRONTEND.md`](FRONTEND.md#block-summaries)) keep computed summaries in memory. Their other limits are fixed: one computation at a time per network, six started by clients running or waiting, and ten per client at once then one every 2 seconds.

| Variable | Default | Protects against |
|---|---|---|
| `BLOCK_SUMMARY_CACHE_MB` | `32` | Memory held by cached summaries, per network: the least recently used go first once it is reached. A summary takes 20 bytes per transaction plus about 0.5 KB: 40 to 140 KB for today's mainnet blocks, so the default holds 230 to 800 of them. Measured with the cache full: 55 MB resident for the whole process. Each summary not in the cache costs the node a `getblock` of up to 10 MB. At most 1,048,576 |

### Block table

The block table ([`STORE.md`](STORE.md#blocks)) needs `DATA_DIR`. Its newest 100 rows follow the tip whatever these say; the backfill fills the rest. The backfill has its own RPC client with one call in flight, outside the four the rest of the server shares, and a 30 s timeout.

| Variable | Default | Protects against |
|---|---|---|
| `BACKFILL_ENABLED` | `true` | `false` pauses the backfill. The table is its progress, so the next start with `true` resumes where it stopped, redoing nothing |
| `BACKFILL_BUSY_PERCENT` | `50` | Load on the node. The share of wall time, 1 to 100, the backfill keeps its one call busy: after a step that took `t` it waits `t × (100 − p) / p`. It also backs off from 5 s to 5 minutes, doubling, whenever the node is busy or failing. At 50, every mainnet block's totals are in after about 21 hours and a year's fee rates after about 38; the costs are in [`STORE.md`](STORE.md#backfill) |
| `BACKFILL_FEE_RATE_DEPTH` | `52560` | How far back fee rates are filled, in blocks below the tip (52,560 is about a year). Each costs a `getblock` of up to 10 MB and 2 s of node time; `0` fills none. Rows deeper than this keep their totals and coinbase, and the block facts route computes their fee rates on demand |

### WebSocket

Bounds on `/app/ws` ([`SECURITY.md`](SECURITY.md) rule 9; the protocol and its close codes are in [`WS.md`](WS.md)). Each must be a positive integer. An upgraded socket gives up its slot under `MAX_CONNECTIONS` and `CLIENT_MAX_CONNECTIONS` and counts under these instead, so keep `MAX_CONNECTIONS` plus `WS_MAX_SOCKETS` below the process's open-file limit. A socket that stops reading is dropped after `REQUEST_TIMEOUT_SECS` without write progress, as for HTTP.

| Variable | Default | Protects against |
|---|---|---|
| `WS_MAX_SOCKETS` | `1024` | Memory and file descriptors held by open sockets, every network together. Past it an upgrade gets `503`. At most 1,048,576 |
| `WS_CLIENT_MAX_SOCKETS` | `8` | One address holding many sockets. Past it an upgrade gets `429`. Applies behind a trusted proxy too, keyed on the proxy header's address |
| `WS_MAX_MESSAGE_BYTES` | `1024` | Large messages from a client. Checked against the frame header before the payload is read, and against a fragmented message's total. Over it: close `1009`. At most 1,048,576 |
| `WS_MESSAGES_PER_SEC` | `10` | A client flooding the server with frames, pings and pongs included. The same number is the burst allowed. Over it: close `4002` |
| `WS_MAX_SUBSCRIPTIONS` | `8` | One socket subscribing to many topics. Over it: close `4001` |
| `WS_SEND_QUEUE` | `64` | Memory held for a client that does not read. Messages waiting for it are shared, not copied. When full: close `4003`. At most 65,536 |
| `WS_IDLE_TIMEOUT_SECS` | `60` | Sockets whose client vanished without closing. A socket silent for half of it is pinged, and one silent for all of it is closed with `4004` |
| `WS_MAX_BLOCK_SUBSCRIPTIONS` | `2` | One socket holding many projected blocks' transactions, the only large topics (up to about 800 KB a snapshot). Counted inside `WS_MAX_SUBSCRIPTIONS`. Over it: close `4001` |
| `WS_MEMPOOL_INTERVAL_MS` | `2000` | Traffic and fan-out work on the mempool topics: each sends at most one message per interval, always the latest state, so changes in between are merged. 100 to 60,000 |
| `WS_LATEST_TXS` | `10` | Size of the `transactions` list, the newest transactions seen. At most 100 |
