# Configuration

The server reads its configuration from the environment once, at startup. Invalid or half-set configuration stops it with a message naming the variable; values are never echoed, so a mistyped password does not land in a log. The error is written as a `config_invalid` event ([`LOGGING.md`](LOGGING.md)). An empty value counts as unset.

[`.env.example`](../.env.example) is a starting point. The server does not read `.env` itself; load it into the environment (`set -a; . ./.env; set +a`) or pass the variables through your process manager.

## Server

| Variable | Default | What it does |
|---|---|---|
| `LISTEN_ADDR` | `127.0.0.1:8080` | Address and port to listen on. Port `0` picks a free one; the `listening` event shows which |
| `WEB_DIR` | `web/dist` | The built frontend. Nothing outside it is served |

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
