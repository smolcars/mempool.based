#!/usr/bin/env bash
# shellcheck disable=SC2034
# The server under load, on a private regtest node: every step in docs/PERFORMANCE.md "The server
# under load", one JSON file per step in OUT. Never point it at a shared node.
#   dev/perf/bench.sh OUT [http|limits|ws|idle ...]     (default: all four)
# Needs docker, python3, and the release builds: `cargo build --release --locked -p mempool-based
# -p load` and `(cd web && bun run build)`. Each step is at most two minutes, with GAP seconds
# (default 15) between steps so the machine settles. Takes about 20 minutes for all four.
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
perf="$root/dev/perf"
out=$(mkdir -p "$1" && cd "$1" && pwd)
shift
phases=${*:-idle http limits ws}
gap=${GAP:-15}
load="${CARGO_TARGET_DIR:-$root/target}/release/load"
case "$load" in /*) ;; *) load="$root/$load" ;; esac
export REGTEST_PROJECT=${REGTEST_PROJECT:-perf-$$}
server="" activity="" srv_log=""

stop_server() {
  if [ -n "$server" ]; then
    kill "$server" 2>/dev/null || true
    wait "$server" 2>/dev/null || true
    server=""
  fi
}
cleanup() {
  [ -n "$activity" ] && kill "$activity" 2>/dev/null
  stop_server
  python3 "$root/dev/regtest/regtest.py" down >/dev/null 2>&1 || true
}
trap cleanup EXIT
trap 'exit 130' INT TERM

log() { echo "perf: $(date -u +%H:%M:%S) $*" >&2; }

# start NAME VAR=value...: a fresh server with a fresh store.
start() {
  stop_server
  local name=$1
  shift
  srv_log="$out/$name/server.log"
  read -r addr server < <("$perf/server.sh" "$out/$name" "$@")
  echo "$*" >"$out/$name/settings"
  # Let it follow the tip, fill its block table and project the mempool.
  for _ in $(seq 60); do
    curl -sf "http://$addr/regtest/api/v1/fees/mempool-blocks" >/dev/null && break
    sleep 1
  done
  sleep 5
}

# activity RATE BLOCK_EVERY: transactions in the background until the next call or the end.
activity() {
  [ -n "$activity" ] && kill "$activity" 2>/dev/null && wait "$activity" 2>/dev/null
  python3 "$perf/activity.py" flow --rate "$1" --seconds 3600 --block-every "$2" \
    2>>"$out/activity.log" &
  activity=$!
}

# step NAME LOAD-ARGS...: one load run, then the server's own timings for the same window.
step() {
  local name=$1
  shift
  local t0
  t0=$(date +%s)
  log "$name"
  "$load" "$@" --pid "$server" >"$out/$name.json"
  python3 "$perf/serverlog.py" "$srv_log" \
    --from "$t0" --to "$(date +%s)" >"$out/$name.server.json"
  sleep "$gap"
}

log "node $REGTEST_PROJECT"
python3 "$root/dev/regtest/regtest.py" scenario >"$out/manifest.json" 2>"$out/regtest.log"
python3 "$perf/activity.py" prepare --outputs 20000 2>>"$out/activity.log"
m() { python3 -c "import json,sys; m=json.load(open('$out/manifest.json')); print(eval(sys.argv[1]))" "$1"; }
block=$(m "m['blocks']['106']")
tx=$(m "m['txs']['script_types']")
address=$(m "m['addresses']['p2wpkh']")
r=/regtest
# Classes of route: answered from memory, immutable (passed to the node), from our store, or by
# asking the node. Read through ${!class} below, which shellcheck cannot follow (SC2034).
memory="$r/api/v1/fees/mempool-blocks $r/api/v1/difficulty-adjustment $r/api/blocks/tip/height $r/app/block/$block/summary"
immutable="$r/api/block/$block"
store="$r/api/v1/blocks $r/api/v1/mining/pools/1w $r/api/v1/statistics/24h $r/api/v1/block/$block"
node="$r/api/tx/$tx $r/api/block/$block/txs $r/api/tx/$tx/outspends $r/api/address/$address"
# Per-client limits out of the way, so many clients from one machine measure the server itself.
unlimited="CLIENT_RATE_PER_SEC=1000000 CLIENT_BURST=1000000"

for phase in $phases; do
  case $phase in
  idle)
    start idle
    step idle-quiet ws "ws://$addr$r/app/ws" --sockets 0 --seconds 60
    activity 20 30
    step idle-activity ws "ws://$addr$r/app/ws" --sockets 0 --seconds 90
    ;;
  http)
    activity 5 60
    # shellcheck disable=SC2086
    start http $unlimited
    for class in memory immutable store node; do
      for clients in 1 100 1000; do
        # shellcheck disable=SC2086
        step "http-$class-$clients" http "http://$addr" ${!class} --clients "$clients" --ips "$clients" --seconds 15
      done
    done
    ;;
  limits)
    # Every limit at its default.
    activity 5 60
    start limits
    # An honest heavy user: one address, six connections, 24 requests a second in all.
    # shellcheck disable=SC2086
    step limits-heavy-user http "http://$addr" $memory $node --clients 6 --ips 1 --rate 4 --seconds 30
    # 100 ordinary clients at 2 requests a second, alone and then beside one client flooding.
    # shellcheck disable=SC2086
    step limits-victims-alone http "http://$addr" $memory $store --clients 100 --ips 100 --rate 2 --seconds 30
    # shellcheck disable=SC2086
    # The flood names no address, so it is 127.0.0.1, apart from the victims.
    "$load" http "http://$addr" $memory --clients 64 --seconds 30 >"$out/limits-flood.json" &
    flood=$!
    # shellcheck disable=SC2086
    step limits-victims-flooded http "http://$addr" $memory $store --clients 100 --ips 100 --rate 2 --seconds 30
    wait "$flood"
    ;;
  ws)
    # Raised for the test: sockets in all (default 1,024). Each socket is its own client address.
    activity 20 30
    start ws WS_MAX_SOCKETS=20000
    sleep 15
    for sockets in 100 1000 10000; do
      step "ws-$sockets" ws "ws://$addr$r/app/ws" --sockets "$sockets" --ips "$sockets" --block-every 10 \
        --seconds 90 --server-log "$out/ws/server.log"
    done
    # Clients that never read, beside 1,000 that do.
    step ws-slow ws "ws://$addr$r/app/ws" --sockets 1000 --ips 1050 --slow 50 --block-every 10 \
      --seconds 110 --server-log "$out/ws/server.log"
    ;;
  esac
done
log "done; results in $out"
