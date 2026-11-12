# WebSocket protocol

The live channel between the server and the frontend: one socket per browser tab at `/app/ws`, or under a network's prefix (`/signet/app/ws`, `/testnet4/app/ws`, `/regtest/app/ws`). A socket belongs to one network. This page is the contract the frontend and every topic are written against. The code is `crates/server/src/ws.rs` and `crates/server/src/ws/`.

The socket is part of the private `/app` API: it may change in any release, since the frontend and server ship together. Breaking changes bump the protocol version.

## Frames

| Frame | Carries |
|---|---|
| Text | One JSON object: control messages and small state |
| Binary | Reserved for projected-block data (a later task). Byte 0 is a kind code naming the format of the rest; version 1 defines none. A client ignores a binary frame whose kind it does not know |

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
| `{"topic":"blocks","type":"reorg","fork_height":N}` | Every block above `N` left the best chain. `block` messages for the new branch follow, in height order |
| `{"topic":"blocks","type":"resync","tip":{"height":…,"hash":"…"}}` | Blocks may have been missed: the server lost and regained the node, or could not fetch a new block. `tip` is the best block now, or `null`. After a failed fetch it is also the snapshot, until the next `block` |

**Keeping up.** Keep the height and hash of the newest block shown. On `block`:

- `height` is one more and `previousblockhash` is the kept hash: append it.
- `height` is at or below the kept height: a reorg replaced blocks. Drop those at and above `height`, then append.
- Anything else is a gap: fetch the recent blocks over HTTP (`GET /api/blocks`).

On `reorg`, drop blocks above `fork_height`. On `resync`, compare `tip` with what is shown and, if it differs, fetch over HTTP. After a reconnect, `hello` and the subscribe snapshot give the tip again; compare as for `resync`.

A `block` message is about 500 bytes whatever the block's size: transactions are not included. The server fetches each new block's JSON once, whatever the number of subscribers.

## Connection rules

| Rule | Default | Setting |
|---|---|---|
| Sockets open at once, every network together | 1,024 | `WS_MAX_SOCKETS` |
| Sockets per client address | 8 | `WS_CLIENT_MAX_SOCKETS` |
| Largest message from a client, after reassembling fragments | 1,024 bytes | `WS_MAX_MESSAGE_BYTES` |
| Frames per second from a client, every kind counted; a burst of the same size is allowed | 10 | `WS_MESSAGES_PER_SEC` |
| Topics subscribed at once | 8 | `WS_MAX_SUBSCRIPTIONS` |
| Messages queued for a client that is not reading | 64 | `WS_SEND_QUEUE` |
| Silence before the socket is closed; the server pings at half of it | 60 s | `WS_IDLE_TIMEOUT_SECS` |

Over a cap at upgrade time, the request is refused before it becomes a socket: `429` past the per-client cap, `503` past the total. Clients are counted as in [`CONFIG.md`](CONFIG.md#limits) (IPv6 by /64, the trusted proxy header when one is set). The upgrade request also passes the ordinary request limits.

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
| 4001 | `too many subscriptions` | Over `WS_MAX_SUBSCRIPTIONS` |
| 4002 | `too many messages` | Over `WS_MESSAGES_PER_SEC` |
| 4003 | `too slow` | The send queue was full: the client is not reading |
| 4004 | `idle timeout` | Nothing received for `WS_IDLE_TIMEOUT_SECS`, ping unanswered |

A client closed for 1002 to 4002 has a bug; reconnecting will not help. After 4003 or 4004, reconnect with backoff. The close frame is written after any frame in progress and the queued messages are dropped. When a client has stopped reading, the close frame may never reach it: the write gives up after `REQUEST_TIMEOUT_SECS` without progress and the connection is dropped.

## How it works

- **Fan-out.** A topic message is encoded once into a complete WebSocket frame (header and payload) held behind a reference count. Sending it to a subscriber puts a reference on that subscriber's bounded queue; the subscriber's writer puts the same bytes on the wire. Nothing is copied or encoded per connection.
- **Cost per connection** is one task, its queue and its subscriptions: about 5 KB of server memory measured with 5,000 open sockets, plus kernel socket buffers. Memory grows linearly with connections. Queued messages are shared, so a full queue costs 64 references, not 64 copies.
- **Cost per message** is one `O(subscribers)` pass of non-blocking queue pushes under the topic's lock, then one write per subscriber on its own task. Measured: about 2 ms to queue a block to 5,000 sockets, and about 35 ms until 5,000 local clients had all read it.
- **Upstream work is shared and capped.** No client message reaches the node. The only upstream call is one Esplora block fetch per new block, made by one task per network. `hello` and the snapshots come from memory.
- **Shutdown.** Every socket is sent `1001` and given up to `REQUEST_TIMEOUT_SECS`, counted from the signal, to finish, together with the HTTP connections. Sockets still open then are dropped, and the server waits for them to release their slots.
- **Behind TLS.** The reverse proxy must pass `Upgrade` and `Connection` through (nginx: `proxy_http_version 1.1`, `proxy_set_header Upgrade $http_upgrade`, `proxy_set_header Connection "upgrade"`) and keep idle connections open longer than `WS_IDLE_TIMEOUT_SECS / 2`, the ping interval. The page connects to `wss://` on its own host, which the Content-Security-Policy's `default-src 'self'` allows in current Chromium and Firefox. Older WebKit did not match `ws:`/`wss:` with `'self'` ([WebKit bug 201591](https://bugs.webkit.org/show_bug.cgi?id=201591)); if a Safari user cannot connect, that is the cause.

## Adding a topic

1. Add a variant to `Topic` in `crates/server/src/ws.rs` and its name to `Topic::parse` and `Topic::name`. A topic with a parameter (a tracked transaction) takes it as a parsed value in the variant, never a raw string.
2. Produce its messages in one task per network, as `feed` in `ws/hub.rs` does for `blocks`: build the JSON with `serde_json`, then call `Hub::announce` (or `Frame::text` and `Hub::publish`) once per change. `Snapshot::Replace` makes the message the topic's current state for new subscribers; `Clear` and `Keep` are for messages that invalidate it or leave it alone.
3. If the topic needs upstream work per subscribed value, start it on the first subscriber and stop it on the last, through the shared, capped Electrum subscriptions ([`SECURITY.md`](SECURITY.md) rule 10), and add its own per-connection cap.
4. Document it here under Topics, with how a client recovers from a missed message.
