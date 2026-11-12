# Lightning

The Lightning explorer shows the public channel graph: nodes, channels, capacity and their history. It does this with a **gossip-only peer** built from LDK's `lightning` crates, running inside the server.

## What it is not

It is not a Lightning node in the usual sense. It never opens a channel, holds no funds and has no wallet. Its only key is a node identity with nothing behind it.

We do not run LND or Core Lightning (a second daemon with a wallet and channel database, polled for graph dumps), and we do not use `ldk-node` (it bundles an on-chain wallet and channel manager we would never use).

## How it works

1. **Connect.** The peer connects out to a small set of well-connected public Lightning nodes. Channel requests from peers are refused.
2. **Sync.** Peers send the full graph, then live updates. LDK verifies every signature and maintains the network graph.
3. **Validate.** For each announced channel, LDK asks whether the funding output exists, is unspent, is six blocks deep and pays the announced script. We answer from rbitcoin's position routes ([`RBITCOIN.md`](RBITCOIN.md)), which also give the capacity. A channel enters the graph only once it passes ([Validation](#validation)).
4. **Track opens and closes.** Every new block's inputs are checked against the set of verified channels' funding outputs. A match is a channel close: the channel leaves the graph, and the spending transaction's shape classifies it: cooperative, force or (later) penalty. Opens and closes are kept as a history ([Opens and closes](#opens-and-closes)).
5. **Store.** Every open and close, and the network's totals every ten minutes, go to the network's SQLite file ([In the server](#in-the-server)). Nodes and open channels are served from the live graph, which LDK's own serialized file brings back quickly after a restart ([Files](#files)).

Steps 1 to 4 and the graph file are the `ln` crate (`crates/ln`). It is a library and does not log; the server runs it, stores what it reports and logs its events ([In the server](#in-the-server)).

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

They were picked from the live graph on 2026-11-12: the best-connected nodes that announce a clearnet address and are run by companies that have operated Lightning nodes for years, with the address in the list matching the node's own announcement. Several nodes picked from memory (Kraken, River, Wallet of Satoshi, Boltz) were unreachable at their old addresses and were dropped. Only the first three are dialled while they stay up. An operator replaces the list with `{NET}_LIGHTNING_PEERS` ([`CONFIG.md`](CONFIG.md#lightning)). On other networks there are no defaults, and the server will not start Lightning there without a list.

**Tor-only nodes are out of reach:** the peer makes plain TCP connections and does not use Tor.

### Files

Both live in `Config::data_dir`, which the server sets to `<DATA_DIR>/lightning/<network>/`, beside the network's SQLite file.

| File | Holds | Size |
|---|---|---|
| `node_key` | The 32-byte identity seed. Created on first run from `/dev/urandom` with mode `0600`; a seed readable by group or others is refused at start | 32 bytes |
| `graph` | LDK's own serialization of the network graph | about 31 MB at 30,000 channels |

The graph is written every 10 minutes when it changed (a failed write is retried at the next period), and at shutdown, except while a loaded graph is still being checked again ([Restart](#restart)), through a temporary file that is synced, renamed over the old one and followed by a sync of the directory, so a crash leaves the old file or the new one, never half of one. Writing it takes about 0.1 s. At start a file for another network, one that does not decode, one larger than 8 KB per channel of the ceiling (the most one channel's gossip can occupy, with its nodes), or one holding more channels than the ceiling (saved under a higher one), is reported (`GraphLoad::Unreadable`) and replaced at the next save.

### Interface

For the tasks that follow, coded against `ln::Peer`:

| Call | Gives |
|---|---|
| `Peer::start(Config, impl Chain)` | The running peer and what was loaded (`GraphLoad`). `Chain` is where funding outputs are looked up and blocks and spends are read: implemented for `node::esplora::Esplora` |
| `read(\|graph\| …)` | LDK's `ReadOnlyNetworkGraph`: nodes with alias, colour, addresses, features, last update; channels with both policies, short channel id and capacity (`capacity_sats`, always `Some`: every channel in it was verified). Gossip waits while the closure runs |
| `changes()` | A stream of `Change::Channel(scid)` and `Change::Node(id)`: read the entry again; a missing entry was removed. A receiver that lags rescans |
| `status()` | A watch of `Status`: `Validation` ([below](#states)), `Closes` ([below](#status-and-events)), synced, counts, ceiling, every peer's connection and failures |
| `history()` | A stream of `Record`s: each channel opened, closed, swept as a penalty, or undone by a reorg ([Opens and closes](#history)) |
| `recent(after)` | The newest `Config::history` (10,000) records after a sequence number |
| `events()` | `Event`s as data: periodic `Stats` (counts, validation, messages and bytes per second, totals), sync, peer, ceiling, save, prune and failed-lookup events, and LDK's own log records |
| `open()` | Every channel open now as an `Opened` record, and the sequence number that set is current at: what a consumer that fell past `recent` stores instead |
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
| `spent` | The output exists but is spent: a **closed channel still being gossiped**, counted apart from an invented one. It never enters the graph. A channel from the graph file found spent closed while the peer was down; its close is looked up ([Catch-up](#catch-up)) |
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

## Opens and closes

A graph is a snapshot; gossip is slow and unreliable about saying a channel ended. The chain is not: a channel ends when its funding output is spent, and the spending transaction says how. The crate watches every verified channel's funding output, removes a channel from the graph in the block that spends it, and keeps a history of opens and closes. It is `crates/ln/src/closes.rs`.

### How closes are found

A funding output pays the P2WSH hash of `2 <key1> <key2> 2 OP_CHECKMULTISIG`, and spending it reveals that 71-byte script as the last witness item. So the watcher reads each new block once (the raw block, one request) and, for every input whose witness ends with a two-of-two of compressed keys, hashes the script and looks the hash up in the set of watched outputs. There is no work per channel: the cost is one block, whatever the number of channels.

A match is checked once more: a script can be paid twice (a splice may keep the funding keys, and anyone can send coins to a funding address), so the spent transaction's position is asked of the node (`/tx/:txid/merkle-proof`, one small request per match) and must be the short channel id's block and index, and the input's output index its own.

**Why the script and not the outpoint.** Matching inputs by outpoint needs each channel's funding txid, which a short channel id does not contain, and the position route validation uses answers with the output (script, value, spent, block) but not its txid. Learning the txid costs two requests per channel (`/block-height/:h`, then `/block/:hash/txid/:index`): 72,000 requests for mainnet's 36,000 channels at every start, an hour and more at validation's pace. The script needs nothing: validation already holds it, since it checked the output pays it. Taproot channels have no such script, but they are not announced through today's gossip, so none is ever in the graph.

The alternative, asking the node about every channel's output at each block, costs 121 requests and 18 MB per block at mainnet's size; reading the block costs one request and 1.7 MB.

The newest block is learned by asking the node's tip every `Config::tip_every` (10 s, 64 bytes). A new tip brings one page of headers (4.4 KB, ten headers), which also shows a reorg, then each new block in turn.

### How a close is classified

| Kind | Recognised by | How sure |
|---|---|---|
| `Force` | The spending transaction is a commitment: BOLT 3 sets the top byte of its locktime to `0x20` and of its input's sequence to `0x80` (the obscured commitment number fills the rest) | Certain. Every LN-penalty implementation builds commitments this way, and no other transaction of the protocol carries both markers. Anchor channels change the outputs (two 330-sat anchors, a delayed `to_remote`), not the markers |
| `Penalty` | Later: an output of that commitment is spent through its revocation path. `to_local` with witness `<sig> 1 <script>` (the script starts `OP_IF <revocation key>`); an HTLC output with `<sig> <revocation key> <script>`, the key hashing to the one its script names | Certain once seen: only the victim holds the revocation key, and only for a revoked commitment. Recorded as a `Penalty` record in the sweep's block; until then the close reads `Force` |
| `Cooperative` | Not a commitment, and its only input is the funding output | Certain that both sides signed a transaction that is not a commitment. Likely a mutual close (legacy `closing_signed`, sequence `0xffffffff`, locktime 0; or `option_simple_close`, sequence `0xfffffffd`). A splice that only takes funds out has the same shape; telling them apart needs the next channel's funding txid, which we do not learn |
| `Other` | Not a commitment, and more inputs than the funding output | Certain that it is not a close by either protocol: closing transactions spend the funding output alone. Most likely a splice that adds funds. It ends the channel id all the same |

**What the chain cannot say:** which side forced the close (the commitment's keys are per-commitment, not the node keys), why, or whether a cooperative close was a splice. A penalty is recognised only if its sweep confirms within 2,016 blocks of the force close (the longest `to_self_delay` implementations accept), while the peer runs: force closes found by the catch-up, and those older than the watcher's memory, are not watched. Sweeps of the second-level HTLC transactions are not watched; a breach almost always shows on `to_local` first.

The fee is the funding value less the outputs when the funding output is the only input; otherwise the node's fee for the transaction (one more request).

### History

Records, in order, each with a sequence number that grows by one:

| Record | Carries | When |
|---|---|---|
| `Opened` | Channel id, both nodes, capacity, funding block (height, hash, time) | A channel is verified: its first time in the graph since start, including every channel at start, with its real funding block time |
| `Closed` | Channel id, both nodes, capacity, funding block, closing block, closing txid, kind, fee | Its funding output is spent in a block |
| `Penalty` | Channel id, sweep txid, block | A force close turns out to be a breach |
| `OpenUndone` | Channel id | The funding block left the chain. An `Opened` with the new block follows if the output is still at the same position |
| `CloseUndone`, `PenaltyUndone` | Channel id | The closing or sweeping block left the chain |

`Peer::history()` streams them; its room is `Config::history` records (rounded up to a power of two), so a receiver that falls further behind gets a lag error, then reads `Peer::recent(last_seq)` for the newest `Config::history` records, and rescans the graph if those do not reach back. At start the stream carries one `Opened` per channel as validation verifies it (36,000 on mainnet in about a minute).

### Removal from the graph

A closed channel leaves the graph in the block that spends it, once: the watched set is the gate, and a channel is removed from it before anything else happens. Its funding position is stored as spent in the lookup cache first, then LDK removes it (with nodes left without a channel), which also makes LDK refuse its announcement for a week. Gossip for it keeps arriving for days; an announcement is answered from the cache (spent) for an hour, then by the node (spent), and its updates are refused for an unknown channel, so it cannot come back. `Change::Channel` and `Change::Node` report the removal.

### Catch-up

Blocks are not read before the watcher's first tip or across a gap; the node's spend records are asked instead:

- **After downtime.** The graph file's channels are all checked again at start ([Restart](#restart)). One found spent closed while the peer was down. Its spend is found in four requests: the funding block's hash and the txid at the channel's index (the funding txid), the output's spender (`/tx/:txid/outspend/:vout`, which gives the block), and the spending transaction. Its fee may need a fifth.
- **A gap.** More than ten new blocks at once (the node was unreachable, or the server was suspended): reading them would cost more than checking every watched output by position (18 MB at mainnet's size, ten blocks' worth), so the watcher starts again from the tip and checks every watched output, 300 per request. Spent ones are looked up as above.
- **A late answer.** A channel whose lookup was answered at a tip older than the newest block read (or the block being read), or at a tip on another branch than the block read at that height, is checked again the same way, so a close in a block read before the channel was watched is not missed.

A spend the catch-up finds in a block the watcher has not read on its own branch (a newer block, or another branch at a height it read) is not recorded there: the channel is watched, and reading that block records the close, with what undoes it. On another branch, the channel is also checked again, in case the watcher's branch wins. A channel queued for checking is queued once.

The catch-up shares the watcher's pacing: one request at a time, at least `Config::lookup_interval` (500 ms) apart, behind the tip check. A check that fails stays queued and is retried after a backoff. `Closes::catch_up` counts what waits.

### Reorgs

The watcher keeps the newest 100 blocks it read with what each changed (its closes, with what puts the channel back in the graph, and its penalties). At each new tip it walks headers down from the tip to the newest block it still holds on the node's best chain (the fork), undoes every block above it newest first, then reads the new blocks in order, as the block table does ([`STORE.md`](STORE.md#blocks)). A block whose parent is not the last one read stops the step, to be retried.

- **A close undone:** the channel is watched again and put back in the graph with its capacity, policies and node announcements (through LDK's partial-announcement call, since LDK refuses an announcement of a channel it removed; the restored channel has no stored announcement, so after a restart it returns through gossip). `CloseUndone` is recorded; if the new chain spends it, it closes again, perhaps differently.
- **An open undone:** every channel funded above the fork is checked again by position. Still at its position in another block: `OpenUndone`, then `Opened` with the new block. Gone: `OpenUndone`, and it leaves the graph. A funding output is six blocks deep before a channel is verified, so this takes a reorg of seven blocks or more.
- **Deeper than the window**, or no common block at all: everything held is undone, the watcher starts again from the tip and checks every watched output, as after a gap. Closes found by the catch-up are undone only while their block is in the window.

`Event::Reorg` reports the fork and how many blocks were undone; `Event::CloseWatchReset` a start from the tip after a gap.

### Status and events

`Status::closes` (`Closes`): the newest block read; `caught_up` (the node had no newer block at the last look, and nothing waits for the catch-up or the replay); funding outputs and force closes watched; the catch-up backlog; counts since start of opens, closes by kind, undone records and reorgs; blocks read, requests and failures in a row. `Event::CloseWatchFailed` reports a failed request (retried after 1 s, doubling to 60 s).

### Bounds and cost

Nothing gossip sends changes what the watcher asks of the node: a channel enters the watched set only once its funding output was found on the chain.

| | Bound |
|---|---|
| Per block | The tip every 10 s (64 bytes), one page of headers, the raw block (about 1.7 MB on mainnet), one position request per input that spends a watched script, and one fee request per such input that is not alone in its transaction. Inputs that spend a watched script require that channel's keys, so this is one request per real close |
| Rate | One request at a time, at least `Config::lookup_interval` apart: at most 2 a second, besides validation's 2 |
| Catch-up | The same rate. Four or five requests per channel closed while down; 300 outputs per request when checking every watched output |
| Watched outputs | One entry per verified channel, so bounded by the graph's ceiling: about 200 bytes each with the script index, 7 MB at 36,000 channels, 20 MB at the 100,000 ceiling |
| Force closes watched for a penalty | At most 10,000, the oldest dropped first, for 2,016 blocks each |
| Block window | 100 blocks with their closes (a few hundred bytes each) |
| History | `Config::history` (10,000) records of about 250 bytes: 2.5 MB |

### Closes measured on mainnet

2026-11-12, `crates/ln/examples/ln_watch.rs`, release build, the default peers, a synced rbitcoin v0.9.2 mainnet node on the same private network. A first start from an empty directory ran 15 minutes; the peer was then stopped for 44 minutes (three blocks), and restarted from the graph file it saved for 75 minutes.

| | |
|---|---|
| First start | The watcher started from the tip; every channel was verified at that tip, so nothing needed checking again. 36,222 channels watched, one `Opened` each |
| Catch-up after 44 minutes down | The restart's replay found 2 stored channels spent; both closes (blocks 975,537 and 975,539) were found from spend records 72 and 74 s after start, 8 requests in all. The replay itself costs what [Validation measured on mainnet](#validation-measured-on-mainnet) says |
| Closes seen | 27 in 13 blocks (975,537 to 975,549): 26 cooperative, 1 force (with anchors). 11 in one block (975,541). No penalty, no reorg |
| Checked by hand | Four closes against the node: each spends the output at its channel id's position, in the block recorded, with the fee and capacity recorded. The three cooperative ones have the `closing_signed` shape (sequence `0xffffffff`, locktime 0); the force close carries both commitment markers and two 330-sat anchors |
| Node cost per block | One page of headers (4.4 KB), the raw block (1.7 MB on average over 8 blocks) and one 0.3 KB position request per close; besides, the tip every 10 s. 388 requests and 13.6 MB in the last hour |
| Memory | Peak 175 MB on the first start, 301 MB on the restart while the loaded graph and the new one coexist (234 MB before the watcher existed: macOS's figures on this busy machine vary by tens of MB) |

**Across a day's blocks.** Every spend of a two-of-two in the 144 blocks before 975,536 (229 MB read): 209 commitments (206 with anchors), 1,720 single-input non-commitment spends and 3,070 multi-input ones, no revocation sweep. Commitments are Lightning's alone, so about 200 force closes a day, private channels included; the other two counts include ordinary two-of-two wallets (consolidations of up to 224 such inputs, sequence 0), which is why the watcher matches only announced channels' outputs. No public source gives a daily close count to compare: Glassnode charts only the net daily change in channels, mempool.space and Coinglass only totals.

## In the server

With `{NET}_LIGHTNING_ENABLED=true` ([`CONFIG.md`](CONFIG.md#lightning)) the server runs one peer per network, in `crates/server/src/lightning.rs`.

### What runs

One task per network owns the peer. It is spawned after the routes are built, and nothing else waits on it: the graph file is read inside that task, so a large or corrupt file, an unreachable peer or a slow node never delays startup, readiness, block handling or the mempool engine. The task starts the peer, then runs three loops until shutdown: the history consumer, the event reporter and the statistics sampler. A peer that cannot start (the store did not open, the data directory or node key is unusable) is logged as `lightning_failed` and shows as `failed`. So does one whose server task panics, which tokio confines to that task, and one that went silent: the peer sends its statistics every ten seconds, and its supervisor stops if the lookup worker or the close-watcher ended (they end only by panicking), so two minutes without them mean it can no longer validate or watch (`error` `stalled`). None of this stops anything else, and nothing restarts the peer: fix the cause and restart the server.

At shutdown, after the last request, the server stops each peer (which writes its graph: `lightning_stopped`, 0.1 s on mainnet), then stores every history record it had not stored yet, the ones still on the stream included, so a close the peer found is never lost to the graph file that no longer holds its channel. All of it within 10 s.

**Readiness.** `/readyz` reports each network's `lightning` state (`off`, `starting`, `syncing`, `synced`, `failed`, `stopped`) but does not count it: the explorer is ready whatever Lightning does ([`DEPLOY.md`](DEPLOY.md#health)). `/app/lightning/status` has the detail ([`API.md`](API.md#lightning)).

### What is stored

| Table | Holds | Written |
|---|---|---|
| [`ln_channels`](STORE.md#ln_channels) | Every channel the peer verified and how it closed: kind, block, txid and fee, kept consistent through reorgs by the `…Undone` records | As the [history](#history) arrives, in writes of up to 2,000 records |
| [`ln_statistics`](STORE.md#ln_statistics) | Channels, nodes (by address kind), capacity and fee policies every ten minutes, rolled into hours and days | At every whole ten minutes while the graph is synced |

Both are written with `try_write`: when the store's queue is full the Lightning side waits and retries instead of taking a slot, so block and mempool history writes never queue behind it. Nodes, aliases and open channels' policies are not stored: they are in the live graph, and come back from the graph file after a restart.

### What is served

The peer's status, the latest statistics and the series over an interval, the newest opens and closes, and nodes, channels, search and rankings ([Routes](#routes)), under `/app/lightning/…` and in the reference's shapes under `/api/v1/lightning/…` ([`API.md`](API.md#lightning)). Every answer comes from the store or the server's memory; none asks the node, and none takes the graph's lock.

### Routes

The node, channel, search and ranking routes read an **index** of the verified graph (`engine::lightning::Index`, built by `crates/server/src/lightning/index.rs`), never the graph itself, so a request costs a lookup whatever the graph's size and gossip never waits for one.

**Built every 30 s while the graph is synced.** The task copies the graph out of LDK in chunks of 2,000 entries, taking and releasing the graph's lock for each (25 holds on mainnet, the longest 1.05 ms), then indexes the copy on a blocking thread with no lock at all and swaps the new index in behind an `Arc`. A request clones the `Arc` and lets go; nothing it does can make the peer or the store's writer wait. Gossip that lands between two chunks is in the copy or not; a channel or node gone by its chunk is skipped, and the next build has it right. A graph that has never been synced is not indexed: until then the routes answer that they are syncing, rather than from a half-loaded graph.

| Held per build | From |
|---|---|
| Every channel by short channel id, with both ends, capacity and both policies | The graph |
| Every node with a channel, by public key, with its announcement, capacity, open and active channel counts, and its channels ordered largest first | The graph |
| Each node's lowercased alias, for search | The graph |
| The top 100 nodes by capacity, by open channels and by age | Both |
| Each node's oldest funding block time and its closed channels by kind, and when our history starts (the oldest statistics row) | The store, in one grouped read of `ln_channels`, done again only after new history or statistics rows were stored |

**Stale by at most 30 s plus a build.** A channel's own routes also read its row from the store, so a close shows there at once. Measured on mainnet (36,223 channels, 13,639 nodes, release build): the copy 10.8 ms, a whole build 24 to 41 ms, so about 0.1 % of one core. **Memory**, estimated from the structures' sizes: about 185 bytes a channel and 320 a node, and 80 a node for the store's part: about 12 MB at mainnet's size, twice that while a new index is built beside the old, and at most about 85 MB at the 100,000-channel ceiling (200,000 nodes). It is not measured apart: the process's footprint after the first build, 249 MB, is within the variation of the figures [measured](#measured-in-the-server) before the index existed.

**Search** is bounded whatever the query: 3 to 66 characters, a binary search over the public keys for a prefix and one pass over at most 2 × the ceiling aliases of at most 32 bytes, 10 results. Aliases are kept as LDK prints them (up to the first NUL, control characters as U+FFFD, so at most 96 bytes), matched lowercased, served as JSON strings and never interpreted; an alias proves nothing, so the impostor of a well-known name is found too, ranked by capacity ([`API.md`](API.md#nodes-channels-search-and-rankings)).

**A node's past** (`/app/lightning/nodes/:key/history`, `/api/v1/lightning/nodes/:key/statistics`) is rebuilt from `ln_channels`, which already holds every channel's funding and closing block times: the node's channels open at each UTC midnight are a sweep over its rows (one indexed read, 1.6 ms for ACINQ's 1,807). Nothing new is stored. Snapshotting every node's totals instead would add 13,600 rows every period (150 MB a year at one a day) for the same numbers. Before our history starts the table does not know the channels that had already closed, so the series begins at the first statistics snapshot. A channel LDK pruned keeps its row as open ([Still missing](#still-missing)) and is counted open.

### Funding transactions

The peer never learns a channel's funding txid ([Why the script and not the outpoint](#how-closes-are-found)), but the transaction page needs to find the channels a transaction opened. So a background task asks the node once per stored channel, `/block/:hash/txid/:index` with the funding block's hash the table already holds, and stores the answer in `ln_channels.open_txid`, indexed. One small request at a time, at least 50 ms apart: on mainnet about 13 a second at the node's latency, so the first start fills 36,000 channels in about 45 minutes, and after that a few hundred new channels a day. A failure is retried after 1 s, doubling to 64 s (`lightning_funding_failed`); a block that left the chain is skipped until the history moves the row. A txid is stored only while the row names the block it was read from, and an open that moves the channel to another block clears it. Requests never wait for this: until a channel's txid is known, its routes say `null` and a lookup by that txid does not find it. Closing txids come with the close and are indexed the same way.

### Still missing

- The Lightning pages (W11), which read the [routes](#routes).
- `/api/v1/lightning/penalties`: its rows are the reference's database records of a closed channel (each end's last policy, balances, outputs, resolution state), which we do not keep ([`compat/api-v1.md`](compat/api-v1.md#lightning)). Breaches show in our closed-channel lists.
- A channel's close is lost when the history store falls past the peer's memory ([`STORE.md`](STORE.md#ln_channels)), and channels the peer stops watching (pruned) keep their row as open.

### Measured in the server

2026-11-12, the release server on an Apple M4, mainnet only, read-only against a synced rbitcoin v0.9.2 on the same private network, mempool engine and backfill off, the default peers, an empty data directory. It ran 16 minutes, was stopped (SIGTERM) and restarted from the files it left, then ran on. Footprint is the process's physical footprint (`footprint`), sampled every 10 s; the same server with Lightning off sat at 90 MB.

| | First start | Restart from the graph file |
|---|---|---|
| Startup | The peer started 25 ms after the server listened; three peers connected 2 s later. Nothing else waited: the first block summaries and the difficulty answer went out meanwhile | The 35 MB graph file loaded in 63 ms, inside the peer's task |
| Graph usable | All 36,193 channels verified after 128 s; `synced` after 238 s (36,219 channels, 13,639 nodes) | 36,219 channels checked again; `synced` after 68 s |
| Node load | 165 funding-output requests (36,220 outputs) by the sync, none after | 123 requests (36,223 outputs) |
| History stored | One row per channel as it was verified, in step: 30,930 of 30,930 records after a minute, 36,219 rows (5.5 MB) by the sync | The 36,219 `Opened` records arrived again; the table was identical after them (same row count, capacity and funding-time sums) |
| Footprint | 270 MB at its peak, so the peer adds about 180 MB | 380 MB while the loaded graph and the new one coexist; over the next 2.5 hours a median of 334 MB and at most 423 MB: plan for 250 to 330 MB more than without Lightning |
| Shutdown | Graph written (35 MB) in 124 ms, then the server exited | After 2.5 hours: graph written (33 MB) in 1.2 s, nothing left unstored |

**Steady state.** The close-watcher's requests are the tip every 10 s, a page of headers and the raw block per block, and one position request per close: 962 requests and 15 blocks read in 2.5 hours, about 385 an hour, nearly all of them the tip. Validation asked for 28 more outputs (new channels) in that time. When the node's proxy answered `502` for a few seconds, `lightning_watch_failed` was logged three times and the watcher went on by itself. A statistics snapshot took 0.3 to 2.1 ms to store; 16 were stored. The routes answered in 0.03 to 1.1 ms of server time (a list of 100 channels with aliases 1.0 ms).

**Closes.** 30 closes in the 13 blocks from 975,557 to 975,569 (the first 45 minutes of the run had no block at all): 27 cooperative and 3 force, no penalty, no reorg. The table held exactly those 30 closed rows. Four were checked by hand against the node: each spending transaction is the funding output's spender in the block recorded, with the fee and capacity recorded; the cooperative ones have the `closing_signed` shape (one input, sequence `0xffffffff`, locktime 0) and the force close both commitment markers and a 330-sat anchor.

**Against the reference.** `GET /api/v1/lightning/statistics/latest` at 13:50 UTC said 36,219 channels, 13,639 nodes, 3,232.02 BTC, average capacity 8,923,540 sats, median 1,000,000, average fee rate 981 ppm and base fee 1,439 msat, medians 130 ppm and 500 msat. mempool.space's public answer was still dated 2026-08-30 and its series were empty, so there is no same-moment figure: there it read 32,518 channels, 16,225 nodes, 3,753.7 BTC, average 11,543,473 sats, median 2,010,000, 823 ppm and 904 msat, medians 100 ppm and 500 msat. Its newest point in the three-month series, 2026-11-03, read 28,940 channels and 3,194.7 BTC. The differences:

- **Channels and capacity:** we count only channels whose funding output we found unspent, six deep and paying the announced keys, as of now; they count what gossip claims, in their own database. Their own series moved from 32,500 to 28,900 channels between August and November, and the capacity agrees within 1.2 % with their November point.
- **Nodes and their kinds:** LDK keeps a node only while it has a channel in the graph, and 6,628 of ours sent no announcement we received (so no address: `unannounced`), against their 2,022. Their database keeps nodes and their last announcement much longer. Tor, clearnet and both are counted the same way from the addresses announced.
- **Fees:** ours leave policies over 1,000,000 ppm or 1,000,000 msat out of the averages (118 and 274 on mainnet; [`STORE.md`](STORE.md#ln_statistics)); we do not know their rule. Our medians count every policy, disabled ones included.

## Bounds

Gossip is untrusted input ([`SECURITY.md`](SECURITY.md)). Validation makes a channel cost an attacker a real funding output, but the graph is bounded anyway, and so is the validation itself.

- **Ceiling.** At most `Config::max_channels` (100,000) channels; announcements of new channels beyond it are refused and `Event::CeilingReached` is sent. The check and the insert happen under one lock, so concurrent peers cannot overshoot it. Nodes enter the graph only through channels, so they are bounded by twice that.
- **Message size.** Gossip messages over 1 KB are refused; real ones are under 500 bytes. The exception is node announcements: about 50 real nodes announce one onion address tens to a thousand times (up to 38 KB). Those are checked against their signature and kept with at most 8 distinct addresses and no unknown fields, unsigned (which only matters for relaying, and we do not relay).
- **Memory.** So each channel and node held is bounded, and with the ceiling so are the graph's memory and its file. At mainnet's mix, 4 KB per channel, the ceiling is about 400 MB; filled by an attacker with the largest messages accepted, about twice that, and a file of at most 8 KB per channel. `Stats` reports the counts, and `Event::GraphSaved` the file's size.
- **Disk.** Nothing gossip sends is written anywhere but the graph file, whose size follows from the ceiling.
- **Range sync.** The channel ids wanted from peers are capped at the ceiling.
- **Funding-output lookups** are deduplicated, cached, batched (up to 300 per request), queued in a bounded queue that pauses gossip when full, and rate-limited ([Settings and bounds](#settings-and-bounds)), so no amount of gossip can load the node beyond one request per `lookup_interval`.
- **The close-watcher** asks the node for each block once, and for one small request per real close, whatever gossip says ([Bounds and cost](#bounds-and-cost)).
- Aliases, colours and addresses are kept as LDK decoded them, never interpreted, and are served and rendered as plain text.
- The peer makes outbound connections only, never listens, never announces itself and never relays.

## Limits

- **History starts when the peer first runs.** Channels that closed before then are not in gossip, and a channel gossiped after it closed is refused as `spent` without a close record. After that, closes during downtime are found from the graph file ([Catch-up](#catch-up)), for channels the file held.
- **A channel that closes before it is six blocks deep is never announced,** so it is never seen, like a private one.
- **A channel LDK prunes** (no update in either direction for two weeks) is no longer watched, so its later close is not recorded. It comes back, watched again, if gossip announces it again.
- **Private channels are invisible,** as on every explorer.
- **Tor-only nodes cannot be peers.** Their gossip still arrives through clearnet peers.
- **No world map.** That needs a GeoIP database and is out of scope.

## Switch

Lightning is optional and off by default: a network runs it only with `{NET}_LIGHTNING_ENABLED=true` and a `DATA_DIR` ([`CONFIG.md`](CONFIG.md#lightning)).
