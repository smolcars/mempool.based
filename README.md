<img src="assets/icon.svg" width="64" height="64" alt="">

# mempool.based

A fast block explorer for [rbitcoin](https://github.com/rbitcoin-org/rbitcoin): live mempool, blocks, mining and Lightning.

> **Status: early development.** Nothing here is ready to run yet. Progress is tracked in [`docs/TODO.md`](docs/TODO.md).

## What it is

- **One Rust server** that runs beside a stock rbitcoin node (pinned to v0.9.2) and talks to it over Esplora, Electrum and JSON-RPC.
- **A static Solid frontend** served by that same server.
- **SQLite** for the little data the node does not already store.
- **A gossip-only LDK peer** for the Lightning explorer. It holds no funds and opens no channels.

Mainnet, signet and testnet4 are supported, plus regtest for development.

## Why

rbitcoin already keeps the transaction index, address index, cluster mempool and per-transaction fee data in one process. An explorer built for it needs no second indexer, no second copy of the mempool and no database server.

## Quick start

Needs Rust (the toolchain in `rust-toolchain.toml` installs itself through rustup) and [bun](https://bun.sh).

```sh
# Build the frontend, then run the server on it
(cd web && bun install && bun run build)
cargo run -p mempool-based          # LISTEN_ADDR and WEB_DIR are read from the environment

# Frontend with hot reload, proxying the server above
(cd web && bun run dev)

# Test and lint: what CI runs
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test
(cd web && bun run typecheck && bun run lint && bun run test && bun run build && bun run size)
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
| [`docs/MEMPOOL.md`](docs/MEMPOOL.md) | How the mempool is tracked, and why each removal happened |
| [`docs/SECURITY.md`](docs/SECURITY.md) | Threat model and the rules that follow from it |
| [`docs/CONFIG.md`](docs/CONFIG.md) | Every environment variable, its default and what it protects against |
| [`docs/CI.md`](docs/CI.md) | CI speed budget and testing layers |
| [`docs/LOGGING.md`](docs/LOGGING.md) | Structured logging: the event catalogue and its rules |
| [`docs/LIGHTNING.md`](docs/LIGHTNING.md) | How the Lightning explorer works |
| [`docs/DEPLOY.md`](docs/DEPLOY.md) | The Docker image and the compose file that runs it beside rbitcoin |
| [`docs/DESIGN.md`](docs/DESIGN.md) | Visual design |
| [`docs/FRONTEND.md`](docs/FRONTEND.md) | Frontend structure, data layer, shared components and rules for page builders |
| [`AGENTS.md`](AGENTS.md) | Rules for contributors, human or agent |

## License

[MIT](LICENSE). Not affiliated with mempool.space or Mempool Space K.K.
