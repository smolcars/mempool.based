#!/usr/bin/env python3
"""Markdown tables from `bench.sh` result directories or a `pages.mjs` result (docs/PERFORMANCE.md).

  summary.py OUT [OUT ...]          server tables; with several passes each cell is the range `a–b`
  summary.py --pages FILE.json      page-load and frame tables: median (min–max) over the runs
"""

import json
import os
import sys


def load(dirs, name):
    out = []
    for d in dirs:
        p = os.path.join(d, name)
        if os.path.exists(p) and os.path.getsize(p):
            with open(p) as f:
                out.append(json.load(f))
    return out


def rng(values, fmt="{:g}"):
    v = [x for x in values if x is not None]
    if not v:
        return "–"
    lo, hi = min(v), max(v)
    return fmt.format(lo) if lo == hi else f"{fmt.format(lo)}–{fmt.format(hi)}"


def server_ms(server_runs, key):
    """p50 and p99 of the server's own timing for the route whose name contains `key`."""
    p50, p99 = [], []
    for s in server_runs:
        for route, r in s["routes"].items():
            if key in route and r["ok_ms"]["n"]:
                p50.append(r["ok_ms"]["p50"])
                p99.append(r["ok_ms"]["p99"])
    return p50, p99


def http(dirs):
    print("| Class | Clients | Requests/s answered | Refused | Client p50 / p99 ms | Server CPU (cores) | Server RSS MB |")
    print("|---|---|---|---|---|---|---|")
    for cls in ["memory", "immutable", "store", "node"]:
        for c in [1, 100, 1000]:
            runs = load(dirs, f"http-{cls}-{c}.json")
            if not runs:
                continue
            refused = [sum(v for k, v in r["status"].items() if not k.startswith("2")) / max(1, r["requests"]) * 100 for r in runs]
            print(f"| {cls} | {c} | {rng([r['ok_per_sec'] for r in runs], '{:,.0f}')} | {rng(refused, '{:.0f}%')} "
                  f"| {rng([r['ok_ms']['p50'] for r in runs])} / {rng([r['ok_ms']['p99'] for r in runs])} "
                  f"| {rng([r['server']['cpu_pct_mean'] / 100 for r in runs], '{:.1f}')} "
                  f"| {rng([r['server']['rss_mb_max'] for r in runs])} |")
    print()
    print("Server time per route (the server's own `duration_ms`), p50 / p99 ms, at 1 / 100 / 1,000 clients:")
    print()
    print("| Route | 1 | 100 | 1,000 |")
    print("|---|---|---|---|")
    routes = {}
    for cls in ["memory", "immutable", "store", "node"]:
        for c in [1, 100, 1000]:
            for s in load(dirs, f"http-{cls}-{c}.server.json"):
                for route, r in s["routes"].items():
                    if r["ok_ms"]["n"]:
                        routes.setdefault(route, {}).setdefault(c, []).append(r["ok_ms"])
    for route, by in routes.items():
        cells = []
        for c in [1, 100, 1000]:
            v = by.get(c, [])
            cells.append(f"{rng([x['p50'] for x in v], '{:.2f}')} / {rng([x['p99'] for x in v], '{:.2f}')}" if v else "–")
        print(f"| `{route}` | " + " | ".join(cells) + " |")


def limits(dirs):
    print("| Run | Requests/s sent | Answered 2xx | 429 | 503 | Client p50 / p99 ms (2xx) | Refusal p50 / p99 ms | Server CPU (cores) |")
    print("|---|---|---|---|---|---|---|---|")
    for name in ["limits-heavy-user", "limits-victims-alone", "limits-victims-flooded", "limits-flood"]:
        runs = load(dirs, f"{name}.json")
        if not runs:
            continue
        st = lambda r, k: r["status"].get(k, 0)
        cpu = [r["server"]["cpu_pct_mean"] / 100 for r in runs if r.get("server")]
        print(f"| {name} | {rng([r['per_sec'] for r in runs], '{:,.0f}')} | {rng([r['ok_per_sec'] for r in runs], '{:,.0f}')}/s "
              f"| {rng([st(r, '429') for r in runs], '{:,}')} | {rng([st(r, '503') for r in runs], '{:,}')} "
              f"| {rng([r['ok_ms']['p50'] for r in runs])} / {rng([r['ok_ms']['p99'] for r in runs])} "
              f"| {rng([r['refused_ms']['p50'] for r in runs])} / {rng([r['refused_ms']['p99'] for r in runs])} | {rng(cpu, '{:.1f}')} |")


def incomplete(r):
    """Messages that missed a reader, or None when an older result cannot tell."""
    msgs = r["messages"].values()
    if all("incomplete" in v for v in msgs):
        return sum(v["incomplete"] for v in msgs)
    # Older results count deliveries without saying how many readers each topic had.
    return None


def ws(dirs):
    # Last-reader times cover only messages every subscribed reader got; "incomplete" counts the rest.
    print("| Sockets | Open | Server CPU mean / busiest s (cores) | Server RSS MB | Messages incomplete | Block: published → last reader p50 / max ms | Block: node → last reader max ms | mempool: first → last reader p50 / max ms | projected-txs:0 delta first → last p50 / max ms | Load tool CPU (cores) |")
    print("|---|---|---|---|---|---|---|---|---|---|")
    for n in ["100", "1000", "2000", "5000", "8000", "10000", "slow"]:
        runs = load(dirs, f"ws-{n}.json")
        if not runs:
            continue
        def m(r, label, key, q):
            # A last-reader time is shown only for a run whose every message reached every reader.
            if incomplete(r) != 0:
                return None
            x = next((v for k, v in r["messages"].items() if k == label or k.startswith(label + ":")), {}).get(key)
            return x[q] if x and x["n"] else None
        print(f"| {n} | {rng([r['open'] for r in runs], '{:,}')} "
              f"| {rng([r['server']['cpu_pct_mean'] / 100 for r in runs], '{:.2f}')} / {rng([r['server']['cpu_pct_busiest_second'] / 100 for r in runs], '{:.2f}')} "
              f"| {rng([r['server']['rss_mb_max'] for r in runs])} "
              f"| {rng([incomplete(r) for r in runs], '{:,}')} "
              f"| {rng([m(r, 'blocks:block', 'published_to_last_ms', 'p50') for r in runs])} / {rng([m(r, 'blocks:block', 'published_to_last_ms', 'max') for r in runs])} "
              f"| {rng([m(r, 'blocks:block', 'node_to_last_ms', 'max') for r in runs])} "
              f"| {rng([m(r, 'mempool:mempool', 'first_to_last_ms', 'p50') for r in runs])} / {rng([m(r, 'mempool:mempool', 'first_to_last_ms', 'max') for r in runs])} "
              f"| {rng([m(r, 'binary:2', 'first_to_last_ms', 'p50') for r in runs])} / {rng([m(r, 'binary:2', 'first_to_last_ms', 'max') for r in runs])} "
              f"| {rng([r['load_tool_cpu_pct'] / 100 for r in runs], '{:.1f}')} |")


def idle(dirs):
    for name in ["idle-quiet", "idle-activity"]:
        runs = load(dirs, f"{name}.json")
        if runs:
            print(f"- {name}: CPU {rng([r['server']['cpu_pct_mean'] for r in runs], '{:.1f}')}% of one core, "
                  f"RSS {rng([r['server']['rss_mb_max'] for r in runs])} MB")


def pages(path):
    with open(path) as f:
        d = json.load(f)
    cell = lambda v: "–" if v["median"] is None else (f"{v['median']:,}" if v["min"] == v["max"] else f"{v['median']:,} ({v['min']:,}–{v['max']:,})")
    kb = lambda v: "–" if v["median"] is None else f"{v['median'] / 1024:.0f}"
    print(f"{d['site']}, {d['runs']} cold loads each, Chromium {d['chromium']}, {d['renderer']}\n")
    for prof in ["none", "mobile"]:
        print(f"**{prof}**\n")
        print("| Page | First paint ms | First contentful paint ms | Data shown ms | Largest contentful paint ms | KB transferred (JS) | Requests | JS run ms |")
        print("|---|---|---|---|---|---|---|---|")
        for name, p in d["pages"].items():
            x = p.get(prof)
            if not x:
                continue
            print(f"| {name} | {cell(x['fp'])} | {cell(x['fcp'])} | {cell(x['data'])} | {cell(x['lcp'])} "
                  f"| {kb(x['bytes'])} ({kb(x['js_bytes'])}) | {cell(x['requests'])} | {cell(x['script_ms'])} |")
        print()
    for prof, f in (d.get("frames") or {}).items():
        for what, runs in f.items():
            print(f"- frames, {prof}, {what}: " + "; ".join(
                f"{r['fps']} fps, p99 {r['p99_ms']} ms, max {r['max_ms']} ms, {r['over_33ms']} over 33 ms, "
                f"{r['long_tasks']} long tasks, shown {r['shown']}" for r in runs))


def main():
    if sys.argv[1:2] == ["--pages"]:
        pages(sys.argv[2])
        return
    dirs = sys.argv[1:]
    for title, f in [("Idle", idle), ("HTTP", http), ("Limits", limits), ("WebSocket", ws)]:
        print(f"## {title}\n")
        f(dirs)
        print()


if __name__ == "__main__":
    main()
