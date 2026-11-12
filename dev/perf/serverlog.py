#!/usr/bin/env python3
"""Server-side timings from the server's JSON log, for a time window.

  serverlog.py LOG [--from EPOCH_S] [--to EPOCH_S]

Prints one JSON object: for each route (the matched template, plus the Esplora endpoint for
`/api` pass-through) the requests, statuses and `duration_ms` percentiles (time until the
response head was ready, as the server measured it); the `ws_fanout` block events; and the
`mempool_summary` and `mempool_block` events. Nearest-rank percentiles.
"""

import argparse
import json
import math
from datetime import datetime


def pct(sorted_values, p):
    if not sorted_values:
        return None
    return sorted_values[min(len(sorted_values), max(1, math.ceil(p / 100 * len(sorted_values)))) - 1]


def stats(values):
    v = sorted(values)
    return {"n": len(v), "p50": pct(v, 50), "p99": pct(v, 99), "max": v[-1] if v else None}


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("log")
    p.add_argument("--from", dest="since", type=float, default=0)
    p.add_argument("--to", dest="until", type=float, default=float("inf"))
    a = p.parse_args()
    routes, statuses, fanouts, summaries, blocks = {}, {}, [], [], []
    with open(a.log) as f:
        for line in f:
            try:
                e = json.loads(line)
                t = datetime.fromisoformat(e["ts"].replace("Z", "+00:00")).timestamp()
            except (ValueError, KeyError):
                continue
            if not a.since <= t < a.until:
                continue
            kind = e.get("event")
            if kind == "http_request":
                key = e["route"] + (f" [{e['endpoint']}]" if e.get("endpoint") else "")
                ok = 200 <= e["status"] < 400
                routes.setdefault(key, {"ok": [], "refused": []})["ok" if ok else "refused"].append(e["duration_ms"])
                s = statuses.setdefault(key, {})
                s[e["status"]] = s.get(e["status"], 0) + 1
            elif kind == "ws_fanout":
                fanouts.append({k: e.get(k) for k in ("kind", "subscribers", "bytes", "duration_ms", "fanout_ms", "dropped_slow")})
            elif kind == "mempool_summary":
                summaries.append({k: e.get(k) for k in ("txs", "cycle_ms_max", "pack_ms_max", "ws_messages", "ws_sent_bytes", "ws_dropped_slow", "fanout_ms_max", "node_calls", "node_bytes")})
            elif kind == "mempool_block":
                blocks.append({k: e.get(k) for k in ("height", "tracker_ms", "sent_ms", "refresh_ms")})
    out = {
        "routes": {k: {"status": statuses[k], "ok_ms": stats(v["ok"]), "refused_ms": stats(v["refused"])}
                   for k, v in sorted(routes.items())},
        "ws_fanout": fanouts,
        "mempool_summary": summaries,
        "mempool_block": blocks,
    }
    print(json.dumps(out, indent=1))


if __name__ == "__main__":
    main()
