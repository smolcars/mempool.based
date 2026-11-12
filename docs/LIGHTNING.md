# Lightning

The Lightning explorer shows the public channel graph: nodes, channels, capacity and their history. It does this with a **gossip-only peer** built from LDK's `lightning` crates, running inside the server.

## What it is not

It is not a Lightning node in the usual sense. It never opens a channel, holds no funds and has no wallet. Its only key is a node identity with nothing behind it.

We do not run LND or Core Lightning (a second daemon with a wallet and channel database, polled for graph dumps), and we do not use `ldk-node` (it bundles an on-chain wallet and channel manager we would never use).

## How it works

1. **Connect.** The peer connects out to a small set of well-connected public Lightning nodes. Channel requests from peers are refused.
2. **Sync.** Peers send the full graph, then live updates. LDK verifies every signature and maintains the network graph.
3. **Validate.** For each announced channel, LDK asks whether the funding output exists, is unspent, is six blocks deep and pays the announced script. We answer from rbitcoin's position routes ([`RBITCOIN.md`](RBITCOIN.md)), which also give the capacity. A channel enters the graph only once it passes ([Validation](#validation)).
4. **Track closes.** *Not built yet.* Every new block's inputs are checked against the set of known funding outputs. A match is a channel close, and the spending transaction's shape classifies it: cooperative, forced or penalty.
5. **Store.** *Not built yet.* Nodes, channels, open and close records and periodic network totals go to SQLite. LDK's own serialized graph is kept for fast restarts (built: see [Files](#files)).

Steps 1 to 3 and the graph file are the `ln` crate (`crates/ln`). It is a library: it does not log and is not wired into the server yet.

## Pieces of LDK used

| Piece | Role |
|---|---|
| `PeerManager::new_routing_only` with `lightning-net-tokio` | Connections and the wire protocol, outbound only |
| `ErroringMessageHandler` | The channel handler: answers every channel message, `open_channel` included, with an error. There is no channel manager |
| `P2PGossipSync` and `NetworkGraph` | Signature checks and the graph, behind our own routing handler (below) |
| `UtxoLookup` and `UtxoFuture` | The hook for funding-output validation: ours answers from its cache at once, or later through a batched lookup ([Validation](#validation)) |
| `KeysManager` | Derives the node identity from the seed file; nothing else is signed |

`lightning` 0.2.8 (the newest release; 0.3 is a release candidate) and `lightning-net-tokio` 0.2.0, the newest 0.2 release of that crate.

Our routing handler wraps `P2PGossipSync`. It passes gossip through to LDK and counts it, enforces the [bounds](#bounds), never relays (it tells the peer manager not to forward, and drops LDK's broadcast requests), never serves the graph (no initial dump to peers; a `query_channel_range` gets an empty reply, as the spec requires), adds the channel-range sync below, and answers LDK's `processing_queue_high` from our own lookup queue ([Validation](#validation)).

## The gossip peer

### Sync

LDK asks each peer for gossip with a `gossip_timestamp_filter` set two weeks back, which Core Lightning and most LND nodes answer with their whole graph. On mainnet the default peers did not: eclair (ACINQ) and the Bitfinex LND nodes sent only new gossip. So the peer also asks every peer that supports gossip queries for every channel id it knows (`query_channel_range`), and requests the channels it lacks (`query_short_channel_ids`), 2,000 ids per batch, one batch at a time per peer and never the same channel from two peers at once. A peer's channel list is complete when a reply reaches the end of the queried range (the `sync_complete` flag is not an end marker), or when its replies stop for a minute. A batch that gets no answer within 2 minutes is dropped; its channels come back at the next range sync, which happens on every connection.

Peers' graphs differ: alone, ACINQ listed 21,600 channels and Bitfinex 30,250. Asking every peer gives the union.

Channels we already hold whose nodes have sent no announcement are requested too, once per node per process. A timestamp filter brings only the last two weeks of node announcements and most nodes announce less often, while a peer answering for a channel also sends its nodes' announcements. Without this, a sync from LND peers left 3,900 of 13,600 nodes with an alias; with it, 7,560. The rest never announce.

**"Synced"** (`Status::synced`, `Event::Synced`) means the graph is complete enough to show: validation is caught up (nothing unchecked or pending), and either every connected peer's channel list has arrived, every channel on them has been received or given up on, and no new channel has arrived for 30 seconds, or the graph was loaded from a file caught up within the last 13 days and every channel in it has been checked again.

The graph file's modification time is set to the last moment the peer was **caught up**: connected, with every listed channel requested, no new channel for 30 seconds and validation caught up. It is not the time of writing, so a graph saved after a week offline does not pass for fresh. After a restart the timestamp filter starts an hour before that moment instead of two weeks back (LDK's own window for peers it does not fully sync), so peers that honour it resend only what is new. A day's margin would be most of the graph again: nodes refresh their channel updates about daily.

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

The graph is written every 10 minutes when it changed (a failed write is retried at the next period), and at shutdown, except while a loaded graph is still being checked again ([Restart](#restart)), through a temporary file that is synced, renamed over the old one and followed by a sync of the directory, so a crash leaves the old file or the new one, never half of one. Writing it takes about 0.1 s. At start a file for another network, one that does not decode, one larger than 8 KB per channel of the ceiling (the most one channel's gossip can occupy, with its nodes), or one holding more channels than the ceiling (saved under a higher one), is reported (`GraphLoad::Unreadable`) and replaced at the next save.

### Interface

For the tasks that follow, coded against `ln::Peer`:

| Call | Gives |
|---|---|
| `Peer::start(Config, impl Chain)` | The running peer and what was loaded (`GraphLoad`). `Chain` is where funding outputs are looked up: implemented for `node::esplora::Esplora` |
| `read(\|graph\| …)` | LDK's `ReadOnlyNetworkGraph`: nodes with alias, colour, addresses, features, last update; channels with both policies, short channel id and capacity (`capacity_sats`, always `Some`: every channel in it was verified). Gossip waits while the closure runs |
| `changes()` | A stream of `Change::Channel(scid)` and `Change::Node(id)`: read the entry again; a missing entry was removed. A receiver that lags rescans |
| `status()` | A watch of `Status`: `Validation` ([below](#states)), synced, counts, ceiling, every peer's connection and failures |
| `events()` | `Event`s as data: periodic `Stats` (counts, validation, messages and bytes per second, totals), sync, peer, ceiling, save, prune and failed-lookup events, and LDK's own log records |
| `shutdown()` | Disconnects and writes the graph. Dropping the peer does the same in the background |

**LDK's log records** (`Event::Ldk`) carry the level, module, line, peer and message, cut to 512 bytes; the message is LDK's free text and may quote what a peer sent. Records below `Config::log_level` (`Info`) are not formatted. At most 20 a second are sent; the rest are counted in `Event::LdkSuppressed`. At `Info`, LDK says one line per connection.

## Validation

A signature proves that four keys agreed to announce a channel, not that it exists. A channel is real only if its funding output is on the chain, unspent, and pays the two-of-two P2WSH script of the two announced bitcoin keys. Every channel is checked before it enters the graph, so the graph the crate exposes holds only verified channels, each with its capacity: the funding output's value.

### How a lookup travels

1. A peer sends a `channel_announcement`. Our handler refuses one for another chain, checks the ceiling, computes the script the announced bitcoin keys require and passes the message to LDK, which checks the four signatures and calls our `UtxoLookup` with the short channel id.
2. **Cached:** if the id was looked up within the last hour, the answer is given at once (`UtxoResult::Sync`): the output if its script matches, otherwise a refusal. A channel LDK already holds as chain-validated with the same nodes is not looked up at all.
3. **Otherwise the answer comes later** (`UtxoResult::Async`): LDK keeps the announcement waiting on a `UtxoFuture`, and holds the channel's updates and its nodes' announcements with it as they arrive. Asynchronous answers are what let lookups be batched without blocking gossip processing. The lookup joins a queue; a second announcement of the same id joins the same entry.
4. **One task sends the queue** to `POST /block-heights/txs/outs`, up to 300 positions per request (the short channel id is the block height, transaction index and output index). There is never more than one request in flight, and at least `Config::lookup_interval` (500 ms) between the starts of two requests, retries included. A first lookup waits 50 ms so that a burst leaves as one batch.
5. **The answer is classified** against the tip height stamped on the response, and every waiting announcement is resolved through LDK. LDK then adds the channel with `capacity_sats` set and applies the updates and node announcements it held; the handler reports the changes. The peer manager is told to read again.

### States

A channel is in exactly one of these. `Status::validation` (`Validation`) counts them.

| State | Meaning | In the graph | Counted in |
|---|---|---|---|
| Unchecked | In the graph file at start, not yet sent for checking ([Restart](#restart)) | No | `unchecked` |
| Pending | Waiting for a lookup, or in one | No | `pending` |
| Early | Its output is less than six blocks deep (BOLT 7), or missing from a block at most six past the node's tip, which may only mean the node is behind. Checked again once the node's tip reaches the funding height plus five | No | `early` |
| Verified | Unspent, at least six deep, pays the announced keys | Yes, with its capacity | `verified` (the graph's channel count) |
| Refused | One of the reasons below | No | `rejected`, by reason |

Refusal reasons (`Rejected`):

| Reason | What it means |
|---|---|
| `missing` | No output at that position, also once an early one is deep enough: an invented channel |
| `spent` | The output exists but is spent: a **closed channel still being gossiped**, counted apart from an invented one. It never enters the graph; recording closes from new blocks is the next task |
| `wrong_script` | The output does not pay the announced keys: someone announced a real output as theirs |
| `wrong_chain` | Announced for another chain's genesis |
| `shallow` | Early while `max_early` announcements already waited |
| `queue_full` | Refused unchecked because the queue held `max_pending` plus 64. Peers send it again at the next channel-range sync |

`missing`, `spent` and `shallow` count lookups: an id is looked up at most once while its result is cached, so a channel refused and announced again within the hour is not counted twice. `wrong_script`, `wrong_chain` and `queue_full` count announcements. A mempool spend of a funding output counts as unspent, as rbitcoin reports it. `caught_up` is true when nothing is unchecked or pending and a loaded graph has been fully replayed; early channels do not hold it back, since they wait for blocks. `node_failures`, `requests` and `outputs` report the node side.

LDK reports a message it holds for a lookup as an error; the handler counts it as `Totals::held` instead of `rejected`.

### Settings and bounds

Lightning gossip triggers these lookups, so they are bounded ([`SECURITY.md`](SECURITY.md) rule 11):

| Setting | Default | Bound |
|---|---|---|
| `Config::lookup_interval` | 500 ms | Requests to the node, one at a time: at most 2 a second, so at most 600 outputs a second, whatever arrives |
| `Config::max_pending` | 5,000 | Announcements waiting for a lookup. At this many, `processing_queue_high` turns true and LDK holds each peer's next gossip message and stops reading from it (it also pauses its ping timeout, so the peer is not dropped) until lookups drain. Nothing is dropped; a burst is paced to the lookup rate. LDK can still hand in a message already read, so the hard limit is 64 more, beyond which announcements are refused as `queue_full` |
| `Config::max_early` | 1,000 | Announcements held as early |
| `Config::cache_entries` | 100,000 | Lookup results kept, found or not, oldest evicted first |
| `Config::cache_ttl` | 1 hour | How long a result is trusted |
| Batch size | 300 | rbitcoin's own limit for the batch route |
| Request timeout | the client's (10 s), and 30 s at most | A request that does not answer counts as failed |

A held announcement costs LDK a copy of it plus the latest update per direction and node announcement per node, at most about 4 KB, so `max_pending` bounds that memory at about 20 MB. The ceiling (`max_channels`) counts channels in the graph plus those unchecked, pending, being resolved and early, so the graph cannot overshoot it when lookups resolve.

**When the node is down or slow**, a failed or timed-out request puts its batch back at the head of the queue and is retried after 1 s, doubling to 60 s. `Event::LookupFailed` reports each failure with the backlog, and `Validation::node_failures` counts them in a row. The queue fills, gossip from peers waits, and once the node answers it drains. Announcements are not lost and the peer does not wedge: connections stay up and reading resumes on its own.

**A hostile peer cycling invented channels** costs at most one request every 500 ms to the node, at most `max_pending` waiting announcements, `max_early` early ones and `cache_entries` cached results; meanwhile honest gossip is paced along with it. It can fill the cache with its own refused ids and evict honest results, which only costs lookups again at the same bounded rate. Honest LDK, LND and CLN peers validate before relaying, so invented channels reach us only from a peer that does not.

### Restart

A graph file is not trusted as it is: a file written by the unverified peer of earlier versions holds channels that were never checked, and any file may hold channels closed while the peer was down. So a loaded graph is not shown. Its channels, with their stored announcements and latest updates, and its node announcements, are taken out, and the peer starts with an empty graph. On each one-second tick, while the lookup queue is under half full (so live gossip keeps the other half), up to 2,000 stored channels are sent through the same check as gossip; their updates wait in LDK with them, and a node's stored announcement is applied once one of its channels is verified. Until then, gossip that arrives for a stored channel or node is kept with it if newer (an oversized node announcement trimmed first), and the channel-range sync does not ask peers for channels already stored or pending. A channel enters the graph only once it passes, so after a restart nothing unverified can pass as verified, and every stored channel is checked again against today's chain.

While stored channels are still to be checked or answered, early ones included (until their output is six deep: twelve blocks at most), the replay stays open: the graph is not saved, at shutdown included, since the file on disk is the better copy, and `caught_up` stays false. A restart in the middle of a check therefore starts over from the same file. The file never records which channels were verified; it does not need to, since every channel is checked again at each start.

The alternative, discarding the file and resyncing from peers, was measured as the first sync below. Replaying costs only the lookups; a resync costs the same lookups plus the full gossip download.

### Measured on mainnet

2026-11-12, the example program `crates/ln/examples/ln_watch.rs` on an Apple M4, release build, the default peers, before validation existed. Bandwidth is the process's own traffic (`nettop`), memory its physical footprint. The gossip figures still hold; for sync and restart times with validation, see [below](#validation-measured-on-mainnet).

| | |
|---|---|
| Initial sync | Synced 84 s after start: 36,247 channels, 13,649 nodes. The first 20 s bring 34,000 channels at 1.5–2.7 MB/s; LND peers then send at their own rate limit, about 51 kB/s |
| Bandwidth | 49.7 MB in and 0.23 MB out for the initial sync. Afterwards 1 to 20 gossip messages a second, about 1–2 kB/s |
| Memory | 160–195 MB, flat over 20 minutes. LDK's graph takes about 4 KB per channel; the same footprint is reached right after loading the file |
| Graph file | 32.8 MB at 36,248 channels. Written in 0.14 s, loaded in 0.07 s |
| Restart | Synced from the file 1.1 s after start, unchecked. Catching up took 6.2 MB of gossip, quiet after a minute |
| Peers | All three first choices connected within 3 s and stayed up for 20 minutes; restarts reconnected within 2 s. No peer dropped us, so live reconnection with backoff is proven by the tests only |

**Against public figures.** Sites built on mempool.space reported about 41,000 channels and 17,400 nodes in mid-2026; CoinPaprika reported 28,900 channels and 15,900 nodes for 4 November 2026; 1ML about 20,000 active channels. Ours sits between them. The node count is lower because LDK keeps a node only while it has a channel in the graph. Of the channels, 21,600 have a policy in both directions, 5,100 in one and 9,500 none yet: LDK refuses policies older than two weeks and drops channels left without one after two weeks, so counts differ by how long each site keeps silent channels. Peers also differ (see [Sync](#sync)).

### Validation measured on mainnet

2026-11-12, the same program and peers, release build, lookups against a synced rbitcoin v0.9.2 mainnet node on the same private network, default settings (500 ms between requests). Memory is the process's resident size; macOS compresses an idle process's pages on this busy machine, so only peaks are given.

| | First sync, empty directory | Restart from the file it saved |
|---|---|---|
| Fully verified | 84 s: 36,184 channels. Lookups ran at their limit, 600 a second; the queue sat at its 5,000 bound from 10 s to 74 s, pacing gossip | 62 s: all 36,226 stored channels checked again, same rate |
| Synced | 235 s. Validation was done at 84 s, as was most of the graph; the LND peers' last range batches, sent at their 51 kB/s and delayed by the pacing, ended on their 2-minute timeout | 62 s |
| Node cost | 125 requests and 18.0 MB of responses (about 500 bytes an output) to be verified; 165 requests and 36,227 outputs by the end of the sync | 121 requests, 36,227 outputs, 18.0 MB |
| Steady state | No request at all in the 4 minutes after the sync, nor in the 2 after the restart: a new channel is announced a few times an hour, and channels LDK already holds as verified are not looked up again | |
| Memory | Peak 186 MB | Peak 234 MB, while the loaded graph's messages and the new graph coexist |
| Refused | 1 spent (a closed channel still gossiped). None missing, wrong-script, wrong-chain or early | The same spent one, announced again |

A restart stopped 4 s into the replay (1,800 channels checked again) wrote nothing: the graph file was byte-identical afterwards.

**Capacity.** The verified graph holds 3,231.11 BTC in 36,226 channels. CoinPaprika, from mempool.emzy.de, gave 3,203 BTC in 28,939 channels for 4 November 2026; Coinglass 3,456 BTC for 27 October. Capacity agrees within 1 % of the first, though our channel count is higher for the reasons above. Five channels sampled across the graph (from block 611,038 to 975,065, 100,000 to 10,000,000 sats) were checked by hand against their funding outputs on the node: each is an unspent P2WSH output at the announced height whose value is the capacity shown.

## Bounds

Gossip is untrusted input ([`SECURITY.md`](SECURITY.md)). Validation makes a channel cost an attacker a real funding output, but the graph is bounded anyway, and so is the validation itself.

- **Ceiling.** At most `Config::max_channels` (100,000) channels; announcements of new channels beyond it are refused and `Event::CeilingReached` is sent. The check and the insert happen under one lock, so concurrent peers cannot overshoot it. Nodes enter the graph only through channels, so they are bounded by twice that.
- **Message size.** Gossip messages over 1 KB are refused; real ones are under 500 bytes. The exception is node announcements: about 50 real nodes announce one onion address tens to a thousand times (up to 38 KB). Those are checked against their signature and kept with at most 8 distinct addresses and no unknown fields, unsigned (which only matters for relaying, and we do not relay).
- **Memory.** So each channel and node held is bounded, and with the ceiling so are the graph's memory and its file. At mainnet's mix, 4 KB per channel, the ceiling is about 400 MB; filled by an attacker with the largest messages accepted, about twice that, and a file of at most 8 KB per channel. `Stats` reports the counts, and `Event::GraphSaved` the file's size.
- **Disk.** Nothing gossip sends is written anywhere but the graph file, whose size follows from the ceiling.
- **Range sync.** The channel ids wanted from peers are capped at the ceiling.
- **Funding-output lookups** are deduplicated, cached, batched (up to 300 per request), queued in a bounded queue that pauses gossip when full, and rate-limited ([Settings and bounds](#settings-and-bounds)), so no amount of gossip can load the node beyond one request per `lookup_interval`.
- Aliases, colours and addresses are kept as LDK decoded them, never interpreted, and are served and rendered as plain text.
- The peer makes outbound connections only, never listens, never announces itself and never relays.

## Limits

- **History starts when the peer starts.** Channels that closed before then are not in gossip.
- **Private channels are invisible,** as on every explorer.
- **Tor-only nodes cannot be peers.** Their gossip still arrives through clearnet peers.
- **No world map.** That needs a GeoIP database and is out of scope.

## Switch

Lightning is optional. A network runs without it when it is not enabled.
