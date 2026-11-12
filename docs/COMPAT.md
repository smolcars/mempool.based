# Parity with mempool.space

> **Status: skeleton.** The inventory below is filled in by task R1 ([`TODO.md`](TODO.md)).

This page tracks every mempool.space feature: whether we serve it, how, and if not, why.

## Policy

1. **Feature parity is the goal; being a drop-in is not.** Where rbitcoin lets us do something better, we do it our way and note the difference here.
2. **Esplora is served as rbitcoin serves it,** under `/api`. Clients written for Esplora or electrs work unchanged.
3. **mempool.space-shaped endpoints under `/api/v1` are served when they are cheap** to derive from what we already compute. Each one is a row below.
4. **Our own frontend uses `/app`,** a private API with no compatibility promise.
5. **This inventory is written clean-room.** It describes observable behaviour in our own words. No mempool.space source code is read, and no text or example payload is copied from its documentation.

## Out of scope

| Feature | Why |
|---|---|
| Liquid and its assets | Different chain; rbitcoin does not serve it |
| Accelerator and other paid services | A commercial service, not explorer functionality |
| Translations | English only for now |
| Lightning world map | Needs a GeoIP database |
| Address-prefix search | rbitcoin has no prefix index; search is exact match |

## Status values

| Value | Meaning |
|---|---|
| `served` | Works today |
| `planned` | Assigned to a milestone |
| `different` | We offer the feature another way; the row says how |
| `skipped` | Not planned; the row says why |

## Inventory

To be filled by R1, one table per group:

- Esplora REST (`/api`)
- mempool.space REST (`/api/v1`): general, fees, mempool, blocks, mining, transactions, addresses, Lightning, prices
- WebSocket messages
- Pages

Each row: the feature, what it returns in one line, where the answer comes from in our system (an rbitcoin interface, our engine, our store, or Lightning), its status, and its milestone.
