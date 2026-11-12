# What we rely on from rbitcoin

Pinned to **rbitcoin v0.9.1**. Everything here was read from that tag's source and docs, or measured against a synced v0.9.1 mainnet node. When the pin moves, re-check this page.

Upstream references: `COMPAT.md`, `docs/rpc.md`, `docs/lightning.md` and `docs/esplora-openapi.yaml` in the rbitcoin repo.

## Interfaces

| Interface | Flag | We use it for |
|---|---|---|
| Esplora REST | `--esplora-listen` | Blocks, transactions, addresses, outspends, broadcast, fee estimates |
| Electrum TCP | `--electrum-listen` | Push: new headers, tracked scripthashes, tracked outpoints |
| JSON-RPC | `--rpc-listen` | Mempool listing, cluster chunks, block template, block stats |
| Address index | `--sh-index` | Address and scripthash routes. Off by default in rbitcoin; required for address pages |

RPC accepts a Bearer token or a Core-style cookie over HTTP Basic. There is no `--rpcuser` / `--rpcpassword`.

## What the node gives us for free

- **Transaction and address index.** No separate indexer.
- **Spend records.** `/tx/:txid/outspend(s)` answers who spent an output.
- **Cluster mempool.** `getmempoolcluster` and `getmempoolfeeratediagram` expose the same chunks mining selection uses, so projected blocks need no template builder of our own.
- **Per-transaction fee, size and weight on disk** (`txstat`), which makes `getblockstats` cheap.
- **Output lookup by position:** `GET /block-height/:height/tx/:index/out/:vout`, and a batch form `POST /block-heights/txs/outs` taking up to 300. Built for Lightning gossip validation and needs no address index.
- **Chain-tip stamps.** Block, transaction and tip routes carry `X-Bitcoin-Chain-Tip` and `X-Bitcoin-Chain-Tip-Height`. If two sequential fetches disagree, the tip moved between them: discard and retry. Address and scripthash routes are stamped with the address index's position, which can trail the tip. Mempool, fee-estimate and broadcast routes carry no stamp (the paged `/mempool/txids/page` does). `?asof=<blockhash>` pins a read to an ancestor.

## What it does not give us

| Missing | Consequence |
|---|---|
| Any mempool push (no ZMQ, WebSocket or SSE) | We poll the mempool |
| `/internal/*` bulk routes over TCP | They exist only on a unix-socket listener, so bulk fetches need the server on the node's host |
| Address-prefix search | Search is exact match only |
| A large recent-transactions feed | `/mempool/recent` returns 10 entries; it cannot serve as a change feed |
| The mempool.space `/api/v1` and WebSocket surface | A permanent non-goal upstream. This project is that surface |

## Measurements

Mainnet, 54,297 transactions in the mempool, client and node on the same private network.

| Call | Time | Response |
|---|---|---|
| `getmempoolinfo` | 10 ms | tiny |
| Electrum request (version, headers, mempool info) | 4–5 ms | tiny |
| Position lookup, single | 28 ms | 0.5 KB |
| Position lookup, batch of 3 | 14 ms | 1 KB |
| `getblockstats` | 97 ms | tiny |
| Electrum `mempool.get_fee_histogram` | 151 ms | 5 KB |
| `GET /fee-estimates` | 200 ms | 0.3 KB |
| `getrawmempool` (txids) | 260 ms | 3.6 MB |
| `GET /mempool` | 320 ms | 6 KB |
| `getmempoolfeeratediagram` | 330 ms | 309 KB, 9,441 chunks |
| `getblocktemplate` | 390 ms | 4.9 MB, 7,501 transactions |
| `getblock` verbosity 2 | 550–860 ms | 10.4 MB, 4,200 transactions |
| `getrawmempool` verbose | 5.7 s | 39.5 MB |

## Rules that follow

- **Never poll verbose `getrawmempool`.** Use it at most once at startup. Diff txids and fetch details for new ones.
- **Compute fee-rate percentiles ourselves.** `getblockstats` rounds to whole sat/vB, and most transactions now pay under 1 sat/vB, so its percentiles come back as zeros.
- **Expect 503 on huge addresses.** Unpaged scripthash joins above `--max-sh-creates` (default 10,000) are refused. Use the paged routes and surface the limit to the user.
- **Fee estimates are a 10-minute inclusion product,** not Core's historical estimator.
- **rbitcoin's mempool policy is libre-relay class:** 0.1 sat/vB minimum, no dust limit, full RBF. Do not assume Core's standardness rules in the UI.
- **`getmempoolfeeratediagram` returns one point per chunk,** in mining order, not Core's running total.
- **`getblock` verbosity 2 carries every transaction's fee** (the coinbase has none). Its `vsize` is the plain BIP141 size, not adjusted for sigops.
- **`getmininginfo.networkhashps` does not fit a `u64`** on mainnet.
- **`getblocktemplate` on signet needs the `signet` rule;** mainnet and testnet4 accept it too, so we always send it.
- **P2P is BIP324 v2 only.** Irrelevant to us unless something tries to peer with the node.

## Upstream wishlist

Not required. Each would make this project faster or simpler, and is recorded here instead of being patched in.

| Wish | Why |
|---|---|
| A mempool event stream (added, removed with reason, sequence number) | Removes the polling lag of up to one interval and the 260 ms diff |
| `/internal/*` over an authenticated TCP listener | Bulk fetches when the server is not on the node's host |
| Sub-sat precision in `getblockstats` percentiles | We could stop recomputing them |
| A paged or capped address-totals route | Addresses above `--max-sh-creates` get no totals or unspent list today, only paged history |
