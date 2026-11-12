#!/usr/bin/env python3
"""Mempool activity on the regtest node, for measuring the server under live updates.

  activity.py prepare [--outputs N]      split mature coinbases into N spendable coins, mine them
  activity.py flow --rate R --seconds S [--block-every B]
                                          broadcast R transactions a second for S seconds,
                                          mining a block every B seconds (0: never)

Uses the node `dev/regtest/regtest.py` runs (same REGTEST_PROJECT). Coins are kept in a file
beside the node's project name in the system temp directory, so `flow` can run several times.
Fee rates are spread from 1 to 60 sat/vB so the projected blocks have a fee gradient.
"""

import argparse
import json
import os
import random
import signal
import sys
import tempfile
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "regtest"))
import regtest  # noqa: E402  (the harness's own node clients and transaction builder)

PER_FANOUT = 1500  # outputs per splitting transaction, well under the standard weight limit
COIN = 1_000_000  # sats in each split coin


def coins_file():
    return os.path.join(tempfile.gettempdir(), f"rbitcoin-space-activity-{regtest.project()}.json")


def prepare(outputs):
    regtest.mine(-(-outputs // PER_FANOUT))  # one more mature coinbase per splitting transaction
    coins = []
    while len(coins) < outputs:
        [cb] = regtest.take_coins()
        n = min(PER_FANOUT, outputs - len(coins), (cb[3] - 1_000_000) // COIN)
        txid = regtest.broadcast([cb], [(regtest.MINER, COIN)] * n, 1)
        coins += [[txid, i, COIN] for i in range(n)]
    regtest.mine(1)
    with open(coins_file(), "w") as f:
        json.dump(coins, f)
    print(f"activity: {len(coins)} coins ready", file=sys.stderr)


class Stopped(BaseException):
    """SIGTERM; not an Exception, so no handler below swallows it."""


def stop(*_):
    raise Stopped


def flow(rate, seconds, block_every):
    with open(coins_file()) as f:
        coins = json.load(f)
    kinds = [regtest.P2WPKH, regtest.P2TR, regtest.P2WSH, regtest.P2PKH]
    start = time.monotonic()
    # Stopped from outside: still record which coins are spent.
    signal.signal(signal.SIGTERM, stop)
    try:
        run(coins, kinds, start, rate, seconds, block_every)
    except Stopped:
        pass
    finally:
        with open(coins_file(), "w") as f:
            json.dump(coins, f)


def run(coins, kinds, start, rate, seconds, block_every):
    next_block = start + block_every if block_every else float("inf")
    sent = blocks = 0
    while time.monotonic() - start < seconds and coins:
        due = start + sent / rate
        now = time.monotonic()
        if now < due:
            time.sleep(due - now)
        if time.monotonic() >= next_block:
            regtest.mine(1)
            blocks += 1
            print(f"activity: block at {time.time():.3f}", file=sys.stderr, flush=True)
            next_block += block_every
        txid, vout, sats = coins.pop()
        feerate = round(random.uniform(1, 60) ** 2 / 60, 1) + 1  # most cheap, a few expensive
        try:
            regtest.broadcast([(txid, vout, regtest.MINER, sats)], [(random.choice(kinds), sats // 2)], feerate)
        except (regtest.RpcError, SystemExit) as e:
            print(f"activity: broadcast failed: {e}", file=sys.stderr)
        sent += 1
    print(f"activity: {sent} transactions, {blocks} blocks, {len(coins)} coins left", file=sys.stderr)


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    pr = sub.add_parser("prepare")
    pr.add_argument("--outputs", type=int, default=6000)
    fl = sub.add_parser("flow")
    fl.add_argument("--rate", type=float, default=20)
    fl.add_argument("--seconds", type=float, default=60)
    fl.add_argument("--block-every", type=float, default=0)
    a = p.parse_args()
    if a.cmd == "prepare":
        prepare(a.outputs)
    else:
        flow(a.rate, a.seconds, a.block_every)


if __name__ == "__main__":
    main()
