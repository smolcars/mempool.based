# Deploying

What an operator runs: one image with the server and the built frontend, and a compose file that runs it beside rbitcoin v0.9.2 on the same host. Same host matters: rbitcoin serves its bulk `/internal/*` Esplora routes only on a unix-socket listener ([`RBITCOIN.md`](RBITCOIN.md)), and a socket can only be shared on one machine.

## The image

```sh
docker build -t mempool-based .
```

The [`Dockerfile`](../Dockerfile) has three stages. Base images are multi-arch and pinned by digest; all come from Docker Hub or `gcr.io`.

| Stage | From | Does |
|---|---|---|
| `web` | `oven/bun` | `bun install --frozen-lockfile` and `bun run build`. Runs on the build platform only, since the output is the same on every architecture |
| `server` | `rust` (Debian 13) | `cargo build --release --locked -p mempool-based`. The cargo registry and `target/` are BuildKit cache mounts, so a one-file change recompiles one crate. The Rust tag must match `rust-toolchain.toml`, which is not copied in |
| final | `gcr.io/distroless/cc-debian13:nonroot` | The binary at `/usr/local/bin/mempool-based`, the frontend at `/srv/web`. No shell, no package manager |

Defaults inside the image: `LISTEN_ADDR=0.0.0.0:8080` and `WEB_DIR=/srv/web`. Everything else is configured as in [`CONFIG.md`](CONFIG.md).

It runs as uid and gid 65532, needs no capabilities, and writes nothing, so it runs with a read-only root filesystem. Its health check is the binary itself: `mempool-based healthcheck` asks the server at `LISTEN_ADDR` for `/healthz` and exits 0 on a 200, 1 otherwise. It prints nothing. With `LISTEN_ADDR` port 0 there is nothing to check; do not use port 0 in a container.

[`.dockerignore`](../.dockerignore) is an allow-list: only `Cargo.toml`, `Cargo.lock`, `crates/` and `web/` (without `node_modules/` and `dist/`) reach the build. `.env`, `target/` and everything else stay out.

linux/amd64 and linux/arm64 both build from the same Dockerfile, natively on each. [`image.yml`](../.github/workflows/image.yml) builds both after every merge to `master`, starts the image read-only with no capabilities until it reports healthy, and validates the compose file. It is not part of the pull-request checks and publishes nothing; publishing comes with the release workflow.

Measured on an Apple Silicon Mac, base images already pulled: 12 MB compressed, cold build 16 s, rebuild after changing one Rust file 2 s, after changing one frontend file 1 s. The amd64 build under emulation took 172 s cold.

## Logs

The server and rbitcoin log to stdout and stderr only; nothing is written to a log file inside either container, and the read-only root filesystem would refuse it anyway. Read them with `docker compose logs`. Rotation and shipping belong to Docker's logging driver: the default `json-file` driver does not rotate, so set `max-size` and `max-file` in the daemon's `daemon.json`, or use `journald` or `local`.

## The compose file

[`deploy/compose.yml`](../deploy/compose.yml) runs two services for one network.

```text
 internet ── reverse proxy (TLS) ──► server :8080 ─┬─ unix socket ──► node  Esplora + /internal/*
                                                   ├─ TCP 8332 ─────► node  JSON-RPC
                                                   └─ TCP 50001 ────► node  Electrum
 bitcoin peers ──────────────────────────────────────────────────────► node  P2P (published)
```

| Service | Image | Talks to |
|---|---|---|
| `node` | rbitcoin v0.9.2, pinned by digest. `RBITCOIN_IMAGE` overrides it | Esplora on a unix socket in the `esplora-socket` volume; RPC on 8332 and Electrum on 50001, both on the private compose network; P2P on `P2P_PORT`, the only thing published |
| `server` | Built from this repo's `Dockerfile`. `SERVER_IMAGE` names another | Mounts `esplora-socket` read-only; RPC and Electrum at `node` |

### Run it

```sh
cd deploy
cp .env.example .env                      # then set MAINNET_RPC_PASSWORD
pass=$(openssl rand -hex 32)              # or any long random string
sed -i "s/^MAINNET_RPC_PASSWORD=.*/MAINNET_RPC_PASSWORD=$pass/" .env
printf 'mempool-based:%s' "$pass" > rpc.cookie
sudo chown 10001:10001 rpc.cookie && sudo chmod 600 rpc.cookie
docker compose up -d --build
```

`.env` and `rpc.cookie` are git-ignored. The node reads the RPC user and password from `rpc.cookie` (rbitcoin has no `--rpcuser`; `--rpc-cookie-file` takes `user:password` for HTTP Basic, with no newline). The server reads the same pair from `MAINNET_RPC_USER` and `MAINNET_RPC_PASSWORD`. The cookie is a file and not compose `content`, because compose cannot copy content into a container whose root filesystem is read-only.

The server listens on `127.0.0.1:8080` on the host (`HTTP_PUBLISH` changes it). The node keeps its chain in the `node-data` volume; the first mainnet sync takes as long as rbitcoin takes.

### Hardening, and why

| Choice | Applies to | Why |
|---|---|---|
| `read_only: true` | both | Nothing can be written outside the volumes, so a compromised process cannot drop files or alter its binary. The server writes nothing; the node writes only `/data` |
| `cap_drop: [ALL]` | both | Neither binds a port below 1024 or needs any other privilege |
| `no-new-privileges` | both | A setuid binary cannot raise privileges. Neither image has one; this keeps it so |
| Non-root users | both | The server runs as 65532, the node as 10001, as their images set |
| Only P2P published | node | RPC and Electrum listen on the compose network, Esplora on a socket. The browser never reaches the node ([`SECURITY.md`](SECURITY.md) rule 1) |
| Server on loopback | server | Reached through the reverse proxy alone, which `TRUSTED_PROXY_HEADER` depends on |
| Socket volume is tmpfs, owner 10001, mode 0750 | both | The socket never outlives the stack, and only the node's uid and group can enter the directory |
| `group_add: ["10001"]` | server | rbitcoin creates the socket mode 0660 owned by 10001:10001. The server joins that group to connect, instead of running as the node's uid |
| Socket volume read-only | server | Connecting to a socket needs no write access to its directory |
| Credentials in `.env` and `rpc.cookie` | both | Nothing secret is in an image or in `compose.yml`. The image carries no configuration beyond `LISTEN_ADDR` and `WEB_DIR` |
| `stop_grace_period: 5m` | node | rbitcoin flushes its store on SIGTERM |
| Images pinned by digest | node | The tag cannot be moved under a running deployment |

### TLS and the reverse proxy

TLS is out of scope for the compose file. Put a reverse proxy (Caddy, nginx, …) on the host in front of `127.0.0.1:8080`. It must set one header to the client's address, and the server must be told which:

- nginx: `proxy_set_header X-Real-IP $remote_addr;` with `TRUSTED_PROXY_HEADER=X-Real-IP`
- Caddy: `header_up X-Real-IP {remote_host}` with the same setting

Set `TRUSTED_PROXY_HEADER` only when every request reaches the server through that proxy, which is why the server is published on loopback only ([`CONFIG.md`](CONFIG.md)). Without a proxy, unset it, and every client shares the rate limits of whatever address it connects from.

### Add a network

The compose file runs one node. For a second network, copy the `node` service with its own name, volumes and network, and give the server that network's six variables. For signet beside mainnet:

```yaml
services:
  signet:
    # Everything from `node`, with these differences:
    command:
      - --network=signet
      - --sh-index
      - --listen=0.0.0.0:38333
      - --rpc-listen=0.0.0.0:8332
      - --rpc-cookie-file=/run/secrets/rpc-cookie
      - --electrum-listen=0.0.0.0:50001
      - --esplora-listen=/run/esplora/esplora.sock
    ports: ["38333:38333"]
    volumes:
      - signet-data:/data
      - signet-socket:/run/esplora
  server:
    volumes:
      - esplora-socket:/run/esplora:ro
      - signet-socket:/run/signet:ro

volumes:
  signet-data:
  signet-socket:
    driver_opts: { type: tmpfs, device: tmpfs, o: "uid=10001,gid=10001,mode=0750" }
```

and in `.env`:

```sh
SIGNET_RPC_HOST=signet
SIGNET_RPC_PORT=8332
SIGNET_RPC_USER=mempool-based
SIGNET_RPC_PASSWORD=…                     # the same as in rpc.cookie, or give signet its own secret
SIGNET_ESPLORA_URL=/run/signet/esplora.sock
SIGNET_ELECTRUM_ADDR=signet:50001
```

The server serves it under `/signet` ([`ARCHITECTURE.md`](ARCHITECTURE.md)). P2P ports: mainnet 8333, signet 38333, testnet4 48333, regtest 18444.

### Try it on regtest

The same file runs a regtest node: set `NETWORK=regtest`, `P2P_PORT=18444`, `P2P_PUBLISH=127.0.0.1:0` and `HTTP_PUBLISH=127.0.0.1:0` (Docker picks free host ports), and the six `REGTEST_*` variables in place of `MAINNET_*`. Then `docker compose -p <name> up -d --build` and `docker compose -p <name> port server 8080`. The app is at `/regtest/`. Mine with JSON-RPC from a container on the compose network:

```sh
docker run --rm --network <name>_default curlimages/curl -su "mempool-based:$pass" \
  -d '{"method":"generatetoaddress","params":[101,"bcrt1q…"]}' http://node:8332/
```

`docker compose -p <name> down -v` removes the chain. If `docker pull` from ghcr.io does not work on your machine, load the rbitcoin image another way and set `RBITCOIN_IMAGE=ghcr.io/rbitcoin-org/rbitcoin:0.9.2`, since a loaded image has no digest to match.
