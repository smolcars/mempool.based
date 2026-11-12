# Lightning

The Lightning explorer shows the public channel graph: nodes, channels, capacity and their history. It does this with a **gossip-only peer** built from LDK's `lightning` crates, running inside the server.

## What it is not

It is not a Lightning node in the usual sense. It never opens a channel, holds no funds and has no wallet. Its only key is a node identity with nothing behind it.

We do not run LND or Core Lightning (a second daemon with a wallet and channel database, polled for graph dumps), and we do not use `ldk-node` (it bundles an on-chain wallet and channel manager we would never use).

## How it works

1. **Connect.** The peer connects to a small set of well-connected public Lightning nodes. Channel requests from peers are refused.
2. **Sync.** Peers send the full graph, then live updates. LDK verifies every signature and maintains the network graph.
3. **Validate.** For each announced channel, LDK asks whether the funding output exists, is unspent and pays the announced script. We answer from rbitcoin's position routes ([`RBITCOIN.md`](RBITCOIN.md)), which also give the capacity.
4. **Track closes.** Every new block's inputs are checked against the set of known funding outputs. A match is a channel close, and the spending transaction's shape classifies it: cooperative, forced or penalty.
5. **Store.** Nodes, channels, open and close records and periodic network totals go to SQLite. LDK's own serialized graph is kept for fast restarts.

## Pieces of LDK used

| Piece | Role |
|---|---|
| `PeerManager` with `lightning-net-tokio` | Connections and the wire protocol |
| `ErroringMessageHandler` | Refuses channel messages |
| `P2PGossipSync` and `NetworkGraph` | Gossip handling and the graph |
| `UtxoLookup` | Our hook for funding-output validation |

Verified to exist in `lightning` 0.2.8.

## Bounds

Gossip is untrusted input ([`SECURITY.md`](SECURITY.md)).

- Funding-output lookups are deduplicated, batched (up to 300 per request) and rate-limited, so a flood of announcements cannot flood the node.
- Aliases, colors and addresses are stored and served as plain data, and rendered as text.
- The peer makes outbound connections only.

## Limits

- **History starts when the peer starts.** Channels that closed before then are not in gossip.
- **Private channels are invisible,** as on every explorer.
- **No world map.** That needs a GeoIP database and is out of scope.

## Switch

Lightning is optional. A network runs without it when it is not enabled.
