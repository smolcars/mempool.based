#!/usr/bin/env python3
"""Runs the Autobahn TestSuite's fuzzing client against the WebSocket hub (docs/WS.md).

Builds the server with the `autobahn` feature (the hub echoes text messages and sends no
`hello`), starts it on a free port with no node behind it, runs the suite in Docker, and prints
a summary by case group. Needs Docker and cargo. Too slow for CI; run it after touching
crates/server/src/ws/.

    dev/autobahn/autobahn.py                 # every case
    dev/autobahn/autobahn.py 1.* 5.* 7.*     # some groups
"""

import json
import os
import platform
import subprocess
import sys
import tempfile
import time
from collections import Counter

IMAGE = os.environ.get("AUTOBAHN_IMAGE", "crossbario/autobahn-testsuite@sha256:519915fb568b04c9383f70a1c405ae3ff44ab9e35835b085239c258b6fac3074")
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
# A separate target directory, so the echo server never replaces the normal build.
TARGET = os.path.join(ROOT, "target", "autobahn")
GROUPS = {
    "1": "Framing",
    "2": "Ping and pong",
    "3": "Reserved bits",
    "4": "Opcodes",
    "5": "Fragmentation",
    "6": "UTF-8",
    "7": "Close",
    "9": "Large messages",
    "10": "Auto-fragmentation",
    "12": "Compression",
    "13": "Compression",
}
STATES = ["OK", "NON-STRICT", "INFORMATIONAL", "UNIMPLEMENTED", "FAILED"]


def log(msg):
    print(msg, file=sys.stderr, flush=True)


def start_server(work):
    env = {
        "PATH": os.environ["PATH"],
        "LISTEN_ADDR": "127.0.0.1:0",
        "WEB_DIR": work,
        "DATA_DIR": work,
        "LOG_FORMAT": "json",
        "LOG_REQUESTS": "failures",
        # A network with no node behind it: the hub needs one, the echo needs nothing from it.
        "REGTEST_RPC_HOST": "127.0.0.1",
        "REGTEST_RPC_PORT": "9",
        "REGTEST_RPC_USER": "x",
        "REGTEST_RPC_PASSWORD": "x",
        "REGTEST_ESPLORA_URL": "http://127.0.0.1:9",
        "REGTEST_ELECTRUM_ADDR": "127.0.0.1:9",
        # The largest message the config allows, and no rate or queue limit in the way of the
        # suite's bursts. Binary messages stay refused, as in production.
        "WS_MAX_MESSAGE_BYTES": "1048576",
        "WS_MESSAGES_PER_SEC": "1000000",
        "WS_SEND_QUEUE": "65536",
        "CLIENT_RATE_PER_SEC": "100000",
        "CLIENT_BURST": "100000",
    }
    binary = os.path.join(TARGET, "release", "mempool-based")
    out = open(os.path.join(work, "server.log"), "w+")
    proc = subprocess.Popen([binary], env=env, stdout=out, stderr=subprocess.STDOUT)
    deadline = time.time() + 20
    while time.time() < deadline:
        out.seek(0)
        for line in out:
            try:
                event = json.loads(line)
            except ValueError:
                continue
            if event.get("event") == "listening":
                return proc, int(event["addr"].rsplit(":", 1)[1])
        if proc.poll() is not None:
            break
        time.sleep(0.1)
    proc.terminate()
    sys.exit("server did not start; see " + out.name)


def main():
    cases = sys.argv[1:] or ["*"]
    log("building the echo server")
    subprocess.run(
        ["cargo", "build", "--release", "--locked", "-p", "mempool-based",
         "--features", "autobahn", "--target-dir", TARGET],
        cwd=ROOT, check=True,
    )
    work = tempfile.mkdtemp(prefix="autobahn-")
    proc, port = start_server(work)
    # Docker on Linux shares the host's loopback with --network host; Docker Desktop and
    # OrbStack forward host.docker.internal to it.
    if platform.system() == "Linux":
        net, host = ["--network", "host"], "127.0.0.1"
    else:
        net, host = [], "host.docker.internal"
    spec = {
        "outdir": "/work/reports",
        "servers": [{"agent": "mempool.based", "url": f"ws://{host}:{port}/regtest/app/ws"}],
        "cases": cases,
        "exclude-cases": [],
        "exclude-agent-cases": {},
    }
    with open(os.path.join(work, "fuzzingclient.json"), "w") as f:
        json.dump(spec, f)
    name = f"autobahn-{os.getpid()}"
    log(f"running {', '.join(cases)} against port {port}; container {name}")
    started = time.time()
    try:
        subprocess.run(
            ["docker", "run", "--rm", "--name", name, *net, "-v", f"{work}:/work", IMAGE,
             "wstest", "-m", "fuzzingclient", "-s", "/work/fuzzingclient.json"],
            check=True, stdout=subprocess.DEVNULL,
        )
    finally:
        subprocess.run(["docker", "rm", "-f", name], capture_output=True)
        proc.terminate()
        proc.wait()
    log(f"suite took {time.time() - started:.0f} s; reports in {work}/reports")

    with open(os.path.join(work, "reports", "index.json")) as f:
        results = json.load(f)["mempool.based"]
    counts = {}
    for case, r in results.items():
        group = case.split(".")[0]
        counts.setdefault(group, Counter())[r["behavior"]] += 1
    print("| Cases | " + " | ".join(STATES) + " |")
    print("|---|" + "---|" * len(STATES))
    for group in sorted(counts, key=int):
        c = counts[group]
        row = " | ".join(str(c[s]) if c[s] else "" for s in STATES)
        print(f"| {group}.* {GROUPS.get(group, '')} | {row} |")
    total = sum(counts.values(), Counter())
    print("| **All** | " + " | ".join(str(total[s]) for s in STATES) + " |")
    print()
    for case in sorted(results, key=lambda c: [int(p) for p in c.split(".")]):
        r = results[case]
        if r["behavior"] != "OK" or r["behaviorClose"] != "OK":
            print(f"{case}: {r['behavior']} (close {r['behaviorClose']}, "
                  f"code {r.get('remoteCloseCode')})")


if __name__ == "__main__":
    main()
