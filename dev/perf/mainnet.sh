#!/usr/bin/env bash
# Light, read-only measurements on mainnet (docs/PERFORMANCE.md): the server's footprint idle and
# while it tracks the live mempool, the new-block latency as a subscriber sees it, and a few paced
# requests per route, ours and, when REFERENCE is given, a stock mempool backend's.
#   dev/perf/mainnet.sh OUT MINUTES [REFERENCE]
# REFERENCE is the reference backend's base URL (http://host:port, serving /api/v1). Reads the
# MAINNET_* variables from .env at the repository root; only mainnet is enabled. Never a load
# test: about 5 requests a second, 50 per route, one client. The mempool engine polls the node
# (about 150 MB a minute) for MINUTES, then the server is stopped.
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
out=$(mkdir -p "$1" && cd "$1" && pwd)
minutes=$2
reference=${3:-}
load="${CARGO_TARGET_DIR:-$root/target}/release/load"
bin="${CARGO_TARGET_DIR:-$root/target}/release/mempool-based"
server="" client=""
cleanup() {
  [ -n "$client" ] && kill "$client" 2>/dev/null
  [ -n "$server" ] && kill "$server" 2>/dev/null && wait "$server" 2>/dev/null
  return 0
}
trap cleanup EXIT
trap 'exit 130' INT TERM
log() { echo "perf: $(date -u +%H:%M:%S) $*" >&2; }

# start NAME ENGINE: a mainnet-only server, its store in OUT/NAME, backfill off.
start() {
  mkdir -p "$out/$1"
  (
    set -a
    # shellcheck disable=SC1091
    . "$root/.env"
    set +a
    exec env -i PATH="$PATH" LISTEN_ADDR=127.0.0.1:0 WEB_DIR="$root/web/dist" DATA_DIR="$out/$1/data" \
      LOG_FORMAT=json BACKFILL_ENABLED=false BROADCAST_ENABLED=false MAINNET_MEMPOOL_ENABLED="$2" \
      MAINNET_RPC_HOST="$MAINNET_RPC_HOST" MAINNET_RPC_PORT="$MAINNET_RPC_PORT" \
      MAINNET_RPC_USER="$MAINNET_RPC_USER" MAINNET_RPC_PASSWORD="$MAINNET_RPC_PASSWORD" \
      MAINNET_ESPLORA_URL="$MAINNET_ESPLORA_URL" MAINNET_ELECTRUM_ADDR="$MAINNET_ELECTRUM_ADDR" "$bin"
  ) >"$out/$1/server.log" 2>&1 &
  server=$!
  addr=""
  for _ in $(seq 100); do
    addr=$(grep -m1 '"event":"listening"' "$out/$1/server.log" | grep -oE '127\.0\.0\.1:[0-9]+' || true)
    [ -n "$addr" ] && break
    sleep 0.1
  done
  [ -n "$addr" ] || { cat "$out/$1/server.log" >&2; exit 1; }
}

# sample FILE SECONDS: the server's CPU seconds and resident KB every 10 s.
sample() {
  local end=$((SECONDS + $2))
  echo "elapsed_s,cpu_s,rss_kb" >"$1"
  local t0=$SECONDS
  while [ $SECONDS -lt $end ]; do
    echo "$((SECONDS - t0)),$(ps -o time=,rss= -p "$server" | awk '{print $1","$2}')" >>"$1"
    sleep 10
  done
}

# paced NAME BASE PATH...: 50 requests per path at 5 a second, one client.
paced() {
  local name=$1 base=$2
  shift 2
  for p in "$@"; do
    "$load" http "$base" "$p" --clients 1 --rate 5 --seconds 10
  done | python3 -c '
import json, sys
print(json.dumps({d["paths"].popitem()[0]: {"status": d["status"], "ok_ms": d["ok_ms"]} for d in map(json.loads, sys.stdin)}, indent=1))
' >"$out/$name.json"
}

log "idle, engine off"
start idle false
sample "$out/idle.csv" 180
cleanup
server=""

log "engine on for $minutes minutes"
start live true
# One subscriber to what the home page shows, for the new-block and mempool fan-out times.
"$load" ws "ws://$addr/app/ws" --sockets 1 --seconds $((minutes * 60 - 30)) \
  --server-log "$out/live/server.log" >"$out/live-client.json" &
client=$!
sample "$out/live.csv" $((minutes * 60)) &
sampler=$!
sleep 600
log "paced requests, ours"
tip=$(curl -sf "http://$addr/api/blocks/tip/hash")
routes="/api/v1/fees/recommended /api/v1/fees/mempool-blocks /api/v1/difficulty-adjustment /api/v1/blocks /api/v1/block/$tip /api/v1/statistics/2h /api/blocks/tip/height /api/block/$tip"
# shellcheck disable=SC2086
paced ours "http://$addr" $routes
if [ -n "$reference" ]; then
  log "paced requests, reference"
  # shellcheck disable=SC2086
  paced reference "$reference" $routes
  # The network's own share: TCP connect time to each, ten times.
  for b in "http://$addr" "$reference"; do
    for _ in $(seq 10); do curl -s -o /dev/null -w '%{time_connect}\n' "$b/api/blocks/tip/height"; done |
      sort -n | awk -v b="$b" '{a[NR]=$1} END {print (b ~ /127.0.0.1/ ? "ours" : "reference") ",connect_ms_median," a[int((NR+1)/2)]*1000}'
  done >"$out/connect.csv"
fi
wait "$sampler"
wait "$client" || true
client=""
log "done; results in $out"
