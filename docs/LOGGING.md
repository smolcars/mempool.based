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

**Logging never blocks or crashes the server.** `emit` formats the line on the caller's thread and puts it on a bounded queue (4,096 lines) drained by one writer thread. When the queue is full the event is dropped and counted, and a `log_events_dropped` event with the count is written once the queue drains. A write error is ignored. At exit the server waits at most two seconds for the queue to empty.

## Settings

`LOG_FORMAT`, `LOG_LEVEL`, `LOG_REQUESTS` and `LOG_CLIENT_ADDR` are in [`CONFIG.md`](CONFIG.md#logging). A passing `/healthz` is never logged, whatever `LOG_REQUESTS` says, so health checks do not flood the log.

### Why client addresses are off by default

A request event already says when someone used which kind of page or API route. With the client address it becomes a per-person activity record, and together with the timing it can often be matched back to what was looked up. The operator did not ask for that record, and anyone who later reads the log (a backup, a log service, a subpoena) gets it too. The limits that stop abuse work on live counters and do not need the log. An operator investigating abuse can turn `LOG_CLIENT_ADDR` on for the duration, or use the access log of the reverse proxy in front of the server, which most deployments already keep under their own retention policy.

## Request ids

Every response carries `X-Request-Id`, a 16-digit hex id that also appears in its `http_request` event, so a user's report can be matched to one line. It is exposed to cross-origin callers of `/api`. Ids are unique within a process and unguessable; a client cannot set its own.

## Adding an event

1. Declare a struct in `crates/logging/src/events.rs` with `#[derive(Debug, Serialize)]` and the fields it needs, following the conventions above. Borrow strings (`&'a str`) rather than allocating.
2. Give it a name and level on the line after: `event!(UpstreamSlow, "upstream_slow", Warn);`
3. Add a sample to `samples()` in the same file's tests, and a row to the catalogue below. Tests fail until both exist.
4. Emit it where the thing happens: `logging::emit(&UpstreamSlow { network, endpoint, duration_ms });`

## Catalogue

### Lifecycle

| Event | Level | Fields | When |
|---|---|---|---|
| `server_starting` | info | `version`, `listen_addr`, `web_dir`, `networks`, `log_format`, `log_level`, `log_requests`, `log_client_addr`, `broadcast_enabled`, every limit and backfill setting from [`CONFIG.md`](CONFIG.md) by its variable's name in lower case (`max_body_bytes`, `request_timeout_secs`, `backfill_busy_percent`, …), `trusted_proxy_header` and `data_dir` if set | Configuration parsed, before listening. Never the RPC user or password |
| `network_enabled` | info | `network`, `rpc_addr`, `esplora`, `electrum_addr`, `mempool` | Once per enabled network at startup. `mempool` is whether its mempool engine runs (`{NET}_MEMPOOL_ENABLED`) |
| `no_networks` | warn | none | No network is configured |
| `web_dir_missing` | warn | `web_dir` | `WEB_DIR` has no `index.html` |
| `listening` | info | `addr` | The listener is bound; shows the real port when asked for port 0 |
| `shutdown_requested` | info | `signal` (`sigterm` or `ctrl_c`) | A shutdown signal arrived |
| `server_stopped` | info | none | In-flight requests are done and the process exits normally |
| `config_invalid` | error | `error` | Invalid configuration; the process exits. The message names the variable, never its value |
| `listen_failed` | error | `addr`, `error` | The listen address could not be bound; the process exits |
| `panic` | error | `message`, `location`, `thread` | Any panic. In a request handler the client gets a 500 and the server keeps running |
| `log_events_dropped` | warn | `count` | The log queue was full; written once it drains |

### HTTP

| Event | Level | Fields | When |
|---|---|---|---|
| `http_request` | info | `request_id`, `network`, `method`, `route`, `status`, `duration_ms`, `bytes`, `refused`, `endpoint`, `source`, `upstream_ms`, `upstream_error`, `client` | Each request, when its response head is ready, subject to `LOG_REQUESTS` |

- `route` is the matched route template (`/signet/api/{*rest}`, `/healthz`), or `frontend` for pages and static files.
- `network` is left out for `/healthz`, `/assets` and unknown network prefixes.
- `duration_ms` runs until the response head is ready; streaming the body is not included. `bytes` is the body size when known up front.
- `refused` names the limit that said no: `client_rate`, `client_concurrency`, `client_table_full`, `server_busy`, `timeout`, `route_rate` (a route's own stricter limit, such as broadcast or one client's uncached block summaries), `summary_busy` (the network's block summary queue is full), or for a WebSocket upgrade `client_sockets` (`WS_CLIENT_MAX_SOCKETS`) and `server_sockets` (`WS_MAX_SOCKETS`). A `413` is an oversized body.
- `endpoint`, `source`, `upstream_ms` and `upstream_error` are set for `/api` requests that reached the pass-through ([`API.md`](API.md)), for the block summary routes and for the mining routes: the named route (`tx_status`, `block_summary`, `block_summary_txs`, `block_summaries`, `mining_pools`, `mining_hashrate`, …), where the answer came from (`upstream`, `coalesced` onto another request's call, `hot` from the short cache, `cached` from the block summary cache or a mining answer already computed, or `stored` from the block table), this request's own upstream time, and why the upstream call got no answer (`timeout`, `unavailable`, `response_too_big`, `decode`, or `busy` when the node's RPC queue was full).
- `client` only with `LOG_CLIENT_ADDR=true`.

### Node clients

| Event | Level | Fields | When |
|---|---|---|---|
| `electrum_connected` | info | `network`, `server`, `protocol` | The Electrum connection is up, subscriptions restored. `server` is the node's own software string |
| `electrum_disconnected` | warn | `network` | A ready connection was lost; the client reconnects with backoff |
| `electrum_unreachable` | warn | `network`, `attempts` | Connection attempts keep failing: at the 1st, 2nd, 4th, 8th… in a row |
| `electrum_resynced` | info | `network` | Updates may have been missed (reconnect, or a tip jump too far to follow) |
| `electrum_lines_skipped` | warn | `network`, `total` | The node sent lines that are not valid JSON-RPC; at a total of 1, 2, 4, 8… |
| `chain_reorg` | warn | `network`, `fork_height` | Blocks above `fork_height` left the best chain |

`crates/server/src/report.rs` produces these from the client's `health()` watch and event stream; call `report_electrum` once where each network's Electrum client is created. The Esplora and RPC clients return their failures as error values (`rpc::Error::Busy` is the node's back-pressure); the code that calls them owns the event, summarised by rule 3.

### WebSocket hub

The protocol and its close codes are in [`WS.md`](WS.md).

| Event | Level | Fields | When |
|---|---|---|---|
| `ws_fanout` | info | `network`, `topic`, `kind`, `subscribers`, `open`, `bytes`, `dropped_slow`, `duration_ms`, `fanout_ms` | One message went to every subscriber of a `blocks` topic: once per new block, block summary, reorg or resync per network, whatever the client count. The mempool topics, which send every few seconds, are summed in `mempool_summary` instead. `duration_ms` runs from the node's notification to the last subscriber's queue (the block fetch included); `fanout_ms` is the queueing part. `open` counts sockets on every network |
| `ws_clients_dropped` | warn | `network`, `reason`, `total` | The server closed clients for breaking a limit: `too_slow`, `idle`, `rate`, `too_big`, `bad_message`, `too_many_subscriptions`, `protocol_error`, `binary` or `invalid_utf8`. At a total of 1, 2, 4, 8… per reason and network |
| `ws_closed` | debug | `network`, `code`, `reason`, `duration_ms`, `open` | Every socket close: its close code (1006 when the connection dropped without one), the reason label (those above, or `client_closed`, `shutdown`, `connection_lost`), how long it was open, and the sockets still open |
| `block_unavailable` | warn | `network`, `height`, `hash`, `failures` | A new block's JSON could not be fetched from Esplora within 3 seconds of retrying, so subscribers were sent `resync`. At the 1st, 2nd, 4th, 8th… failure in a row |

### Block summaries

The routes are in [`FRONTEND.md`](FRONTEND.md#block-summaries). Each request is also an `http_request` with `endpoint` and `source`.

| Event | Level | Fields | When |
|---|---|---|---|
| `block_summary_computed` | debug | `network`, `height`, `txs`, `block_bytes`, `bytes`, `node_ms`, `duration_ms`, `cause` | A summary was computed: the block's transactions and size, the records kept, the `getblock` call's time and the whole time including the wait for a slot. `cause` is `request` or `new_block`. Debug, since a busy server computes a few a second |
| `block_summary_failed` | warn | `network`, `error`, `failures` | The node gave no usable answer (`busy`, `timeout`, `unavailable`, `response_too_big`, `decode`). At the 1st, 2nd, 4th, 8th… failure in a row. An unknown block is not a failure |
| `block_summaries_recovered` | info | `network`, `failures` | A summary was computed after `failures` in a row |
| `block_summaries_refused` | warn | `network`, `reason`, `total` | Requests for uncached blocks were refused: `summary_busy` (the network's queue was full) or `route_rate` (one client over its share). At a total of 1, 2, 4, 8… per reason and network |

Opening a socket is an `http_request` with route `/app/ws` (or the network's prefix) and status `101`; a refused upgrade is the same event with `429` or `503` and `refused` naming the cap.

### The live mempool

How the tracker and projected blocks run in the server is in [`MEMPOOL.md`](MEMPOOL.md#in-the-server). A healthy network writes `mempool_synced` once and then one `mempool_summary` a minute; everything else is an abnormal condition.

| Event | Level | Fields | When |
|---|---|---|---|
| `mempool_synced` | info | `network`, `duration_ms`, `txs`, `bytes`, `calls` | The tracker's first complete cycle: the initial sync, failed attempts included in the time |
| `mempool_summary` | info | `network`, `txs`, `cycles`, `skipped`, `added`, `confirmed`, `replaced`, `evicted`, `vanished`, `node_calls`, `node_bytes`, `cycle_ms_max`, `updates`, `cluster_calls`, `clusters_asked`, `cluster_bytes`, `pack_ms_max`, `stale`, `ws_messages`, `ws_bytes`, `ws_sent_bytes`, `ws_dropped_slow`, `fanout_ms_max` | Once a minute per network while the tracker is not failing: the minute's tracker cycles and their node cost, the projector's updates and cluster requests, and the mempool topics' messages (`ws_bytes` encoded once; `ws_sent_bytes` that times the subscribers). `txs` and `stale` are the state now |
| `mempool_poll_failed` | warn | `network`, `error`, `failures`, `retry_in_ms` | A tracker cycle failed and nothing was published: at the 1st, 2nd, 4th, 8th… failure in a row. `error` is `busy`, `timeout`, `unavailable`, `response_too_big`, `decode`, `auth` or `failed` |
| `mempool_recovered` | info | `network`, `failures` | A cycle succeeded after `failures` in a row |
| `mempool_sequence_regressed` | warn | `network`, `from`, `to` | The node's `mempool_sequence` went backwards: it restarted |
| `projection_fetch_failed` | warn | `network`, `error`, `failures`, `pending`, `retry_in_ms` | A cluster request failed; the projected blocks are marked stale. At the 1st, 2nd, 4th, 8th… failure in a row. `error` as above |
| `projection_recovered` | info | `network`, `failures` | The projected blocks are current again after failed requests |
| `projection_resynced` | warn | `network`, `txs` | The projector fell behind the tracker and rebuilt from a snapshot |
| `mempool_subscriber_lagged` | warn | `network`, `source`, `missed`, `total` | A reader of the `tracker`'s or the `projector`'s change stream fell behind and started over. At a total of 1, 2, 4, 8… per source and network |

### Derived data

| Event | Level | Fields | When |
|---|---|---|---|
| `difficulty_computed` | info | `network`, `height`, `next_retarget_height`, `difficulty_change`, `duration_ms` | The difficulty-adjustment answer ([`API.md`](API.md#difficulty-adjustment)) was recomputed for a new tip: once per block or reorg per network. `difficulty_change` is the expected change in percent at that moment; `duration_ms` is the node fetches |
| `difficulty_refresh_failed` | warn | `network`, `height`, `error`, `failures` | It could not be recomputed for the tip at `height`, so the last answer is still served: at the 1st, 2nd, 4th, 8th… failure in a row. `error` is `timeout`, `unavailable`, `response_too_big`, `decode`, `not_found`, `tip_moved` (Esplora answered for another tip than the one being computed: a block or reorg in between; retried), `wrong_height` or `invalid_bits`. The next `difficulty_computed` is the recovery |

### Store and block table

The store is in [`STORE.md`](STORE.md); the block table's settings in [`CONFIG.md`](CONFIG.md#block-table).

| Event | Level | Fields | When |
|---|---|---|---|
| `store_open_failed` | error | `network`, `error` | The network's SQLite file would not open (unwritable `DATA_DIR`, a corrupt file, a file from a newer build). That network runs without a store |
| `store_migration_applied` | info | `network`, `migration`, `duration_ms` | A schema migration ran while opening |
| `store_slow_write` | warn | `network`, `duration_ms`, `queue_depth` | One write held the writer for a second or more, with `queue_depth` writes waiting |
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
