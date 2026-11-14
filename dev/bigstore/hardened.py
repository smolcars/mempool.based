#!/usr/bin/env python3
"""Runs the server image as deploy/compose.yml hardens it against a mainnet-sized store.

    python3 -I dev/bigstore/hardened.py <image> [--work DIR]

Read-only root, no capabilities, no writable /tmp: only the store's volume is writable. The store
is bigstore.py's. A stand-in node answers `getblockchaininfo` for mainnet and nothing else, so
the store's gate opens and every background task that writes runs, the pool attribution among
them, rewriting all 975,000 block rows. Then every route that answers from the store alone must
answer 200, and no store read or write may have failed. Exit 0 when all held. Docker and Python
3.11 or later; nothing reaches the internet, and the container runs on the host's network so
the stand-in and the server each take a free port.
"""

import argparse
import http.server
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request

HERE = pathlib.Path(__file__).resolve().parent
# The pool attribution rewrites every row first; a runner takes a few times a laptop's 12 s.
READY_WITHIN = 300
FAILURES = ("store_read_failed", "store_write_failed", "store_read_timed_out", "mining_failed",
            "mempool_history_failed", "lightning_index_failed")


def node(tip):
    """A JSON-RPC stand-in on a free loopback port: mainnet at `tip`. Returns the port."""
    info = {"chain": "main", "blocks": tip, "headers": tip, "bestblockhash": "00" * 32,
            "bits": "17023a04", "target": "0" * 64, "difficulty": 1.0, "time": 0,
            "mediantime": 0, "verificationprogress": 1.0, "initialblockdownload": False,
            "chainwork": "0" * 64, "size_on_disk": 0, "pruned": False}

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_POST(self):
            req = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            if req.get("method") == "getblockchaininfo":
                out = {"result": info, "error": None, "id": req.get("id")}
            else:
                out = {"result": None, "id": req.get("id"),
                       "error": {"code": -32601, "message": "Method not found"}}
            body = json.dumps(out).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server.server_address[1]


def lines(name):
    out = subprocess.run(["docker", "logs", name], capture_output=True, text=True).stdout
    return [json.loads(line) for line in out.splitlines() if line.startswith("{")]


def get(url):
    try:
        with urllib.request.urlopen(url, timeout=30) as r:
            return r.status, r.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()


def paths(db):
    """Every route that answers from the store alone, at its largest: GET paths."""
    import sqlite3

    c = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    one = lambda sql: c.execute(sql).fetchone()[0]
    tip = one("SELECT max(height) FROM blocks")
    audited = one("SELECT hash FROM audits ORDER BY height DESC LIMIT 1")[::-1].hex()
    pool = "unknown"
    out = [
        f"/app/block/{audited}/summary", f"/app/block/{audited}/audit",
        f"/api/v1/block/{audited}/audit-summary", f"/api/v1/mining/blocks/audit/score/{audited}",
        "/api/v1/mining/blocks/audit/scores", f"/api/v1/mining/blocks/audit/scores/{tip}",
        "/api/v1/mining/pools", "/api/v1/mining/hashrate/pools",
        "/api/v1/mining/difficulty-adjustments", "/api/v1/mining/reward-stats/144",
        "/api/v1/mining/reward-stats/10000", "/api/v1/mining/blocks/timestamp/1600000000",
        f"/api/v1/mining/pool/{pool}", f"/api/v1/mining/pool/{pool}/blocks",
        f"/api/v1/mining/pool/{pool}/hashrate",
        "/app/lightning/status", "/app/lightning/channels/opened", "/app/lightning/channels/closed",
        "/app/lightning/statistics/latest", "/api/v1/lightning/statistics/latest",
    ]
    for i in ("24h", "3d", "1w", "1m", "3m", "6m", "1y", "2y", "3y", "all"):
        out += [f"/api/v1/mining/pools/{i}", f"/api/v1/mining/hashrate/{i}",
                f"/api/v1/mining/hashrate/pools/{i}", f"/api/v1/mining/blocks/fees/{i}",
                f"/api/v1/mining/blocks/rewards/{i}", f"/api/v1/mining/blocks/fee-rates/{i}",
                f"/api/v1/mining/blocks/sizes-weights/{i}",
                f"/api/v1/mining/blocks/predictions/{i}",
                f"/api/v1/mining/difficulty-adjustments/{i}"]
    for i in ("24h", "1w", "1m", "3m", "6m", "1y", "2y", "3y", "all"):
        out += [f"/app/mempool/history/{i}", f"/api/v1/statistics/{i}",
                f"/app/lightning/statistics/{i}", f"/api/v1/lightning/statistics/{i}"]
    return tip, out


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    p.add_argument("image")
    p.add_argument("--work", type=pathlib.Path, help="the directory to make a temporary one in")
    a = p.parse_args()
    work = pathlib.Path(tempfile.mkdtemp(prefix="bigstore-", dir=a.work))
    name = f"bigstore-{os.getpid()}"
    try:
        data = work / "data"
        data.mkdir()
        db = data / "mainnet.sqlite"
        started = time.monotonic()
        subprocess.run([sys.executable, "-I", str(HERE / "bigstore.py"), str(db)], check=True)
        print(f"store: {db.stat().st_size >> 20} MiB in {time.monotonic() - started:.0f} s")
        # The image's user (65532) writes the store and its WAL in the bind mount.
        data.chmod(0o777)
        db.chmod(0o666)
        tip, checked = paths(db)
        env = {
            "DATA_DIR": "/data", "LISTEN_ADDR": "127.0.0.1:0",
            "MAINNET_RPC_HOST": "127.0.0.1", "MAINNET_RPC_PORT": str(node(tip)),
            "MAINNET_RPC_USER": "u", "MAINNET_RPC_PASSWORD": "p",
            # Nothing listens on port 1: the node's other interfaces and the Lightning peer (the
            # generator point as its key) stay unreachable.
            "MAINNET_ESPLORA_URL": "http://127.0.0.1:1/", "MAINNET_ELECTRUM_ADDR": "127.0.0.1:1",
            "MAINNET_MEMPOOL_ENABLED": "false", "MAINNET_LIGHTNING_ENABLED": "true",
            "MAINNET_LIGHTNING_PEERS":
                "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798@127.0.0.1:1",
            "CLIENT_BURST": "100000", "CLIENT_RATE_PER_SEC": "100000",
        }
        cmd = ["docker", "run", "-d", "--name", name, "--network", "host", "--read-only",
               "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
               "-v", f"{data}:/data"]
        for k, v in env.items():
            cmd += ["-e", f"{k}={v}"]
        subprocess.run(cmd + [a.image], check=True, stdout=subprocess.DEVNULL)
        started = time.monotonic()
        addr = None
        while True:
            seen = lines(name)
            failed = [e for e in seen if e.get("event") in FAILURES]
            if failed:
                sys.exit(f"store failed: {failed[0]}")
            addr = addr or next((e["addr"] for e in seen if e.get("event") == "listening"), None)
            if any(e.get("event") == "pools_reattributed" for e in seen):
                break
            if time.monotonic() - started > READY_WITHIN:
                sys.exit(f"no pools_reattributed within {READY_WITHIN} s")
            time.sleep(1)
        print(f"pools attributed again in {time.monotonic() - started:.0f} s")
        base = f"http://{addr}"
        # The mining answers are computed after the attribution: wait for the first one.
        while get(base + "/api/v1/mining/pools/all")[0] == 503:
            if time.monotonic() - started > READY_WITHIN:
                sys.exit("mining answers never ready")
            time.sleep(1)
        bad = []
        for path in checked:
            status, body = get(base + path)
            if status != 200:
                bad.append(f"{status} {path} {body[:100]!r}")
        failed = [e for e in lines(name) if e.get("event") in FAILURES]
        for b in bad:
            print(b)
        for f in failed:
            print(f)
        if bad or failed:
            sys.exit(f"{len(bad)} of {len(checked)} routes failed, {len(failed)} store failures")
        print(f"{len(checked)} routes answered 200; no store read or write failed")
    finally:
        subprocess.run(["docker", "rm", "-f", name], capture_output=True)
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
