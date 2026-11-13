<img src="assets/icon.svg" width="64" height="64" alt="">

# rbitcoin.space

A fast block explorer for [rbitcoin](https://github.com/rbitcoin-org/rbitcoin): live mempool, blocks, mining and Lightning.

This project was called mempool.based until 2026-11-12, when its repository moved from `smolcars/mempool.based` to [`smolcars/rbitcoin-space`](https://github.com/smolcars/rbitcoin-space).

> **Status: approaching its first release, `v0.1.0`.** On mainnet, signet and testnet4 it serves live blocks, the live mempool with projected blocks and fee recommendations, block, transaction and address pages, replacements, mining history with pools and block audits, and the Lightning graph. What is left, and why, is in [`docs/COMPAT.md`](docs/COMPAT.md#what-is-left).

## What it is

- **One Rust server** that runs beside a stock rbitcoin node (pinned to v0.9.2) and talks to it over Esplora, Electrum and JSON-RPC.
- **A static Solid frontend** served by that same server.
- **SQLite** for the little data the node does not already store.
- **A gossip-only LDK peer** for the Lightning explorer. It holds no funds and opens no channels.

Mainnet, signet and testnet4 are supported, plus regtest for development.

## Why

rbitcoin already keeps the transaction index, address index, cluster mempool and per-transaction fee data in one process. An explorer built for it needs no second indexer, no second copy of the mempool and no database server.

## Run it

You need a Linux host with Docker and its Compose plugin, on amd64 or arm64, with room for rbitcoin's chain. [`deploy/compose.yml`](deploy/compose.yml) runs rbitcoin v0.9.2 and the explorer beside it: [`docs/DEPLOY.md`](docs/DEPLOY.md#same-host-with-the-compose-file) has the commands to start both from a release, and is the operator's guide for everything after, from TLS and the first backfill to upgrades. The explorer is then at `http://127.0.0.1:8080` on that host while the node syncs; put it behind a reverse proxy with TLS before anyone else uses it. To try it first, the same compose file runs a private regtest chain ([`docs/DEPLOY.md`](docs/DEPLOY.md#try-it-on-regtest-first)).

## Develop

Needs Rust (the toolchain in `rust-toolchain.toml` installs itself through rustup), [bun](https://bun.sh), and for the private regtest node Docker with Compose v2.23 or later and `python3`.

```sh
# A private regtest node with a known chain (docs/REGTEST.md), and a .env pointing at it.
# For a node of your own, fill in the MAINNET_ lines of .env.example instead (docs/CONFIG.md).
dev/regtest/regtest.py scenario > /dev/null
grep -v '^MAINNET_' .env.example > .env && dev/regtest/regtest.py env >> .env

# Build the frontend, then run the server on it, at http://127.0.0.1:8080/regtest/
(cd web && bun install && bun run build)
set -a && . ./.env && set +a         # the server reads the environment, never .env itself
cargo run -p rbitcoin-space

# Frontend with hot reload, proxying the server above
(cd web && bun run dev)

# Test and lint: what CI runs
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test
(cd web && bun run typecheck && bun run lint && bun run test && bun run build && bun run size)
(cd web && bun run format)            # oxfmt rewrites files; `lint` only checks them
(cd web && bun run e2e)               # browser tests; needs Docker
```

## Documentation

| Read this | For |
|---|---|
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | How the pieces fit together |
| [`docs/PLAN.md`](docs/PLAN.md) | Milestones and what each one delivers |
| [`docs/TODO.md`](docs/TODO.md) | The task list and its current state |
| [`docs/API.md`](docs/API.md) | The public Esplora API at `/api` and how it differs from the node |
| [`docs/COMPAT.md`](docs/COMPAT.md) | Feature parity with mempool.space |
| [`docs/RBITCOIN.md`](docs/RBITCOIN.md) | What we rely on from rbitcoin, with measurements |
| [`docs/MEMPOOL.md`](docs/MEMPOOL.md) | How the mempool is tracked, why each removal happened, and the projected blocks |
| [`docs/SECURITY.md`](docs/SECURITY.md) | Threat model and the rules that follow from it |
| [`docs/WS.md`](docs/WS.md) | The WebSocket protocol at `/app/ws`: messages, topics, limits and close codes |
| [`docs/CONFIG.md`](docs/CONFIG.md) | Every environment variable, its default and what it protects against |
| [`docs/CI.md`](docs/CI.md) | CI speed budget and testing layers |
| [`docs/REGTEST.md`](docs/REGTEST.md) | The private regtest node for development and tests: its commands and the known chain |
| [`docs/PERFORMANCE.md`](docs/PERFORMANCE.md) | How fast the server and the site are against the budgets, how that was measured, and how they compare |
| [`docs/STORE.md`](docs/STORE.md) | The SQLite store: migrations, writes, reads, backup |
| [`docs/LOGGING.md`](docs/LOGGING.md) | Structured logging: the event catalogue and its rules |
| [`docs/LIGHTNING.md`](docs/LIGHTNING.md) | How the Lightning explorer works |
| [`docs/DEPLOY.md`](docs/DEPLOY.md) | Running it: the image, the compose file, TLS, the data directory, upgrades, what to watch |
| [`docs/DESIGN.md`](docs/DESIGN.md) | Visual design |
| [`docs/FRONTEND.md`](docs/FRONTEND.md) | Frontend structure, data layer, shared components and rules for page builders |
| [`AGENTS.md`](AGENTS.md) | Rules for contributors, human or agent |

## License

[MIT](LICENSE). Not affiliated with mempool.space or Mempool Space K.K.
