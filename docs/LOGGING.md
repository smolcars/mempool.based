# Logging

The server writes structured logs to stdout. Every line is one **event**: a declared Rust type with named, typed fields, a stable name and a level. There is no free-text logging call anywhere. The event types in [`crates/logging/src/events.rs`](../crates/logging/src/events.rs) are everything the server can say, and the catalogue below lists them all.

## Rules

1. **Typed events only.** No `println!`, `eprintln!`, `dbg!` or message strings. The workspace denies the print macros through clippy; tests and manual probes allow them locally, with a comment.
2. **Add an event when an operator would act on it or needs it to explain an incident:** a request, a lifecycle step, a lost or restored upstream, a limit that refused work. Do not add one to trace control flow, to say a function ran, or for something a request event already shows.
3. **Summarise repeats.** Something that can fail in a loop (a reconnect, a poll) logs on the 1st, 2nd, 4th, 8th… occurrence in a row, carrying the count, and logs again when it recovers. Never one line per attempt.
4. **Privacy by default.** A request event carries the matched route template, never the raw path or query string: the path of an explorer request says which address or transaction someone looked up. Client addresses are off unless the operator turns them on (`LOG_CLIENT_ADDR`).
5. **No secrets.** No event field holds a password, a token, a credentialed URL or a request body. Configuration types that hold secrets (`Secret`) cannot be formatted, and the config refuses URLs with credentials, so nothing can carry them into an event by accident.
6. **Library crates do not log.** `node` and `engine` report through values: errors they return, and state they expose (the Electrum client's `health()` watch). The server turns those into events (`crates/server/src/report.rs`). This keeps the libraries free of global state and testable without a logger.
7. **Keep fields bounded.** A field holds a fixed label, a number, or text whose size the code controls. Attacker-chosen text is allowed only when it is needed (the node's software string, a panic message) and is always escaped, never trusted to be short or clean.
8. **A client cannot choose how much we write.** An event a client can cause at will is either one per request served (bounded by its limits) or a summary: refusals are counted ([below](#refusals)), repeats follow rule 3. Above that, the whole log has a ceiling ([below](#the-ceiling)). A new event says in the [table](#what-a-client-can-make-us-write) who can cause it and how often.

## Field conventions

- snake_case names; units in the name: `duration_ms`, `upstream_ms`, `bytes`, `request_timeout_secs`.
- The same name for the same thing everywhere: `network` (`mainnet`, `signet`, `testnet4`, `regtest`), `request_id`, `route`, `status`, `method`.
- Labels are fixed snake_case strings from our code (`refused: "client_rate"`), not formatted messages.
- An optional field that does not apply is left out of the line, not written as `null`.
- `ts`, `level` and `event` are reserved for the line header and are never field names.

## Output

| Format | Line |
|---|---|
| `json` | `{"ts":"2026-11-11T09:30:12.345Z","level":"info","event":"http_request","request_id":"…","route":"/api/{*rest}",…}` |
| `text` | `2026-11-11T09:30:12.345Z INFO  http_request duration_ms=0.41 method=GET route=/api/{*rest} status=200 …` |

`ts` is RFC 3339 in UTC with milliseconds. In `json`, fields follow the header in declaration order; in `text` they are sorted by name, plain values are written bare and anything else as a JSON string.

**One event is always exactly one line, and a value cannot leave its field.** Quotes, backslashes and control characters are escaped by the JSON encoder; DEL, C1 controls (NEL and the 8-bit CSI among them), Unicode line and paragraph separators and bidirectional overrides are escaped as `\uXXXX` in both formats. Invalid UTF-8 cannot reach a field: fields are Rust strings, so bytes from the network are converted lossily first.

**Logging never blocks or crashes the server.** `emit` formats the line on the caller's thread and puts it on a bounded queue (4,096 lines) drained by one writer thread. When the queue is full (the output is slower than the events: a slow or full disk, a stalled pipe) the event is dropped and counted, and `log_events_dropped` says how many of each kind, written by the writer itself once it can write again. A write error is ignored. A request never waits for the log: `crates/server/tests/slow_log.rs` answers 8,192 requests with the output blocked outright. At exit the server waits at most two seconds for the queue to empty.

## Settings

`LOG_FORMAT`, `LOG_LEVEL`, `LOG_REQUESTS` and `LOG_CLIENT_ADDR` are in [`CONFIG.md`](CONFIG.md#logging). A passing `/healthz` or `/readyz` is never logged, whatever `LOG_REQUESTS` says, so health checks do not flood the log.

### Why client addresses are off by default

A request event already says when someone used which kind of page or API route. With the client address it becomes a per-person activity record, and together with the timing it can often be matched back to what was looked up. The operator did not ask for that record, and anyone who later reads the log (a backup, a log service, a subpoena) gets it too. The limits that stop abuse work on live counters and do not need the log. An operator investigating abuse can turn `LOG_CLIENT_ADDR` on for the duration, or use the access log of the reverse proxy in front of the server, which most deployments already keep under their own retention policy.

## What a client can make us write

Who can cause each event that is not on a fixed schedule, and how often. Per-client limits are in [`CONFIG.md`](CONFIG.md#limits) (defaults: 20 API requests a second, a burst of 120; static files 100 a second, a burst of 400). "Many clients" is what the [ceiling](#the-ceiling) is for: a crowd each within its limits is bounded only by the server's throughput.

| Event | Who causes it | How often, at most |
|---|---|---|
| `http_request`, served (any status a handler answered, `400`, `404` and `413` included) | Any client | One per request admitted: 20 a second per client sustained on the API (120 at once), 100 on static files (400 at once). Many clients together: as many as the server answers, so the ceiling applies; `LOG_REQUESTS=failures` or `off` lowers it |
| `http_request`, an `OPTIONS` preflight to `/api` | Any client | One each, `204`: preflights are answered before the limits, so browsers can always read a refusal. Bounded only by the [ceiling](#the-ceiling) |
| `http_request`, refused by a limit (`refused` set, except `timeout`) | Any client | One per reason after a minute without that reason; the rest go into `requests_refused` |
| `http_request`, `timeout` | A client whose request outlives `REQUEST_TIMEOUT_SECS` | One each: admitted work that ran, bounded by `CLIENT_MAX_CONCURRENT_REQUESTS` per timeout per client |
| `requests_refused` | Any client | One a minute while anything is refused, the same size whatever the number of clients |
| `connections_refused` | A client past `CLIENT_MAX_CONNECTIONS` | At a total of 1, 2, 4, 8…: about 30 lines over a process's life |
| `ws_origin_refused` | Another site's page | At a total of 1, 2, 4, 8… |
| `ws_clients_dropped` (`too_big`, `rate`, `bad_message`, `binary`, `invalid_utf8`, `protocol_error`, `too_many_subscriptions`, `too_slow`, `idle`) | A client breaking a WebSocket limit | At a total of 1, 2, 4, 8… per reason and network. Each socket first cost an admitted upgrade |
| `ws_closed` (debug) | Any client opening and closing sockets | One per socket, each of which cost an admitted upgrade; debug, so off by default and under the ceiling |
| `tracking_refused` | Clients following more transactions or addresses than the server holds | At a total of 1, 2, 4, 8… per kind and network |
| `block_summaries_refused` | Clients asking for uncached block summaries past the queue or their share | At a total of 1, 2, 4, 8… per reason and network |
| `block_summary_computed` (debug) | A client asking for an uncached block summary | One per computation, bounded by `route_rate` per client and the queue; debug |
| `mining_failed`, `mempool_history_failed` (`task` `request`) | A client asking while the store fails | At the 1st, 2nd, 4th, 8th… failure in a row |
| `panic` | A bug a client can reach | One per panic. Kept, because each is a bug to fix ([`SECURITY.md`](SECURITY.md) rule 18 says it cannot happen on untrusted input) |
| Broadcast refusals | A client posting transactions | `http_request` lines like any other request: `route_rate` refusals are summarised |
| Anything else | Not a client: the node, the chain, the schedule | Lifecycle once, summaries once a minute, block facts once a block, failures by rule 3 |

Requests hyper rejects before they reach us (a malformed request line, a head over 16 KB) get a `400` or `431` from hyper and no event at all.

### Refusals

A request a limit refused is counted, not written (`crates/server/src/refusals.rs`). Once a minute, if anything was refused, one `requests_refused` (warn) says how many by reason and by route class, how many distinct clients, and how much the busiest one took. Its size never depends on the attackers: reasons are labels from our code, route classes are five, distinct clients are estimated in a 2 KB sketch and the busiest is found among 8 counters.

**The first refusal of each reason after a minute without one is also written whole**, as the `http_request` it would always have been, at once: an operator chasing one user's `429` finds it by its `X-Request-Id`. A `timeout` is not a refusal here: it is written like any served request.

Telling an attack from a mistake:

| What the summary shows | Likely cause |
|---|---|
| `top_client_refused` near `total`, `untrusted_forwarded` and `proxy_keyed` near 0 | One address flooding. Block it in front, if it bothers you; it costs about 17 µs of CPU per refusal and nothing else |
| `untrusted_forwarded` near `total` | A proxy in front of the server is not in `TRUSTED_PROXIES`, so every visitor is that one address ([`CONFIG.md`](CONFIG.md#client-addresses)) |
| `proxy_keyed` near `total` | The proxy is trusted but does not send the configured header (or sends it unparsable), so every visitor is the proxy |
| `clients` high and `top_client_refused` small | Many addresses, each a little over its limit: a crowd or a distributed flood. Raise the limits if they are your users |

A refused request is answered before it is routed (`crates/server/src/serve.rs`): the per-client check runs as soon as hyper has the request head, and the refusal is the same response the router would have built, without the routing, middleware or log line. A flood of one client at 150,000 to 210,000 requests a second costs about 17 µs of CPU per refusal on the development machine, against 27 µs before; what is left is the kernel's socket reads and writes, which only a proxy or firewall in front can save.

### The ceiling

Above `LOG_MAX_BYTES_PER_SEC` (default 256 KiB a second, after a burst of ten seconds' worth), events that may be dropped are dropped before they are formatted, and counted in `log_events_dropped` by kind. The check is one atomic load, so a dropped event costs next to nothing.

- **May be dropped:** `http_request`, every `debug` event, and any `info` event not marked `keep` in [`events.rs`](../crates/logging/src/events.rs). A new event is droppable until it is marked.
- **Never dropped:** every `warn` and `error` event (each is already a summary by rule 3, or a fact an operator must not lose: `panic`, `rpc_auth_failed`, `store_open_failed`, `requests_refused`), and the `info` events marked `keep`: lifecycle (`server_starting`, `listening`, `server_stopped`…), the once-a-minute summaries and the once-a-block facts. Their rate is fixed by the code, so they are not charged against the ceiling. `log_events_dropped` is written by the writer itself.
- An `http_request` line is about 250 bytes, so the default is about 1,000 requests a second sustained: 90 million a day, which a busy honest site does not reach.

**Worst case.** At the ceiling the log grows by 256 KiB a second: **22.6 GB (21.1 GiB) a day**, plus the kept events (a few megabytes a day) and one 2.5 MB burst. Size the disk, or the log rotation in front of it (journald, logrotate, Docker's `max-size`), for that, or lower `LOG_MAX_BYTES_PER_SEC`.

### For operators

What a client can make the server log is bounded, whatever it sends. Each request served is still one `http_request` line; refused requests are counted in one `requests_refused` line a minute (plus the first of each kind, written whole); and the whole log is capped at `LOG_MAX_BYTES_PER_SEC`, 256 KiB a second by default, past which request lines are dropped and counted in `log_events_dropped` while warnings, errors and summaries are always written. So the most the log can grow is about 22.6 GB a day at the default: plan the disk or the rotation for that, or lower the setting ([`CONFIG.md`](CONFIG.md#logging)). Seeing `log_events_dropped` with `over_budget` means the site served more than the ceiling: raise it if that is real traffic. Seeing `requests_refused` tells you whether one address is flooding or a proxy is misconfigured ([refusals](#refusals)).

## Request ids

Every response carries `X-Request-Id`, a 16-digit hex id that also appears in its `http_request` event, so a user's report can be matched to one line. It is exposed to cross-origin callers of `/api`. Ids are unique within a process and unguessable; a client cannot set its own.

## Adding an event

1. Declare a struct in `crates/logging/src/events.rs` with `#[derive(Debug, Serialize)]` and the fields it needs, following the conventions above. Borrow strings (`&'a str`) rather than allocating.
2. Give it a name and level on the line after: `event!(UpstreamSlow, "upstream_slow", Warn);`. An `info` event the [ceiling](#the-ceiling) must never drop ends in `keep`: `event!(MempoolSummary, "mempool_summary", Info, keep);`, only for an event whose rate the code fixes whatever clients do. If a client can cause it at will, add a row to [the table](#what-a-client-can-make-us-write).
3. Add a sample to `samples()` in the same file's tests, and a row to the catalogue below. Tests fail until both exist.
4. Emit it where the thing happens: `logging::emit(&UpstreamSlow { network, endpoint, duration_ms });`

## Catalogue

### Lifecycle

| Event | Level | Fields | When |
|---|---|---|---|
| `server_starting` | info | `version`, `revision` (the commit built, or `unknown`), `listen_addr`, `web_dir`, `networks`, `log_format`, `log_level`, `log_requests`, `log_client_addr`, `log_max_bytes_per_sec`, `broadcast_enabled`, every limit and backfill setting from [`CONFIG.md`](CONFIG.md) by its variable's name in lower case (`max_body_bytes`, `request_timeout_secs`, `backfill_busy_percent`, …), `trusted_proxy_header` and `trusted_proxies` (as prefixes) and `data_dir` if set | Configuration parsed, before listening. Never the RPC user or password |
| `network_enabled` | info | `network`, `rpc_addr`, `esplora`, `electrum_addr`, `mempool`, `lightning` | Once per enabled network at startup. `mempool` is whether its mempool engine runs (`{NET}_MEMPOOL_ENABLED`), `lightning` whether its Lightning peer does (`{NET}_LIGHTNING_ENABLED`) |
| `no_networks` | warn | none | No network is configured |
| `web_dir_missing` | warn | `web_dir` | `WEB_DIR` has no `index.html` |
| `listening` | info | `addr` | The listener is bound; shows the real port when asked for port 0 |
| `shutdown_requested` | info | `signal` (`sigterm` or `ctrl_c`) | A shutdown signal arrived |
| `server_stopped` | info | none | In-flight requests are done and the process exits normally |
| `config_invalid` | error | `error` | Invalid configuration; the process exits. The message names the variable, never its value |
| `listen_failed` | error | `addr`, `error` | The listen address could not be bound; the process exits |
| `panic` | error | `message`, `location`, `thread` | Any panic. In a request handler the client gets a 500 and the server keeps running |
| `log_events_dropped` | warn | `count`, `queue_full`, `over_budget`, `events` | Events were dropped: `queue_full` because the output fell behind, `over_budget` past the [ceiling](#the-ceiling); `events` counts them by event name. Written by the writer at most once a second, the first at once |
| `connections_refused` | warn | `total` | Connections closed as soon as they were accepted because their address already had `CLIENT_MAX_CONNECTIONS` open: at a total of 1, 2, 4, 8…. The client sees a reset and no response, so no `http_request` records it |

### HTTP

| Event | Level | Fields | When |
|---|---|---|---|
| `http_request` | info | `request_id`, `network`, `method`, `route`, `status`, `duration_ms`, `bytes`, `refused`, `endpoint`, `source`, `upstream_ms`, `upstream_error`, `client` | Each request served, when its response head is ready, subject to `LOG_REQUESTS`. A refused request only when it is the first of its reason after a quiet minute ([refusals](#refusals)) |
| `requests_refused` | warn | `window_secs`, `total`, `logged`, `reasons`, `routes`, `clients`, `top_client_refused`, `top_client`, `untrusted_forwarded`, `proxy_keyed` | Once a minute while requests are refused, and at shutdown: how many (`logged` of them were the first of their reason, written whole when `LOG_REQUESTS` allows), by `refused` reason and by route class (`api`, `app`, `ws`, `health`, `pages`), distinct clients (estimated), the busiest client's count (a lower bound) and, only with `LOG_CLIENT_ADDR=true`, its address, and the [proxy hints](#refusals) |

- `route` is the matched route template (`/signet/api/{*rest}`, `/healthz`), or `frontend` for pages and static files.
- `network` is left out for `/healthz`, `/assets` and unknown network prefixes.
- `duration_ms` runs until the response head is ready; streaming the body is not included. `bytes` is the body size when known up front.
- `refused` names the limit that said no: `client_rate`, `client_concurrency`, `client_table_full`, `server_busy`, `timeout`, `route_rate` (a route's own stricter limit, such as broadcast or one client's uncached block summaries), `summary_busy` (the network's block summary queue is full), `heavy_busy` (the network's address and batch calls are all running) and `route_concurrency` (one client's are), or for a WebSocket upgrade `origin` (another site's page, [`WS.md`](WS.md#connection-rules)), `client_sockets` (`WS_CLIENT_MAX_SOCKETS`), `server_sockets` (`WS_MAX_SOCKETS`), `v1_client_sockets` (`WS_V1_CLIENT_MAX_SOCKETS`) and `v1_server_sockets` (`WS_V1_MAX_SOCKETS`). A `413` is an oversized body.
- `endpoint`, `source`, `upstream_ms` and `upstream_error` are set for `/api` requests that reached the pass-through ([`API.md`](API.md)), for the block summary routes and for the mining routes: the named route (`tx_status`, `block_summary`, `block_summary_txs`, `block_summaries`, `mining_pools`, `mining_hashrate`, …), where the answer came from (`upstream`, `coalesced` onto another request's call, `hot` from the short cache, `cached` from the block summary cache or a mining answer already computed, or `stored` from the block table), this request's own upstream time, and why the upstream call got no answer (`timeout`, `unavailable`, `response_too_big`, `decode`, `busy` when the node's RPC queue was full, or `auth` when the node refused our RPC credentials; the client sees only `upstream unavailable`).
- `client` only with `LOG_CLIENT_ADDR=true`.
- Dropped first when the log is over its [ceiling](#the-ceiling).

### Node clients

| Event | Level | Fields | When |
|---|---|---|---|
| `electrum_connected` | info | `network`, `server`, `protocol` | The Electrum connection is up, subscriptions restored. `server` is the node's own software string |
| `electrum_disconnected` | warn | `network` | A ready connection was lost; the client reconnects with backoff |
| `electrum_unreachable` | warn | `network`, `attempts` | Connection attempts keep failing: at the 1st, 2nd, 4th, 8th… in a row |
| `electrum_resynced` | info | `network` | Updates may have been missed (reconnect, or a tip jump too far to follow) |
| `electrum_lines_skipped` | warn | `network`, `total` | The node sent lines that are not valid JSON-RPC; at a total of 1, 2, 4, 8… |
| `chain_reorg` | warn | `network`, `fork_height` | Blocks above `fork_height` left the best chain |
| `rpc_auth_failed` | error | `network` | The node refused `{NET}_RPC_USER` and `{NET}_RPC_PASSWORD`. Checked at startup and every 30 s ([`DEPLOY.md`](DEPLOY.md#health)); written when the refusals start. The server keeps running: fix the credentials. Clients see only `upstream unavailable` |
| `rpc_unreachable` | warn | `network`, `error`, `failures` | The RPC check got no answer (`timeout`, `unavailable`, `busy`, `decode`): at the 1st, 2nd, 4th, 8th… check in a row. Look at the network, or whether the node is up |
| `rpc_reachable` | info | `network`, `height`, `failures` | The RPC check got an answer with our credentials: the first at startup, and the first after failures or a refused password |
| `address_index_off` | warn | `network` | The node runs without `--sh-index`: address pages and the address and scripthash routes answer `503`. Asked once the Electrum connection is up and after every reconnect (one lookup of an unused scripthash); written at the first answer and when it changes. Also `address_index` in `/readyz` ([`DEPLOY.md`](DEPLOY.md#health)) |
| `address_index_on` | info | `network` | The node has its address index: the first answer, and after it was off |

`crates/server/src/report.rs` produces these from the client's `health()` watch and event stream; call `report_electrum` once where each network's Electrum client is created. The Esplora and RPC clients return their failures as error values (`rpc::Error::Busy` is the node's back-pressure); the code that calls them owns the event, summarised by rule 3.

### WebSocket hub

The protocol and its close codes are in [`WS.md`](WS.md).

| Event | Level | Fields | When |
|---|---|---|---|
| `ws_fanout` | info | `network`, `topic`, `kind`, `subscribers`, `open`, `bytes`, `dropped_slow`, `duration_ms`, `fanout_ms` | One message went to every subscriber of a `blocks` topic: once per new block, block summary, reorg or resync per network, whatever the client count. The mempool topics, which send every few seconds, are summed in `mempool_summary` instead. `duration_ms` runs from the node's notification to the last subscriber's queue (the block fetch included); `fanout_ms` is the queueing part. `open` counts sockets on every network |
| `ws_origin_refused` | warn | `network`, `origin`, `host`, `total` | Upgrades refused because their `Origin` is not this site, every network together: at a total of 1, 2, 4, 8…, with the latest refused `Origin` and `Host`, each cut to 100 characters. A steady stream means another site embeds ours; refusals of our own pages mean the proxy rewrites `Host` |
| `ws_clients_dropped` | warn | `network`, `reason`, `total` | The server closed clients for breaking a limit: `too_slow`, `idle`, `rate`, `too_big`, `bad_message`, `too_many_subscriptions`, `protocol_error`, `binary` or `invalid_utf8`. At a total of 1, 2, 4, 8… per reason and network |
| `ws_closed` | debug | `network`, `api`, `code`, `reason`, `duration_ms`, `open` | Every socket close: `api` is `app` for `/app/ws` or `v1` for `/api/v1/ws`, then its close code (1006 when the connection dropped without one), the reason label (those above, or `client_closed`, `shutdown`, `connection_lost`), how long it was open, and the sockets still open |
| `block_unavailable` | warn | `network`, `height`, `hash`, `failures` | A new block's JSON could not be fetched from Esplora within `WS_BLOCK_WAIT_SECS` (60 s) of its announcement, so subscribers were sent `resync`; the block table keeps looking for it. At the 1st, 2nd, 4th, 8th… failure in a row |

### Block summaries

The routes are in [`FRONTEND.md`](FRONTEND.md#block-summaries). Each request is also an `http_request` with `endpoint` and `source`.

| Event | Level | Fields | When |
|---|---|---|---|
| `block_summary_computed` | debug | `network`, `height`, `txs`, `block_bytes`, `bytes`, `node_ms`, `duration_ms`, `cause`, `source` | A summary was computed: the block's transactions and size, the records kept, the node calls' time and the whole time including the wait for a slot. `cause` is `request` or `new_block`; `source` is what listed the transactions, `summary_route` or `getblock` ([`RBITCOIN.md`](RBITCOIN.md#listing-a-blocks-transactions)). Debug, since a busy server computes a few a second |
| `block_txs_source` | info | `network`, `source`, `reason` | Which source lists the network's blocks for the summaries, the audit and the fee-rate backfill: `summary_route` (`/block/:hash/txs/summary`) or `getblock`. `reason` is `probe` (asked when the node connection came up: at startup and on every reconnect), `answered` (the route listed a block before any probe got an answer) or `refused` (the node refused the route for a block, so it has `getblock` until the next probe) |
| `block_txs_fell_back` | warn | `network`, `reason`, `total` | The route failed for a block, which `getblock` listed instead; the source stays. `reason` is the latest: `not_found` (the route does not list it, as for a block off the best chain), `unavailable`, `too_big`, `malformed` or `mismatch` (its row count or coinbase disagreed with the block's header). At a total of 1, 2, 4, 8… per network |
| `block_summary_failed` | warn | `network`, `error`, `failures` | The node gave no usable answer (`busy`, `timeout`, `unavailable`, `response_too_big`, `decode`). At the 1st, 2nd, 4th, 8th… failure in a row. An unknown block is not a failure |
| `block_summaries_recovered` | info | `network`, `failures` | A summary was computed after `failures` in a row |
| `block_summaries_refused` | warn | `network`, `reason`, `total` | Requests for uncached blocks were refused: `summary_busy` (the network's queue was full) or `route_rate` (one client over its share). At a total of 1, 2, 4, 8… per reason and network |
| `block_audited` | info | `network`, `height`, `match_rate`, `expected_txs`, `matched_txs`, `missing_txs`, `displaced_txs`, `fresh_txs`, `conflicted_txs`, `added_txs`, `unseen_txs`, `duration_ms` | A new block was audited against the block we expected on its parent ([`MEMPOOL.md`](MEMPOOL.md#block-audits)): its score and each category's size. Once per block |
| `block_audit_skipped` | info | `network`, `height`, `reason` | A new block got no audit, and why: `no_projection` (no projection on its parent: the server started after it, or the projector had not caught up), `stale` (the projection on its parent was stale), `superseded` (another block replaced it or followed it before it could be audited), `store` (the write failed). Once per block. A server without `DATA_DIR` or with the network's mempool engine off audits nothing and says so only at startup (`server_starting`, `network_enabled`) |

Opening a socket is an `http_request` with route `/app/ws` (or the network's prefix) and status `101`; a refused upgrade is counted in `requests_refused` under route class `ws`, and the first after a quiet minute is the same event with `403`, `429` or `503` and `refused` naming the cap.

### The live mempool

How the tracker and projected blocks run in the server is in [`MEMPOOL.md`](MEMPOOL.md#in-the-server). A healthy network writes `mempool_synced` once and then one `mempool_summary` a minute; everything else is an abnormal condition.

| Event | Level | Fields | When |
|---|---|---|---|
| `mempool_synced` | info | `network`, `duration_ms`, `txs`, `bytes`, `calls` | The tracker's first complete cycle: the initial sync, failed attempts included in the time |
| `mempool_summary` | info | `network`, `txs`, `cycles`, `skipped`, `added`, `confirmed`, `replaced`, `evicted`, `vanished`, `node_calls`, `node_bytes`, `cycle_ms_max`, `updates`, `cluster_calls`, `clusters_asked`, `cluster_bytes`, `pack_ms_max`, `stale`, `ws_messages`, `ws_bytes`, `ws_sent_bytes`, `ws_dropped_slow`, `fanout_ms_max` | Once a minute per network while the tracker is not failing: the minute's tracker cycles and their node cost, the projector's updates and cluster requests, and the mempool topics' messages (`ws_bytes` encoded once; `ws_sent_bytes` that times the subscribers). `txs` and `stale` are the state now |
| `mempool_block` | info | `network`, `height`, `path`, `confirmed`, `replaced`, `tracker_ms`, `sent_ms`, `node_calls`, `node_bytes`, `refresh_ms`, `refresh_set`, `refresh_removed`, `refresh_bytes` | Once per block, when the once-a-block refresh of every cluster completes ([`MEMPOOL.md`](MEMPOOL.md#when-a-block-is-found)). Times run from Electrum's announcement: `tracker_ms` until the tracker dropped what the block confirmed, `sent_ms` until the mempool topics went out on it, `refresh_ms` until the refresh was done. `path` is `block` when the tracker applied the block from its own transactions (`node_calls` and `node_bytes` are that step's cost) and `listing` when its next poll did. `refresh_set` and `refresh_removed` are the records the refresh corrected: how far the projection first sent for the block was from the node's full answer. Blocks that arrive faster than the refresh share one line, under the newest |
| `mempool_poll_failed` | warn | `network`, `error`, `failures`, `retry_in_ms` | A tracker cycle failed and nothing was published: at the 1st, 2nd, 4th, 8th… failure in a row. `error` is `busy`, `timeout`, `unavailable`, `response_too_big`, `decode`, `auth` or `failed` |
| `mempool_recovered` | info | `network`, `failures` | A cycle succeeded after `failures` in a row |
| `mempool_sequence_regressed` | warn | `network`, `from`, `to` | The node's `mempool_sequence` went backwards: it restarted |
| `projection_fetch_failed` | warn | `network`, `error`, `failures`, `pending`, `retry_in_ms` | A cluster request failed; the projected blocks are marked stale. At the 1st, 2nd, 4th, 8th… failure in a row. `error` as above |
| `projection_recovered` | info | `network`, `failures` | The projected blocks are current again after failed requests |
| `projection_resynced` | warn | `network`, `txs` | The projector fell behind the tracker and rebuilt from a snapshot |
| `mempool_subscriber_lagged` | warn | `network`, `source`, `missed`, `total` | A reader of the `tracker`'s or the `projector`'s change stream fell behind and started over. At a total of 1, 2, 4, 8… per source and network |
| `fees_summary` | info | `network`, `source`, `high`, `medium`, `low`, `no_rush`, `minimum`, `inflow_minutes`, `ws_messages` | Once a minute per network: the fee recommendations being served, in sat per 1,000 vB ([`MEMPOOL.md`](MEMPOOL.md#fee-recommendations)), whether they are ours (`projected`), the node's (`node`) or nothing (`none`, and the rates left out), the minutes of arrivals watched, and the `fees` messages published that minute |
| `fees_fallback_started` | warn | `network`, `reason` | Our own recommendations stopped and the node's estimates, or nothing, are served instead. `reason` is `warming_up` (from start until five minutes of arrivals were watched), `stale` (the projection is, because the node is failing) or `engine_off` (once at start) |
| `fees_fallback_ended` | info | `network`, `duration_ms` | Our own recommendations are served again |

### Following transactions and addresses

The `tx:` and `address:` topics ([`WS.md`](WS.md#following-a-transaction-or-an-address)). **No event names a transaction or an address**: the log must not record what people look at, for the same reason as rule 4. A network writes one summary a minute while anything is followed or happened, and nothing while idle.

| Event | Level | Fields | When |
|---|---|---|---|
| `tracking_summary` | info | `network`, `txs`, `addresses`, `subscribed`, `refused`, `messages`, `bytes`, `address_errors`, `replacements`, `replaced` | Once a minute per network while anything is followed or happened: transactions and addresses followed now, new ones followed in the minute, refused for a full server, messages sent and their bytes (encoded once), addresses the node would not or could not watch, the replacement record's size now and the replacements recorded in the minute |
| `tracking_refused` | warn | `network`, `kind`, `reason`, `total` | A socket was refused a transaction (`kind` `tx`) or address (`address`) because the network already follows `WS_TRACKED_TXS_TOTAL` or `WS_TRACKED_ADDRESSES_TOTAL` (`reason` `server_full`). At a total of 1, 2, 4, 8… per kind and network |
| `replacements_restored` | info | `network`, `links`, `duration_ms` | The replacement record was put back from the store when the mempool engine started ([`STORE.md`](STORE.md#replacements)): `links` read, oldest first. Once per network per start, with a `DATA_DIR` |
| `replacements_store_failed` | warn | `network`, `task`, `error`, `links`, `failures` | The record could not be read back (`task` `restore`) or a change written (`write`): `error` is `queue_full` (the writer was 64 writes behind and this one was dropped; memory still has it), `timed_out` or `store`. `links` is how many the lost write held. At the 1st, 2nd, 4th, 8th… failure in a row |

### Derived data

| Event | Level | Fields | When |
|---|---|---|---|
| `difficulty_computed` | info | `network`, `height`, `next_retarget_height`, `difficulty_change`, `duration_ms` | The difficulty-adjustment answer ([`API.md`](API.md#difficulty-adjustment)) was recomputed for a new tip: once per block or reorg per network. `difficulty_change` is the expected change in percent at that moment; `duration_ms` is the node fetches |
| `difficulty_refresh_failed` | warn | `network`, `height`, `error`, `failures` | It could not be recomputed for the tip at `height`, so the last answer is still served: at the 1st, 2nd, 4th, 8th… failure in a row. `error` is `timeout`, `unavailable`, `response_too_big`, `decode`, `not_found`, `wrong_height` or `invalid_bits`. The next `difficulty_computed` is the recovery |
| `difficulty_refresh_retried` | debug | `network`, `height`, `retries` | Esplora answered for another tip than the one being computed (a block or reorg in between), so the refresh starts again: expected during a burst of blocks, so not a failure. At the 1st, 2nd, 4th, 8th… in a row |

### Store and block table

The store is in [`STORE.md`](STORE.md); the block table's settings in [`CONFIG.md`](CONFIG.md#block-table).

| Event | Level | Fields | When |
|---|---|---|---|
| `store_open_failed` | error | `network`, `error` | The network's SQLite file would not open (unwritable `DATA_DIR`, a corrupt file, a file from a newer build). That network runs without a store |
| `store_migration_applied` | info | `network`, `migration`, `duration_ms` | A schema migration ran while opening |
| `store_slow_write` | warn | `network`, `duration_ms`, `queue_depth` | One write held the writer for a second or more, with `queue_depth` writes waiting |
| `store_write_failed` | warn | `network`, `error`, `sqlite`, `failures` | SQLite refused a write: `error` is `disk_full`, `read_only` (a read-only mount, or a file the server may not write), `corrupt`, `busy` (another process held the lock past 5 s), `io` or `other`; `sqlite` is SQLite's own message. At the 1st, 2nd, 4th, 8th… in a row, whichever task wrote. `/readyz` shows `store` `failing` meanwhile |
| `store_write_recovered` | info | `network`, `failures` | A write committed after `failures` in a row had failed |
| `store_read_timed_out` | warn | `network`, `limit_ms` | A read ran past its limit and was stopped |
| `block_rows_followed` | debug | `network`, `tip`, `written`, `rated`, `duration_ms` | The newest rows were checked against a new tip: rows added or replaced, rows given fee rates. Debug, since it is once per block |
| `block_rows_reorged` | warn | `network`, `fork_height`, `stale` | Rows described blocks a reorg removed from the best chain; they were replaced or deleted |
| `block_rows_failed` | warn | `network`, `task`, `error`, `failures` | Following the tip (`follow`) or a backfill step (`tip`, `totals`, `rates`) failed with `busy`, `too_big`, `node` or `store`, and will be retried after a pause. At the 1st, 2nd, 4th, 8th… failure in a row |
| `block_rows_recovered` | info | `network`, `task`, `failures` | `follow` or `backfill` worked again after failures |
| `block_backfill_progress` | info | `network`, `tip`, `rows`, `rated`, `totals_done`, `rates_done`, `totals_written`, `rates_written`, `busy_ms`, `bytes` | Every minute while the backfill works, and when it has caught up: rows in the table and how many are rated, whether each tier has reached its end, and since start the rows it added and rated, the time it spent working and the bytes it read. The rate is the difference between two lines |

### Mining

The routes and what they cache are in [`API.md`](API.md#mining).

| Event | Level | Fields | When |
|---|---|---|---|
| `pools_reattributed` | info | `network`, `rows`, `changed`, `duration_ms` | Every row's pool was recomputed because the build's pool list or rules differ from the ones the table was attributed with (also on the first start after the column was added). The mining routes answer once it is done |
| `mining_refreshed` | debug | `network`, `tip`, `from`, `answers`, `duration_ms` | The answers clients have asked for were recomputed for a changed table: once a block, and at most once a minute while the backfill adds rows. `from` is the lowest height of the stored history they cover. Debug, since it is once per block |
| `mining_failed` | warn | `network`, `task`, `error`, `failures` | Mining answers could not be computed from the table (`task`: `reattribute`, `refresh` or `request`; `error`: `timed_out` past the store's read limit, or `store`). The last answers are still served. At the 1st, 2nd, 4th, 8th… failure in a row |

### Mempool history

The sampler and its table are in [`STORE.md`](STORE.md#mempool_history), the routes in [`API.md`](API.md#mempool-statistics). A healthy network writes one `mempool_history_skipped` and one `mempool_history_resumed` at start (the first minute's flow was not watched whole), then one `mempool_history_rolled_up` an hour once eight days are stored.

| Event | Level | Fields | When |
|---|---|---|---|
| `mempool_history_rolled_up` | info | `network`, `minutes`, `hours`, `duration_ms` | A sample's write also aged rows into coarser ones: `minutes` minute rows into hours, `hours` hour rows into days. `duration_ms` is the whole write |
| `mempool_history_skipped` | info | `network`, `reason`, `skipped` | Minutes left out of the history, at the 1st, 2nd, 4th, 8th… in a row, with the latest `reason`: `syncing` (the engine's first cycle, or the first minute after it started or came back), `stale` (the node is failing), `lagged` (the sampler fell behind the change stream), `late` (it read the minute more than 5 s off, or missed it), `store_busy` (the write queue was full) or `store_failed` |
| `mempool_history_resumed` | info | `network`, `skipped` | A minute was stored again after `skipped` in a row were not |
| `mempool_history_failed` | warn | `network`, `error`, `failures` | A history route could not read the table (`timed_out` past the store's read limit, or `store`). At the 1st, 2nd, 4th, 8th… failure in a row |

### Lightning

The peer and what it stores are in [`LIGHTNING.md`](LIGHTNING.md#in-the-server). A healthy mainnet start writes `lightning_started`, three `lightning_peer_connected`, `lightning_synced` after a few minutes, then one `lightning_summary` a minute, one `lightning_stats_stored` every ten minutes and one `lightning_channel_closed` per close (about 200 a day). **No event carries gossip content:** peers are named as configured, channels by their short channel id, and no alias, colour or announced address is ever written. LDK's own lines (`lightning_ldk_record`, debug) may quote what a peer sent; they stay in their `message` field, escaped like every field and cut to 512 bytes.

| Event | Level | Fields | When |
|---|---|---|---|
| `lightning_started` | info | `network`, `peers`, `max_peers`, `max_channels`, `graph`, `graph_channels`, `graph_nodes`, `graph_bytes`, `graph_age_secs`, `load_ms`, `error` | The peer started: its settings and its graph file, `fresh` (none), `loaded` (with its size and age) or `unreadable` (with why; it is replaced at the next save) |
| `lightning_failed` | error | `network`, `stage`, `error` | The peer did not start (`stage` `start`: the store did not open, or the data directory or node key is unusable), or stopped working (`run`): its server task panicked (`panicked`), or it sent no statistics for two minutes (`stalled`: its supervisor ended). The rest of the server keeps running; `/app/lightning/status` says `failed` |
| `lightning_stopped` | info | `network`, `graph_bytes`, `error`, `duration_ms` | The peer stopped at shutdown and wrote its graph, or says why it could not |
| `lightning_peer_connected` | info | `network`, `peer` | A configured peer (`<pubkey>@<ip>:<port>`) completed the handshake |
| `lightning_peer_lost` | info | `network`, `peer`, `connected_secs` | A connected peer went away; it is dialled again after a backoff |
| `lightning_peer_unreachable` | warn | `network`, `peer`, `failures` | Dialling a peer failed: at the 1st, 2nd, 4th, 8th… failure in a row |
| `lightning_synced` | info | `network`, `after_ms`, `nodes`, `channels`, `from_disk` | The graph is complete enough to show ([`LIGHTNING.md`](LIGHTNING.md#sync)) |
| `lightning_summary` | info | `network`, `synced`, `peers`, `nodes`, `channels`, `unchecked`, `pending`, `early`, `rejected`, `lookup_requests`, `lookup_outputs`, `lookup_failures`, `close_height`, `watched`, `catch_up`, `blocks_read`, `close_requests`, `close_failures`, `cooperative`, `force`, `penalty`, `other`, `reorgs`, `messages_per_sec`, `gossip_bytes`, `history_records`, `history_seq`, `index_ms`, `funding_txids` | Once a minute: the graph, validation, the close-watcher, the node load both cause (requests since start: the difference between two lines is the rate), the history stored, the last index build's time and the funding txids learned |
| `lightning_index_built` | info | `network`, `nodes`, `channels`, `copy_ms`, `longest_hold_ms`, `duration_ms` | The first index of the synced graph was built; the node, channel, search and ranking routes answer from now on ([`LIGHTNING.md`](LIGHTNING.md#routes)). Rebuilt every 30 s after that |
| `lightning_funding_failed` | warn | `network`, `error`, `failures` | A funding-txid request to the node failed: at the 1st, 2nd, 4th… in a row; retried after 1 s, doubling to 64 s |
| `lightning_ceiling_reached` | warn | `network`, `channels` | The graph holds `{NET}_LIGHTNING_MAX_CHANNELS`; new channels are refused |
| `lightning_graph_saved` | debug | `network`, `bytes`, `duration_ms` | The graph file was written (every 10 minutes when it changed) |
| `lightning_save_failed` | warn | `network`, `error` | The graph file could not be written; retried at the next period |
| `lightning_pruned` | info | `network`, `channels`, `nodes` | Channels without an update for two weeks left the graph |
| `lightning_lookup_failed` | warn | `network`, `error`, `failures`, `pending` | A funding-output request to the node failed: at the 1st, 2nd, 4th… failure in a row, with the announcements waiting |
| `lightning_watch_failed` | warn | `network`, `error`, `failures` | A close-watcher request to the node failed: at the 1st, 2nd, 4th… in a row |
| `lightning_reorg` | warn | `network`, `fork_height`, `disconnected` | Blocks the close-watcher had read left the best chain; their records were undone |
| `lightning_watch_reset` | warn | `network`, `from`, `to`, `rechecked` | The close-watcher skipped blocks `from` (exclusive) to `to` (a gap of more than ten, or a reorg past its window) and checks every watched output instead |
| `lightning_channel_closed` | info | `network`, `channel`, `kind`, `height`, `capacity`, `fee` | A channel's funding output was spent (`cooperative`, `force`, `other`), or a force close turned out to be a breach (`penalty`, at the sweep's height, without capacity or fee) |
| `lightning_history_gap` | warn | `network`, `missed`, `open` | The history store fell further behind than the peer remembers: `missed` records were never seen. The open channels were stored again from the peer's current set; closes among the missed records are lost ([`STORE.md`](STORE.md#ln_channels)) |
| `lightning_history_failed` | warn | `network`, `error`, `failures`, `waiting` | Storing history records failed (`busy`: the write queue was full; `store`); retried after 1 s, doubling to 32 s. At the 1st, 2nd, 4th… in a row. `shutdown`: the records still `waiting` at shutdown could not be stored in time and are lost |
| `lightning_history_resumed` | info | `network`, `failures` | History records were stored again after failures |
| `lightning_stats_stored` | info | `network`, `time`, `channels`, `nodes`, `capacity`, `skipped`, `rolled_minutes`, `rolled_hours`, `duration_ms` | A ten-minute statistics snapshot was stored, with the slots missed before it and the rows the same write rolled up |
| `lightning_stats_skipped` | info | `network`, `reason`, `skipped` | Snapshots not taken, at the 1st, 2nd, 4th… in a row: `syncing` (the graph is not synced), `late` (the server woke more than a minute after the slot), `store_busy` or `store_failed` |
| `lightning_ldk_record` | debug | `network`, `ldk_level`, `module`, `line`, `peer`, `message` | A line from LDK's own logger, at most 20 a second ([`LIGHTNING.md`](LIGHTNING.md#interface)) |
