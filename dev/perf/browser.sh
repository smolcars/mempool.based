#!/usr/bin/env bash
# Page loads and frame times on a private regtest node (docs/PERFORMANCE.md "The site in a
# browser"): the scenario chain, a block of about 600 transactions for the grid, the release server,
# then dev/perf/pages.mjs with --frames. Writes OUT/pages.json; removes the node and stops the
# server and the transaction flow when it ends.
#   dev/perf/browser.sh OUT [pages.mjs options, e.g. --runs 5 or --only home]
# Needs the same builds as bench.sh, and Playwright's Chromium (`cd web && bun install`).
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
perf="$root/dev/perf"
out=$(mkdir -p "$1" && cd "$1" && pwd)
shift
export REGTEST_PROJECT=${REGTEST_PROJECT:-perf-$$}
server="" activity=""
cleanup() {
  local pid
  for pid in "$activity" "$server"; do
    if [ -n "$pid" ]; then
      kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    fi
  done
  python3 "$root/dev/regtest/regtest.py" down >/dev/null 2>&1 || true
}
trap cleanup EXIT
trap 'exit 130' INT TERM

python3 "$root/dev/regtest/regtest.py" scenario >"$out/manifest.json" 2>"$out/regtest.log"
python3 "$perf/activity.py" prepare --outputs 3000 2>>"$out/activity.log"
# 30 s at 20 a second, then a block: the grid's block, about 600 transactions.
python3 "$perf/activity.py" flow --rate 20 --seconds 30 2>>"$out/activity.log"
grid=$(python3 "$root/dev/regtest/regtest.py" mine 1)
# A live mempool for the home page while the pages load.
python3 "$perf/activity.py" flow --rate 5 --seconds 3600 --block-every 60 2>>"$out/activity.log" &
activity=$!
read -r addr server < <("$perf/server.sh" "$out/server")
eval "$(python3 "$root/dev/regtest/regtest.py" env | sed 's/^/export /')"
for _ in $(seq 60); do
  curl -sf "http://$addr/regtest/api/v1/fees/mempool-blocks" >/dev/null && break
  sleep 1
done
cd "$root/web"
node ../dev/perf/pages.mjs regtest "http://$addr" --manifest "$out/manifest.json" --frames \
  --grid-block "$grid" "$@" >"$out/pages.json"
echo "perf: done; dev/perf/summary.py --pages $out/pages.json" >&2
