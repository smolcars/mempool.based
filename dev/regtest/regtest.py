#!/usr/bin/env python3
"""Private rbitcoin v0.9.1 regtest node in Docker. Needs only docker and python3.

rbitcoin has no wallet, so every coin here sits in an anyone-can-spend script
and transactions are built and serialized by hand: no keys, no signatures.
Usage is in docs/REGTEST.md; `regtest.py -h` lists the commands.
"""

import argparse
import base64
import hashlib
import json
import os
import socket
import struct
import subprocess
import sys
import time
import urllib.error
import urllib.request
from decimal import Decimal

HERE = os.path.dirname(os.path.abspath(__file__))
COMPOSE = os.path.join(HERE, "compose.yml")
USER = PASSWORD = "regtest"  # throwaway; must match the cookie in compose.yml
PORTS = {"rpc": 18443, "esplora": 3000, "electrum": 50001}  # container side
T0 = 1767225600  # 2026-01-01T00:00:00Z. Fixed block times make the scenario's hashes repeatable.


def sha256(b):
    return hashlib.sha256(b).digest()


def tag(name):
    return sha256(b"mempool.based regtest " + name.encode())


# Scripts. Coins we spend later are anyone-can-spend; the rest pay to made-up
# keys and hashes that nobody holds, which is fine for display.
OP_TRUE = b"\x51"
MINER = b"\x00\x20" + sha256(OP_TRUE)  # P2WSH(OP_TRUE): spent with witness [OP_TRUE]
P2SH_TRUE = bytes.fromhex("a914da1745e9b549bd0bfa1a569971c77eba30cd5a4b87")  # P2SH(OP_TRUE), hash160(0x51)
P2A = bytes.fromhex("51024e73")  # pay-to-anchor: spent with nothing at all
P2PKH = b"\x76\xa9\x14" + tag("p2pkh")[:20] + b"\x88\xac"
P2WPKH = b"\x00\x14" + tag("p2wpkh")[:20]
P2WSH = b"\x00\x20" + tag("p2wsh")
P2TR = b"\x51\x20" + tag("p2tr")
NOTE = b"mempool.based regtest"
OP_RETURN = b"\x6a" + bytes([len(NOTE)]) + NOTE
UNLOCK = {MINER: (b"", [OP_TRUE]), P2SH_TRUE: (b"\x01" + OP_TRUE, []), P2A: (b"", [])}


# --- docker -----------------------------------------------------------------


def project():
    # One instance per checkout by default, so builders in separate worktrees never collide.
    return os.environ.get("REGTEST_PROJECT") or "regtest-" + sha256(HERE.encode()).hex()[:8]


def compose(*args, capture=False):
    env = dict(os.environ, REGTEST_PROJECT=project())
    cmd = ["docker", "compose", "-f", COMPOSE, "-p", project(), *args]
    out = subprocess.PIPE if capture else None
    r = subprocess.run(cmd, env=env, text=True, stdout=out, stderr=out)
    if r.returncode:
        sys.exit(f"regtest: docker compose {args[0]} failed for {project()} (exit {r.returncode})")
    return r.stdout


_ports = {}


def port(name):
    if name not in _ports:
        if not compose("ps", "-q", "--status", "running", capture=True).strip():
            sys.exit(f"regtest: {project()} is not running; start it with `regtest.py up`")
        _ports[name] = int(compose("port", "node", str(PORTS[name]), capture=True).strip().rsplit(":", 1)[1])
    return _ports[name]


# --- node clients -----------------------------------------------------------


class RpcError(Exception):
    pass


def rpc(method, *params):
    req = urllib.request.Request(
        f"http://127.0.0.1:{port('rpc')}/",
        data=json.dumps({"jsonrpc": "1.0", "id": 1, "method": method, "params": list(params)}).encode(),
        headers={
            "Authorization": "Basic " + base64.b64encode(f"{USER}:{PASSWORD}".encode()).decode(),
            "Content-Type": "application/json",
        },
    )
    try:
        body = urllib.request.urlopen(req, timeout=60).read()
    except urllib.error.HTTPError as e:
        body = e.read()  # JSON-RPC errors come back with HTTP 500
    r = json.loads(body, parse_float=Decimal)
    if r.get("error"):
        raise RpcError(f"{method}: {r['error']}")
    return r["result"]


def esplora(path):
    with urllib.request.urlopen(f"http://127.0.0.1:{port('esplora')}{path}", timeout=30) as r:
        return json.loads(r.read())


def electrum(method, *params):
    with socket.create_connection(("127.0.0.1", port("electrum")), timeout=30) as s:
        s.sendall((json.dumps({"id": 1, "method": method, "params": list(params)}) + "\n").encode())
        r = json.loads(s.makefile().readline())
    if r.get("error"):
        raise RpcError(f"{method}: {r['error']}")
    return r["result"]


def wait_ready(timeout=120):
    deadline = time.monotonic() + timeout
    while True:
        try:
            height = rpc("getblockcount")
            if esplora("/blocks/tip/height") == height and electrum("server.version", "regtest.py", "1.4"):
                return
        except (OSError, ValueError, RpcError, urllib.error.URLError):
            pass
        if time.monotonic() > deadline:
            compose("logs", "--tail", "40")
            sys.exit(f"regtest: not ready after {timeout} s")
        time.sleep(0.2)


# --- transactions -----------------------------------------------------------


def varint(n):
    if n < 0xFD:
        return bytes([n])
    return b"\xfd" + struct.pack("<H", n) if n <= 0xFFFF else b"\xfe" + struct.pack("<I", n)


def serialize(ins, outs, with_witness=True):
    """ins: (txid, vout, spk, sequence); outs: (spk, sats)."""
    unlock = [UNLOCK[spk] for _, _, spk, _ in ins]
    segwit = with_witness and any(w for _, w in unlock)
    b = struct.pack("<i", 2) + (b"\x00\x01" if segwit else b"") + varint(len(ins))
    for (txid, vout, _, seq), (script_sig, _) in zip(ins, unlock):
        b += bytes.fromhex(txid)[::-1] + struct.pack("<I", vout) + varint(len(script_sig)) + script_sig
        b += struct.pack("<I", seq)
    b += varint(len(outs))
    for spk, sats in outs:
        b += struct.pack("<q", sats) + varint(len(spk)) + spk
    if segwit:
        for _, wit in unlock:
            b += varint(len(wit)) + b"".join(varint(len(item)) + item for item in wit)
    return b + struct.pack("<I", 0)


def build(coins, outs, feerate, seq=0xFFFFFFFD):
    """coins: (txid, vout, spk, sats). Change goes back to MINER. Returns (txid, hex)."""
    ins = [(txid, vout, spk, seq) for txid, vout, spk, _ in coins]
    change = sum(c[3] for c in coins) - sum(sats for _, sats in outs)
    trial = outs + [(MINER, change)]
    base, full = len(serialize(ins, trial, False)), len(serialize(ins, trial))
    vsize = (base * 3 + full + 3) // 4
    change -= -(-int(vsize * feerate * 1000) // 1000)  # fee, rounded up
    if change < 0:
        sys.exit("regtest: inputs do not cover outputs and fee")
    final = outs + ([(MINER, change)] if change else [])
    return sha256(sha256(serialize(ins, final, False)))[::-1].hex(), serialize(ins, final).hex()


def broadcast(coins, outs, feerate, **kw):
    txid, raw = build(coins, outs, feerate, **kw)
    if rpc("sendrawtransaction", raw) != txid:
        sys.exit(f"regtest: node returned a different txid for {txid}")
    return txid


def take_coins(n=1):
    """The n oldest mature coinbases not yet spent (mempool included)."""
    coins, tip = [], rpc("getblockcount")
    for h in range(1, tip - 98):  # a coinbase needs 100 confirmations
        txid = rpc("getblock", rpc("getblockhash", h), 1)["tx"][0]
        out = rpc("gettxout", txid, 0, True)
        if out and out["scriptPubKey"]["hex"] == MINER.hex():
            coins.append((txid, 0, MINER, int(out["value"] * 100_000_000)))
            if len(coins) == n:
                return coins
    sys.exit("regtest: not enough mature coins; mine more blocks (a coinbase matures after 100)")


def mine(n):
    return rpc("generatetodescriptor", n, f"raw({MINER.hex()})")


def mine_at_fixed_times(n):
    hashes = []
    for _ in range(n):
        rpc("setmocktime", T0 + 600 * (rpc("getblockcount") + 1))
        hashes += mine(1)
    return hashes


def rbf():
    [coin] = take_coins()
    outs = [(P2TR, 25_000_000)]
    original = broadcast([coin], outs, 1)
    replacement = broadcast([coin], outs, 5)
    return {"rbf_original": original, "rbf_replacement": replacement}


def cpfp():
    [coin] = take_coins()
    parent = broadcast([coin], [(MINER, 100_000_000)], 1)
    child = broadcast([(parent, 0, MINER, 100_000_000)], [(P2WPKH, 99_000_000)], 25)
    return {"cpfp_parent": parent, "cpfp_child": child}


# --- commands ---------------------------------------------------------------


def print_env():
    print(
        f"REGTEST_RPC_HOST=127.0.0.1\nREGTEST_RPC_PORT={port('rpc')}\n"
        f"REGTEST_RPC_USER={USER}\nREGTEST_RPC_PASSWORD={PASSWORD}\n"
        f"REGTEST_ESPLORA_URL=http://127.0.0.1:{port('esplora')}\n"
        f"REGTEST_ELECTRUM_ADDR=127.0.0.1:{port('electrum')}"
    )


def cmd_up(_):
    start = time.monotonic()
    compose("up", "-d", "--quiet-pull")
    wait_ready()
    print(f"regtest: {project()} ready in {time.monotonic() - start:.1f} s", file=sys.stderr)
    print_env()


def cmd_reset(args):
    compose("down", "-v", "--remove-orphans")
    cmd_up(args)


def cmd_send(args):
    info = rpc("validateaddress", args.address)
    if not info.get("isvalid"):
        sys.exit(f"regtest: not a regtest address: {args.address}")
    spk = bytes.fromhex(info["scriptPubKey"])
    print(broadcast(take_coins(), [(spk, args.sats)], args.feerate))


def cmd_scenario(args):
    cmd_reset(args)
    blocks = mine_at_fixed_times(105)  # coinbases 1..6 are now mature
    cb1, cb2, cb3 = take_coins(3)
    kinds = [(P2PKH, 100_000_000), (P2SH_TRUE, 50_000_000), (P2WPKH, 100_000_000), (P2WSH, 100_000_000),
             (P2TR, 100_000_000), (P2A, 10_000), (OP_RETURN, 0)]
    txs = {"script_types": broadcast([cb1], kinds, 2)}
    txs["consolidation"] = broadcast([cb2, cb3], [(P2WPKH, 9_000_000_000)], 3)
    blocks += mine_at_fixed_times(1)  # 106
    st = txs["script_types"]
    txs["spend_p2sh_p2a"] = broadcast([(st, 1, P2SH_TRUE, 50_000_000), (st, 5, P2A, 10_000)], [(P2TR, 49_000_000)], 4)
    blocks += mine_at_fixed_times(4)  # 107..110

    # Back to the wall clock: mempool expiry counts from acceptance time.
    rpc("setmocktime", 0)
    txs["payment"] = broadcast(take_coins(), [(P2WPKH, 50_000_000)], 2)
    txs.update(rbf())
    txs.update(cpfp())

    # Check what the scenario promises, including the address index.
    pool = set(rpc("getrawmempool"))
    want = {txs[k] for k in ("payment", "rbf_replacement", "cpfp_parent", "cpfp_child")}
    assert pool == want, f"mempool {sorted(pool)} != {sorted(want)}"
    addr = esplora(f"/tx/{st}")["vout"][2]["scriptpubkey_address"]  # the P2WPKH output
    history = {t["txid"] for t in esplora(f"/address/{addr}/txs")}
    assert {st, txs["consolidation"], txs["payment"], txs["cpfp_child"]} <= history, history
    scripthash = sha256(P2WPKH)[::-1].hex()
    assert st in {h["tx_hash"] for h in electrum("blockchain.scripthash.get_history", scripthash)}

    vout = esplora(f"/tx/{st}")["vout"]
    names = ["p2pkh", "p2sh", "p2wpkh", "p2wsh", "p2tr", "p2a", None, "miner"]
    addresses = {name: out["scriptpubkey_address"] for name, out in zip(names, vout) if name}
    print(json.dumps({"tip": {"height": len(blocks), "hash": blocks[-1]}, "blocks": {"1": blocks[0], "106": blocks[105]},
                      "txs": txs, "addresses": addresses}, indent=2))


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    sub.add_parser("up", help="start (or keep) the node, wait until ready, print env").set_defaults(f=cmd_up)
    sub.add_parser("stop", help="stop the node, keep the chain").set_defaults(f=lambda _: compose("stop"))
    sub.add_parser("down", help="remove the node and its chain").set_defaults(
        f=lambda _: compose("down", "-v", "--remove-orphans"))
    sub.add_parser("reset", help="down, then up on an empty chain").set_defaults(f=cmd_reset)
    sub.add_parser("env", help="print .env lines for the running node").set_defaults(f=lambda _: print_env())
    m = sub.add_parser("mine", help="mine N blocks to the anyone-can-spend miner script")
    m.add_argument("n", type=int)
    m.set_defaults(f=lambda a: print("\n".join(mine(a.n))))
    s = sub.add_parser("send", help="pay SATS to ADDRESS from a mature coinbase, print txid")
    s.add_argument("address")
    s.add_argument("sats", type=int)
    s.add_argument("--feerate", type=float, default=2.0, help="sat/vB (default 2)")
    s.set_defaults(f=cmd_send)
    sub.add_parser("rbf", help="broadcast a tx and its replacement, print both txids").set_defaults(
        f=lambda _: print(json.dumps(rbf(), indent=2)))
    sub.add_parser("cpfp", help="broadcast a low-fee parent and a high-fee child").set_defaults(
        f=lambda _: print(json.dumps(cpfp(), indent=2)))
    sub.add_parser("scenario", help="reset, build the known chain, print its manifest").set_defaults(f=cmd_scenario)
    args = p.parse_args()
    try:
        args.f(args)
    except RpcError as e:
        sys.exit(f"regtest: rpc {e}")


if __name__ == "__main__":
    main()
