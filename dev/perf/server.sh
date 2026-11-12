#!/usr/bin/env bash
# Starts the release server against the regtest node that dev/regtest/regtest.py runs (same
# REGTEST_PROJECT), on a free port, with its data and log in DIR. Prints `ADDR PID` once it listens.
#   dev/perf/server.sh DIR [VAR=value ...]
# Extra VAR=value pairs are passed to the server: this is how a test raises a limit, never by
# changing a default. Clients name themselves in X-Forwarded-For, trusted from 127.0.0.1 only.
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
dir=$1
shift
mkdir -p "$dir"
eval "$(python3 "$root/dev/regtest/regtest.py" env | sed 's/^/export /')"
bin="${CARGO_TARGET_DIR:-$root/target}/release/mempool-based"
case "$bin" in /*) ;; *) bin="$root/$bin" ;; esac
[ -x "$bin" ] || { echo "perf: build first: cargo build --release --locked -p mempool-based" >&2; exit 1; }
[ -d "$root/web/dist" ] || { echo "perf: build the frontend first: (cd web && bun run build)" >&2; exit 1; }

env -i PATH="$PATH" LISTEN_ADDR=127.0.0.1:0 WEB_DIR="$root/web/dist" DATA_DIR="$dir/data" LOG_FORMAT=json \
  TRUSTED_PROXY_HEADER=X-Forwarded-For TRUSTED_PROXIES=127.0.0.1 BROADCAST_ENABLED=false \
  REGTEST_RPC_HOST="$REGTEST_RPC_HOST" REGTEST_RPC_PORT="$REGTEST_RPC_PORT" \
  REGTEST_RPC_USER="$REGTEST_RPC_USER" REGTEST_RPC_PASSWORD="$REGTEST_RPC_PASSWORD" \
  REGTEST_ESPLORA_URL="$REGTEST_ESPLORA_URL" REGTEST_ELECTRUM_ADDR="$REGTEST_ELECTRUM_ADDR" \
  "$@" "$bin" >"$dir/server.log" 2>&1 &
pid=$!
for _ in $(seq 100); do
  addr=$(grep -m1 '"event":"listening"' "$dir/server.log" | grep -oE '127\.0\.0\.1:[0-9]+' || true)
  if [ -n "$addr" ]; then
    echo "$addr $pid"
    exit 0
  fi
  kill -0 "$pid" 2>/dev/null || { cat "$dir/server.log" >&2; exit 1; }
  sleep 0.1
done
kill "$pid"
echo "perf: the server did not start" >&2
exit 1
