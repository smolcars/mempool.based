#!/usr/bin/env bash
# Browser tests in one command: a private regtest node with the known chain, the server and the
# frontend built from this checkout, the server on a free port, then Playwright. Everything it
# starts is named for this run, so runs on one machine never collide. Arguments go to Playwright.
set -euo pipefail

web=$(cd "$(dirname "$0")/.." && pwd)
root=$(dirname "$web")
regtest="$root/dev/regtest/regtest.py"
run=$(mktemp -d "${TMPDIR:-/tmp}/mempool-based-e2e.XXXXXX")
# Never a node the caller set up: scenario resets the chain it is pointed at.
export REGTEST_PROJECT="e2e-$$"
server=""

cleanup() {
  local status=$?
  if [ -n "$server" ]; then kill "$server" 2>/dev/null || true; fi
  python3 "$regtest" down >/dev/null 2>&1 || true
  if [ "$status" -eq 0 ]; then
    rm -rf "$run"
  else
    [ -f "$run/server.log" ] && tail -n 20 "$run/server.log" >&2
    echo "e2e: failed; logs and traces are in $run" >&2
  fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# The node builds its chain while the server and frontend compile.
python3 "$regtest" scenario >"$run/manifest.json" 2>"$run/regtest.log" &
node_pid=$!

cd "$web"
[ -d node_modules ] || bun install --frozen-lockfile
node_modules/.bin/playwright install --only-shell chromium &
browser_pid=$!
bun run build --outDir "$run/dist" --emptyOutDir --logLevel warn
(cd "$root" && cargo build --locked --quiet -p mempool-based)
bin="${CARGO_TARGET_DIR:-$root/target}/debug/mempool-based"
case "$bin" in /*) ;; *) bin="$root/$bin" ;; esac

wait "$browser_pid"
wait "$node_pid" || { cat "$run/regtest.log" >&2; exit 1; }
eval "$(python3 "$regtest" env | sed 's/^/export /')"

# Only the regtest network, whatever the caller's environment holds: tests never reach a shared
# node. One client runs every test, so the per-client limits are raised.
env -i PATH="$PATH" LISTEN_ADDR=127.0.0.1:0 WEB_DIR="$run/dist" LOG_FORMAT=json \
  CLIENT_RATE_PER_SEC=1000 CLIENT_BURST=1000 CLIENT_MAX_CONCURRENT_REQUESTS=256 \
  REGTEST_RPC_HOST="$REGTEST_RPC_HOST" REGTEST_RPC_PORT="$REGTEST_RPC_PORT" \
  REGTEST_RPC_USER="$REGTEST_RPC_USER" REGTEST_RPC_PASSWORD="$REGTEST_RPC_PASSWORD" \
  REGTEST_ESPLORA_URL="$REGTEST_ESPLORA_URL" REGTEST_ELECTRUM_ADDR="$REGTEST_ELECTRUM_ADDR" \
  "$bin" >"$run/server.log" 2>&1 &
server=$!

# Port 0 picks a free port; the server logs which in its `listening` event.
addr=""
for _ in $(seq 100); do
  addr=$(grep '"event":"listening"' "$run/server.log" | grep -oE '127\.0\.0\.1:[0-9]+' || true)
  [ -n "$addr" ] && break
  kill -0 "$server" 2>/dev/null || { echo "e2e: the server exited" >&2; exit 1; }
  sleep 0.1
done
[ -n "$addr" ] || { echo "e2e: the server did not start" >&2; exit 1; }

echo "e2e: ready in ${SECONDS}s, server at http://$addr" >&2
E2E_BASE_URL="http://$addr" E2E_OUTPUT_DIR="$run/results" node_modules/.bin/playwright test "$@"
