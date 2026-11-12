# WebSocket protocol

The live channel between the server and the frontend: one socket per browser tab at `/app/ws`, or under a network's prefix (`/signet/app/ws`, `/testnet4/app/ws`, `/regtest/app/ws`). A socket belongs to one network. This page is the contract the frontend and every topic are written against. The code is `crates/server/src/ws.rs` and `crates/server/src/ws/`.

The socket is part of the private `/app` API: it may change in any release, since the frontend and server ship together. Breaking changes bump the protocol version.

## Frames

| Frame | Carries |
|---|---|
| Text | One JSON object: control messages and small state |
| Binary | A projected block's transactions ([below](#binary-frames)). Byte 0 is a kind code naming the format of the rest, byte 1 that format's version. A client ignores a binary frame whose kind or version it does not know |

A client sends text frames only. The server ignores the `Sec-WebSocket-Protocol` header and negotiates no extensions, so frames are never compressed.

## Messages

Every server message is an object with a `type`. Messages that belong to a topic also carry `topic`; the others are about the connection. New fields may be added to any message: a client ignores fields it does not know.

### Client to server

| Message | Does |
|---|---|
| `{"op":"subscribe","topics":["blocks"]}` | Subscribes to each topic. The server first sends the topic's current state, if it has one, then every change. Subscribing to a topic already subscribed does nothing |
| `{"op":"unsubscribe","topics":["blocks"]}` | Stops the topics. Unknown to this connection is not an error |
| `{"op":"ping"}` | The server answers `{"type":"pong"}`. For a browser, which cannot send WebSocket pings, to check the socket is alive |

Anything else (not JSON, an unknown `op`, a missing or misspelled field, an unknown topic) closes the socket with `4000`.

### Server to client

| Message | When |
|---|---|
| `{"type":"hello","v":1,"network":"mainnet","tip":{"height":975516,"hash":"…"}}` | First, on every connection. `tip` is the node's best block, or `null` before the server has reached the node |
| `{"type":"pong"}` | Answer to `ping` |
| Topic messages | Below |

## Topics

### `blocks`

| Message | Meaning |
|---|---|
| `{"topic":"blocks","type":"block","block":{…}}` | A block joined the best chain. `block` is the node's Esplora JSON for it, exactly as `GET /api/block/:hash` returns it (`id`, `height`, `previousblockhash`, `timestamp`, `tx_count`, `size`, `weight`, …). Also the snapshot sent on subscribe: the current tip's block |
| `{"topic":"blocks","type":"summary","summary":{…}}` | A block's fee facts, exactly as `GET /app/block/:hash/summary` returns them ([`FRONTEND.md`](FRONTEND.md#block-summaries)); `summary.id` names the block. Sent after the `block` message for each new tip, once it is computed (well under a second on mainnet), and for older blocks the server fills in after a gap or at startup. Never a snapshot: a new subscriber fetches the facts of the tiles it shows over HTTP, from the batch route |
| `{"topic":"blocks","type":"reorg","fork_height":N}` | Every block above `N` left the best chain. `block` messages for the new branch follow, in height order |
| `{"topic":"blocks","type":"resync","tip":{"height":…,"hash":"…"}}` | Blocks may have been missed: the server lost and regained the node, or could not fetch a new block within 3 seconds. `tip` is the best block now, or `null`. After a failed fetch it is also the snapshot, until the next `block` |

**Keeping up.** Keep the height and hash of the newest block shown. On `block`:

- `height` is one more and `previousblockhash` is the kept hash: append it.
- `height` is at or below the kept height: a reorg replaced blocks. Drop those at and above `height`, then append.
- Anything else is a gap: fetch the recent blocks over HTTP (`GET /api/blocks`).

On `reorg`, drop blocks above `fork_height`. On `resync`, compare `tip` with what is shown and, if it differs, fetch over HTTP. After a reconnect, `hello` and the subscribe snapshot give the tip again; compare as for `resync`.

A `block` message is about 500 bytes whatever the block's size: transactions are not included. The server fetches each new block's JSON in one loop, whatever the number of subscribers: it asks the node's Esplora every 25 ms until the block is served, for at most 3 seconds (120 requests), then sends `resync` instead. A `summary` message is about 400 bytes. Key it by `summary.id`; one whose block is not shown can be dropped.

### Mempool topics

Fed by the network's mempool tracker and projected blocks ([`MEMPOOL.md`](MEMPOOL.md#in-the-server)). Each topic sends at most one message per `WS_MEMPOOL_INTERVAL_MS` (2 s), always the latest state, so changes in between are merged. Nothing is sent before the tracker's first complete cycle; a subscriber then gets the current state on subscribe, as for `blocks`.

| Topic | For | Format | Size on mainnet |
|---|---|---|---|
| `mempool` | The home page's mempool panel | JSON, the whole state each time | about 200 bytes |
| `projected` | The block strip's projected tiles | JSON, the whole state each time | about 1.4 KB |
| `transactions` | The home page's latest transactions | JSON, the whole list each time | about 1.4 KB |
| `projected-txs:0` to `projected-txs:6` | One projected block's grid | Binary: a snapshot, then deltas | snapshot 48 bytes a transaction; see [Cost](#cost-of-the-mempool-topics) |

With `{NET}_MEMPOOL_ENABLED=false` every one of them answers subscribe with `{"topic":"…","type":"off"}` and nothing else.

Rates are integers in **sat per 1,000 vB**, the node's own unit and resolution (`1234` is 1.234 sat/vB); sizes are sigop-adjusted virtual bytes; amounts are satoshis.

**`mempool`**

```json
{"topic":"mempool","type":"mempool","seq":812,"count":55210,"vsize":41503331,"fees":20125872,
 "min_fee_rate":1000,"bands":[25411234,…10 values],"stale":false}
```

`count`, `vsize` and `fees` are the whole mempool. `min_fee_rate` is the node's `mempoolminfee`, the lowest rate it accepts now (`null` before the first answer). `bands` is the virtual size in each of the ten fee bands of [`DESIGN.md`](DESIGN.md#fee-scale), cheapest first, by each transaction's effective rate: the mempool bar. `stale` is true while the node is failing: the numbers are the last good ones. `seq` is the projection they come from.

**`projected`**

```json
{"topic":"projected","type":"projected","seq":812,"tip":{"height":975530,"hash":"…"},"stale":false,
 "blocks":[{"txs":3214,"weight":3991876,"vsize":997969,"fees":1543210,"min_rate":1012,
            "median_rate":1500,"max_rate":250000,"bands":[0,0,0,0,312000,…]}, … 8 blocks]}
```

Eight blocks, the next first; the last holds everything beyond the other seven (the "+N blocks" tile). `tip` is the block they were projected on. `median_rate` is the rate at the block's middle vbyte; `bands` are its strata. `stale` as above. Key the tiles by position: block 0 is always "next".

**`transactions`**

```json
{"topic":"transactions","type":"transactions",
 "txs":[{"txid":"…","time":1794461221,"fee":381,"vsize":127,"value":4999999619}, …]}
```

The newest `WS_LATEST_TXS` (10) transactions the server has seen, newest first by the node's acceptance `time` (Unix seconds). `value` is the total output value. A transaction stays in the list after it confirms or is replaced; it leaves when newer ones push it out. Rows already shown keep their place: diff by `txid` to find the new ones.

**`projected-txs:N`**

Block `N` of `projected`, 0 to 6. The last block has no topic: it is the rest of the mempool and has no size bound. One socket holds at most `WS_MAX_BLOCK_SUBSCRIPTIONS` (2) of these; switch by subscribing to the new one and unsubscribing from the old. The first message is a snapshot (kind 1), possibly followed at once by up to 16 deltas (kind 2) that bring it to the current state; then a delta whenever the block's transactions change, or a snapshot when that is smaller (a new block usually replaces block 0 whole).

### Binary frames

Integers are little-endian. A txid is 32 bytes in the order it is written in hex, so hex-encoding the bytes gives the usual txid string.

**Record**, 48 bytes, one transaction:

| Offset | Bytes | Field |
|---|---|---|
| 0 | 32 | txid |
| 32 | 4 | effective fee rate, sat per 1,000 vB: the rate of the chunk it is mined in |
| 36 | 4 | sigop-adjusted vsize |
| 40 | 8 | its own fee, satoshis |

**Kind 1, snapshot**: the block's whole state.

| Offset | Bytes | Field |
|---|---|---|
| 0 | 1 | kind, `1` |
| 1 | 1 | version, `1` |
| 2 | 1 | block `N` |
| 3 | 1 | zero |
| 4 | 4 | record count |
| 8 | 8 | `seq`: the projection this is |
| 16 | 48 × count | records, in mining order |

**Kind 2, delta**: changes since the previous frame of this topic.

| Offset | Bytes | Field |
|---|---|---|
| 0 | 1 | kind, `2` |
| 1 | 1 | version, `1` |
| 2 | 1 | block `N` |
| 3 | 1 | zero |
| 4 | 4 | removed count |
| 8 | 8 | `seq`: the projection this brings the block to |
| 16 | 8 | `prev`: the `seq` of the frame it follows |
| 24 | 4 | set count |
| 28 | 4 | zero |
| 32 | 32 × removed | txids that left the block: confirmed, replaced, evicted, or moved to another block |
| … | 48 × set | records that entered the block or changed rate, size or fee |

**Applying them.** Hold, per block, a map from txid to record and the `seq` of the last frame applied.

- On a snapshot, replace the map and the `seq`, whenever it comes.
- On a delta whose `prev` equals the held `seq`: delete each removed txid, then insert or overwrite each set record, then hold the delta's `seq`. The map is now exactly the server's block.
- On a delta whose `prev` is anything else, or before any snapshot, or one that removes a txid the map does not hold: the client has lost its place. Unsubscribe and subscribe again; the snapshot that follows starts it over.

The server never skips a frame for a subscriber: one that cannot keep up is closed with `4003` instead. So a lost place means a client bug or a reconnect without a fresh subscribe, never a normal event. Draw the grid by sorting on rate, highest first; ties have no meaningful order ([`MEMPOOL.md`](MEMPOOL.md#limits)).

**Falling behind.** A slow client is dropped, not resynchronised: its send queue fills (`WS_SEND_QUEUE` frames) and it is closed with `4003`, as on every topic. Resynchronising would mean holding, per client, what it has missed and sending a slow client a snapshot of up to 800 KB on top; dropping it costs nothing, and its reconnect gets one snapshot through the normal path. Frames are shared, so a full queue holds 64 references, not 64 copies.

### Cost of the mempool topics

**Per client, mainnet,** 2026-11-12, one script client on every mempool topic for 943 s, a mempool of 57,000 to 59,000 transactions, read-only against a synced node:

| Topics | Bytes a second | Messages |
|---|---|---|
| `mempool` | 98 | one per 2.1 s |
| `projected` | 654 | one per 2.1 s |
| `transactions` | 671 | one per 2.1 s |
| **The home page, all three** | **1.4 KB/s** | |
| `projected-txs:0` | 321 after a 166 KB snapshot (3,458 transactions) | a delta per 2.1 s |
| `projected-txs:1` | 582 after a 378 KB snapshot (7,880 transactions) | a delta per 2.1 s |

A second run of 1,760 s, which included block 975,526 (3,663 transactions confirmed), gave the same rates (1.4 KB/s for the home page; 278 and 602 B/s for blocks 0 and 1). At the block each projected block's change was larger than its new state, so both topics sent a snapshot instead: 87 KB for block 0 (1,822 transactions) and 270 KB for block 1. A client watching a block pays that once per block. Throughout, our counts matched the node's `getmempoolinfo` to within 10 transactions (the ones that arrived between our cycle and the check), and our block 0 matched the node's template above 99.7% by weight, tie-aware, at every one of 46 checks.

**Server, at scale,** measured locally only: the release build against a regtest node taking 20 new transactions a second (a block every 3 minutes, mempool up to 3,600 transactions), clients in Python on the same machine (Apple M4, shared with other work). One client in ten also watched `projected-txs:0`.

| Clients | Server CPU | Peak RSS | Queued to clients | Slowest fan-out | Dropped |
|---|---|---|---|---|---|
| none | 0.2–0.3% of one core | 14–37 MB | | | |
| 1,000 | 2.2% | 18 MB | | | 0 |
| 5,000 | 8.0% | 64 MB | 7 MB/s | 21 ms | 0 |

CPU and memory grow with the clients, not with the topics: each message is encoded once (in that run 139 KB a minute encoded, 424 MB a minute queued) and each client's queue holds references to the same bytes. With one client on mainnet the whole server, engines included, used 2.8% of one core and 80 MB.

## Connection rules

| Rule | Default | Setting |
|---|---|---|
| Sockets open at once, every network together | 1,024 | `WS_MAX_SOCKETS` |
| Sockets per client address | 16 | `WS_CLIENT_MAX_SOCKETS` |
| Largest message from a client, after reassembling fragments | 1,024 bytes | `WS_MAX_MESSAGE_BYTES` |
| Frames per second from a client, every kind counted; a burst of the same size is allowed | 10 | `WS_MESSAGES_PER_SEC` |
| Topics subscribed at once | 8 | `WS_MAX_SUBSCRIPTIONS` |
| Of those, `projected-txs:N` topics | 2 | `WS_MAX_BLOCK_SUBSCRIPTIONS` |
| Messages per mempool topic | one per 2 s | `WS_MEMPOOL_INTERVAL_MS` |
| Transactions in the `transactions` list | 10 | `WS_LATEST_TXS` |
| Messages queued for a client that is not reading | 64 | `WS_SEND_QUEUE` |
| Silence before the socket is closed; the server pings at half of it | 60 s | `WS_IDLE_TIMEOUT_SECS` |

Over a cap at upgrade time, the request is refused before it becomes a socket: `429` past the per-client cap, `503` past the total. Clients are counted as in [`CONFIG.md`](CONFIG.md#client-addresses) (IPv6 by /64, the trusted proxy header only from a trusted proxy). The upgrade request also passes the ordinary request limits.

**Only this site's pages may open a socket** ([`SECURITY.md`](SECURITY.md) rule 21). A browser always sends `Origin` with an upgrade; the server accepts it only when its host and port equal the request's `Host` (letter case and the scheme's default port ignored), and refuses anything else with `403` `{"error":"websocket refused: Origin is not this site"}` before any socket slot is taken. Without the rule, any website could open sockets from each of its visitors until their per-client cap was reached and lock them out of this one. `Host` is the name the browser used, so every host name the site answers to works without a setting, as long as the reverse proxy passes `Host` through (nginx: `proxy_set_header Host $host`; Caddy does by default). A proxy that rewrites `Host` to the server's own address makes every browser socket fail: the refusal is an `http_request` with `refused: "origin"` and a `ws_origin_refused` warning naming the origin and host seen ([`LOGGING.md`](LOGGING.md#websocket-hub)). An upgrade without `Origin` is a program, not a browser page, and is allowed: no third party can make a visitor's browser send one.

Browsers answer pings on their own, so a live tab is never closed for silence. A client may send `{"op":"ping"}` every 30 seconds or so to learn that a socket died without a close.

## Close codes

| Code | Reason text | Why |
|---|---|---|
| 1000 | `closed by client` | Answer to the client's close |
| 1001 | `server shutting down` | Shutdown. Reconnect with backoff |
| 1002 | `protocol error` | Bad framing: unmasked, reserved bits set, unknown opcode, a fragmented or oversized control frame, a continuation out of place, a close frame with a status an endpoint may not send |
| 1003 | `binary messages are not accepted` | The client sent a binary message |
| 1007 | `invalid utf-8` | A text message, or a close frame's reason, that is not UTF-8 |
| 1009 | `message too big` | A message over `WS_MAX_MESSAGE_BYTES`, refused from the frame header before the payload is read |
| 4000 | `bad message` | Not a message listed above |
| 4001 | `too many subscriptions` | Over `WS_MAX_SUBSCRIPTIONS` or `WS_MAX_BLOCK_SUBSCRIPTIONS` |
| 4002 | `too many messages` | Over `WS_MESSAGES_PER_SEC` |
| 4003 | `too slow` | The send queue was full: the client is not reading |
| 4004 | `idle timeout` | Nothing received for `WS_IDLE_TIMEOUT_SECS`, ping unanswered |

A client closed for 1002 to 4002 has a bug; reconnecting will not help. After 4003 or 4004, reconnect with backoff. The close frame is written after any frame in progress and the queued messages are dropped. When a client has stopped reading, the close frame may never reach it: the write gives up after `REQUEST_TIMEOUT_SECS` without progress and the connection is dropped.

## How it works

- **Fan-out.** A topic message is encoded once into a complete WebSocket frame (header and payload) held behind a reference count. Sending it to a subscriber puts a reference on that subscriber's bounded queue; the subscriber's writer puts the same bytes on the wire. Nothing is copied or encoded per connection.
- **Cost per connection** is one task, its queue and its subscriptions: about 5 KB of server memory measured with 5,000 open sockets, plus kernel socket buffers. Memory grows linearly with connections. Queued messages are shared, so a full queue costs 64 references, not 64 copies.
- **Cost per message** is one `O(subscribers)` pass of non-blocking queue pushes under the topic's lock, then one write per subscriber on its own task. Measured: about 2 ms to queue a block to 5,000 sockets, and about 35 ms until 5,000 local clients had all read it.
- **Upstream work is shared and capped.** No client message reaches the node. The only upstream calls are the block fetch for each new block, made by one task per network (normally one request, at most 120), the block's summary, computed once by the summary routes' own capped worker, and the mempool engine's polling, which runs whatever the number of subscribers. The full fetch is also the existence check, since a missing block is a short 404 and a separate check would add a round trip once the block is there. `hello` and the snapshots come from memory.
- **Mempool topics** are produced by one task per network that reads the latest projection, compares each block with what it last published, and encodes each message once. A topic's stored state for new subscribers is its last snapshot plus the deltas appended since; after 16 deltas, or when they outweigh the snapshot, they are folded into a fresh snapshot, which is stored but sent to nobody.
- **Shutdown.** Every socket is sent `1001` and given up to `REQUEST_TIMEOUT_SECS`, counted from the signal, to finish, together with the HTTP connections. Sockets still open then are dropped, and the server waits for them to release their slots.
- **Behind TLS.** The reverse proxy must pass `Upgrade` and `Connection` through (nginx: `proxy_http_version 1.1`, `proxy_set_header Upgrade $http_upgrade`, `proxy_set_header Connection "upgrade"`), pass `Host` through for the origin check above and keep idle connections open longer than `WS_IDLE_TIMEOUT_SECS / 2`, the ping interval. The page connects to `wss://` on its own host, which the Content-Security-Policy's `default-src 'self'` allows in current Chromium and Firefox. Older WebKit did not match `ws:`/`wss:` with `'self'` ([WebKit bug 201591](https://bugs.webkit.org/show_bug.cgi?id=201591)); if a Safari user cannot connect, that is the cause.

## Conformance

The server side of RFC 6455 (handshake, framing, masking, fragmentation, control frames, close, SHA-1 and base64 for the accept key) is our own code in `ws/frame.rs` and `ws.rs`, about 300 lines in place of a library and its 22 crates. Two checks hold it to the standard:

- **[Autobahn TestSuite](https://github.com/crossbario/autobahn-testsuite)**, the standard conformance suite, run by `dev/autobahn/autobahn.py`. It builds the server with the `autobahn` feature, which makes the hub echo text messages and skip `hello`, raises the size, rate and queue limits to the most the config allows, and runs every case in Docker against it. It takes about a minute, too slow for CI: rerun it after changing `ws/`.
- **A fuzz test** in the normal test suite (`the_parser_survives_random_and_mutated_streams`) feeds 50,000 generated byte streams through the reader: valid and invalid frames, truncation, bit flips, absurd and non-minimal lengths, missing masks, interleaved control frames and fragments that never end. Each must end in a defined close code, and no payload buffer may exceed the message limit (or 125 bytes for a control frame). It takes about 0.6 s.

Autobahn results, 2026-11-12, every case:

| Cases | OK | Non-strict | Informational | Unimplemented | Failed |
|---|---|---|---|---|---|
| 1 Framing | 8 | | | | 8 |
| 2 Ping and pong | 11 | | | | |
| 3 Reserved bits | 5 | 2 | | | |
| 4 Opcodes | 6 | 4 | | | |
| 5 Fragmentation | 19 | 1 | | | |
| 6 UTF-8 | 141 | 4 | | | |
| 7 Close | 34 | | 3 | | |
| 9 Large messages | 15 | | | | 39 |
| 10 Auto-fragmentation | 1 | | | | |
| 12, 13 Compression | | | | 216 | |
| **All** | **240** | **11** | **3** | **216** | **47** |

No case shows a protocol violation. The rest, and why each stays:

| Result | Cases | Why |
|---|---|---|
| Failed | 1.2.*, and the binary cases of 9.* | Binary messages are refused with `1003` by design; the suite expects them echoed |
| Failed | The 9.* cases over 1 MiB, the most `WS_MAX_MESSAGE_BYTES` allows | Refused with `1009` from the frame header. When the suite is still sending (8 and 16 MiB), the server has stopped reading and drops the connection, so the suite never sees the close frame. RFC 6455 section 7.1.7 lets an endpoint that fails a connection stop processing data |
| Non-strict | 3.2, 3.3, 4.1.3, 4.1.4, 4.2.3, 4.2.4, 5.15 | A valid message, then a broken frame. The connection closes with `1002` without echoing the valid message first, because the close frame goes ahead of queued messages. Strict order would mean flushing a queue that a slow client may have filled |
| Non-strict | 6.4.1–6.4.4 | Invalid UTF-8 in a fragmented message is caught when the message is complete, not at the fragment that broke it. The message is capped at `WS_MAX_MESSAGE_BYTES`, so the cost is reading at most that much |
| Informational | 7.1.6, 7.13.1, 7.13.2 | Behaviour the RFC leaves open: a close handled before a large earlier echo went out, and close codes 5000 and above answered with `1002` |
| Unimplemented | 12.*, 13.* | Compression (`permessage-deflate`), which the server never negotiates |

## Adding a topic

1. Add a variant to `Topic` in `crates/server/src/ws.rs` and its name to `Topic::parse` and `Topic::name`. A topic with a parameter (a tracked transaction) takes it as a parsed value in the variant, never a raw string.
2. Produce its messages in one task per network, as `feed` in `ws/hub.rs` does for `blocks` and `ws/mempool.rs` for the mempool topics: build the JSON with `serde_json` (or a binary payload with `Frame::encode`), then call `Hub::announce` (or `Hub::publish`) once per change. `Snapshot::Replace` makes the message the topic's current state for new subscribers; `Clear` and `Keep` are for messages that invalidate it or leave it alone; `Append` is for a change new subscribers must also get after the state, and `Hub::set_snapshot` folds those into a new state without sending it. `announce` logs a `ws_fanout` line per message: use `publish` and a periodic summary for a topic that sends every few seconds.
3. If the topic needs upstream work per subscribed value, start it on the first subscriber and stop it on the last, through the shared, capped Electrum subscriptions ([`SECURITY.md`](SECURITY.md) rule 10), and add its own per-connection cap. A topic family with a parameter that costs more to send (`projected-txs:N`) also gets a per-connection cap.
4. Rate-limit a topic whose source changes often, and keep each message bounded in size.
5. Document it here under Topics, with how a client recovers from a missed message, and its cost.
