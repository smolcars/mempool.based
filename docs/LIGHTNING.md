# Lightning

The Lightning explorer shows the public channel graph: nodes, channels, capacity and their history. It does this with a **gossip-only peer** built from LDK's `lightning` crates, running inside the server.

## What it is not

It is not a Lightning node in the usual sense. It never opens a channel, holds no funds and has no wallet. Its only key is a node identity with nothing behind it.

We do not run LND or Core Lightning (a second daemon with a wallet and channel database, polled for graph dumps), and we do not use `ldk-node` (it bundles an on-chain wallet and channel manager we would never use).

## How it works

1. **Connect.** The peer connects out to a small set of well-connected public Lightning nodes. Channel requests from peers are refused.
2. **Sync.** Peers send the full graph, then live updates. LDK verifies every signature and maintains the network graph.
3. **Validate.** *Not built yet: until then the graph is unverified* ([below](#unverified-until-validation)). For each announced channel, LDK asks whether the funding output exists, is unspent and pays the announced script. We answer from rbitcoin's position routes ([`RBITCOIN.md`](RBITCOIN.md)), which also give the capacity.
4. **Track closes.** *Not built yet.* Every new block's inputs are checked against the set of known funding outputs. A match is a channel close, and the spending transaction's shape classifies it: cooperative, forced or penalty.
5. **Store.** *Not built yet.* Nodes, channels, open and close records and periodic network totals go to SQLite. LDK's own serialized graph is kept for fast restarts (built: see [Files](#files)).

Steps 1, 2 and the graph file are the `ln` crate (`crates/ln`). It is a library: it does not log and is not wired into the server yet.

## Pieces of LDK used

| Piece | Role |
|---|---|
| `PeerManager::new_routing_only` with `lightning-net-tokio` | Connections and the wire protocol, outbound only |
| `ErroringMessageHandler` | The channel handler: answers every channel message, `open_channel` included, with an error. There is no channel manager |
| `P2PGossipSync` and `NetworkGraph` | Signature checks and the graph, behind our own routing handler (below) |
| `UtxoLookup` | The hook for funding-output validation. Left unset |
| `KeysManager` | Derives the node identity from the seed file; nothing else is signed |

`lightning` 0.2.8 (the newest release; 0.3 is a release candidate) and `lightning-net-tokio` 0.2.0, the newest 0.2 release of that crate.

Our routing handler wraps `P2PGossipSync`. It passes gossip through to LDK and counts it, enforces the [bounds](#bounds), never relays (it tells the peer manager not to forward, and drops LDK's broadcast requests), never serves the graph (no initial dump to peers; a `query_channel_range` gets an empty reply, as the spec requires), and adds the channel-range sync below.

## The gossip peer

### Sync

LDK asks each peer for gossip with a `gossip_timestamp_filter` set two weeks back, which Core Lightning and most LND nodes answer with their whole graph. On mainnet the default peers did not: eclair (ACINQ) and the Bitfinex LND nodes sent only new gossip. So the peer also asks every peer that supports gossip queries for every channel id it knows (`query_channel_range`), and requests the channels it lacks (`query_short_channel_ids`), 2,000 ids per batch, one batch at a time per peer and never the same channel from two peers at once. A peer's channel list is complete when a reply reaches the end of the queried range (the `sync_complete` flag is not an end marker), or when its replies stop for a minute. A batch that gets no answer within 2 minutes is dropped; its channels come back at the next range sync, which happens on every connection.

Peers' graphs differ: alone, ACINQ listed 21,600 channels and Bitfinex 30,250. Asking every peer gives the union.

**"Synced"** (`Status::synced`, `Event::Synced`) means the graph is complete enough to show: every connected peer's channel list has arrived, every channel on them has been received or given up on, and no new channel has arrived for 30 seconds, or the graph was loaded from a file caught up within the last 13 days.

The graph file's modification time is set to the last moment the peer was **caught up**: connected, with every listed channel requested and no new channel for 30 seconds. It is not the time of writing, so a graph saved after a week offline does not pass for fresh. After a restart the timestamp filter starts one day before that moment instead of two weeks back, so peers that honour it resend only what is new.

### Peers

Peers come only from configuration, never from gossip, so gossip cannot choose where the server connects ([`SECURITY.md`](SECURITY.md) rule 1). `Config::max_peers` (3) are kept connected. A failed or short-lived connection (under a minute) counts as a failure; a peer is retried after 2 s, doubling per failure up to 10 minutes. The next peer to dial is the due one with the fewest failures, in list order, so a peer that stays unreachable gives its slot to the next one in the list.

Default mainnet peers, all `<pubkey>@<ip>:9735`-style clearnet addresses:

| Node | Why |
|---|---|
| ACINQ | Runs eclair; long-lived; answers range queries at full speed (its whole graph in about 20 s) |
| Bitfinex `bfx-lnd0`, `bfx-lnd1` | Long-lived exchange nodes (LND, by their names); two keys on one host |
| CoinGate | Long-lived payment processor |
| Bitstamp | Exchange |
| NiceHash | Exchange |

They were picked from the live graph on 2026-11-12: the best-connected nodes that announce a clearnet address and are run by companies that have operated Lightning nodes for years, with the address in the list matching the node's own announcement. Several nodes picked from memory (Kraken, River, Wallet of Satoshi, Boltz) were unreachable at their old addresses and were dropped. Only the first three are dialled while they stay up. An operator replaces the list through `Config::peers` (the server will read it from a variable when the peer is wired in). On other networks there are no defaults.

**Tor-only nodes are out of reach:** the peer makes plain TCP connections and does not use Tor.

### Files

Both live in `Config::data_dir`, which the server will put beside each network's SQLite file.

| File | Holds | Size |
|---|---|---|
| `node_key` | The 32-byte identity seed. Created on first run from `/dev/urandom` with mode `0600`; a seed readable by group or others is refused at start | 32 bytes |
| `graph` | LDK's own serialization of the network graph | about 31 MB at 30,000 channels |

The graph is written every 10 minutes when it changed (a failed write is retried at the next period), and at shutdown, through a temporary file that is synced, renamed over the old one and followed by a sync of the directory, so a crash leaves the old file or the new one, never half of one. Writing it takes about 0.1 s. At start a file for another network, one that does not decode, one larger than 8 KB per channel of the ceiling (the most one channel's gossip can occupy, with its nodes), or one holding more channels than the ceiling (saved under a higher one), is reported (`GraphLoad::Unreadable`) and replaced at the next save.

### Interface

For the tasks that follow, coded against `ln::Peer`:

| Call | Gives |
|---|---|
| `Peer::start(Config)` | The running peer and what was loaded (`GraphLoad`) |
| `read(\|graph\| …)` | LDK's `ReadOnlyNetworkGraph`: nodes with alias, colour, addresses, features, last update; channels with both policies, short channel id, and capacity once validated. Gossip waits while the closure runs |
| `changes()` | A stream of `Change::Channel(scid)` and `Change::Node(id)`: read the entry again; a missing entry was removed. A receiver that lags rescans |
| `status()` | A watch of `Status`: validation state, synced, counts, ceiling, every peer's connection and failures |
| `events()` | `Event`s as data: periodic `Stats` (counts, messages and bytes per second, totals), sync, peer, ceiling, save and prune events, and LDK's own log records |
| `shutdown()` | Disconnects and writes the graph. Dropping the peer does the same in the background |

**LDK's log records** (`Event::Ldk`) carry the level, module, line, peer and message, cut to 512 bytes; the message is LDK's free text and may quote what a peer sent. Records below `Config::log_level` (`Info`) are not formatted. At most 20 a second are sent; the rest are counted in `Event::LdkSuppressed`. At `Info`, LDK says one line per connection.

### Unverified until validation

The graph is taken on signatures alone: no funding output is looked up, so `ChannelInfo::capacity_sats` is `None` and anyone can announce a channel that does not exist. `Status::validation` is `Validation::Unverified` to say so; do not present the graph as verified.

The validation task plugs a `UtxoLookup` into the gossip handler: `crates/ln/src/gossip.rs` builds `P2PGossipSync::new(graph, None, logger)`, and the hook's type is the `UtxoHook` alias there. It adds a `Validation` variant and sets it in `Status`. Channels validated asynchronously come back from LDK as broadcast events, which the handler already turns into `Change`s.

### Measured on mainnet

TBD

## Bounds

Gossip is untrusted input ([`SECURITY.md`](SECURITY.md)). Until validation exists, a channel costs an attacker nothing to invent, so the graph itself is bounded.

- **Ceiling.** At most `Config::max_channels` (100,000) channels; announcements of new channels beyond it are refused and `Event::CeilingReached` is sent. The check and the insert happen under one lock, so concurrent peers cannot overshoot it. Nodes enter the graph only through channels, so they are bounded by twice that.
- **Message size.** Gossip messages over 1 KB are refused; real ones are under 500 bytes. The exception is node announcements: about 50 real nodes announce one onion address tens to a thousand times (up to 38 KB). Those are checked against their signature and kept with at most 8 distinct addresses and no unknown fields, unsigned (which only matters for relaying, and we do not relay).
- **Memory.** So each channel and node held is bounded, and with the ceiling the graph's memory and its file are bounded: at most about TBD of memory at the ceiling. `Stats` reports the counts, and `Event::GraphSaved` the file's size.
- **Disk.** Nothing gossip sends is written anywhere but the graph file, whose size follows from the ceiling.
- **Range sync.** The channel ids wanted from peers are capped at the ceiling.
- **Funding-output lookups** (the validation task) are to be deduplicated, batched (up to 300 per request) and rate-limited, so a flood of announcements cannot flood the node.
- Aliases, colours and addresses are kept as LDK decoded them, never interpreted, and are served and rendered as plain text.
- The peer makes outbound connections only, never listens, never announces itself and never relays.

## Limits

- **History starts when the peer starts.** Channels that closed before then are not in gossip.
- **Private channels are invisible,** as on every explorer.
- **Tor-only nodes cannot be peers.** Their gossip still arrives through clearnet peers.
- **No world map.** That needs a GeoIP database and is out of scope.

## Switch

Lightning is optional. A network runs without it when it is not enabled.
