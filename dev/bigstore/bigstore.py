#!/usr/bin/env python3
"""Writes a mainnet-sized store for the hardened-container test (docs/STORE.md, "Temporary data").

    python3 -I dev/bigstore/bigstore.py <out.sqlite> [--blocks N] [--channels N]

Made-up rows of mainnet's shape and size: a block per height with its totals, a year of
mempool and Lightning history, a year of audits, the fee score's 30 days, the replacement
bound and 40,000 Lightning channels among 15,000 nodes. The schema is the migrations', applied
here as the server would, so the server opens the file as its own. Seeded: the same arguments
give the same rows, stamped up to now. Standard library only.
"""

import argparse
import hashlib
import pathlib
import random
import sqlite3
import sys
import time as clock

ROOT = pathlib.Path(__file__).resolve().parents[2]
MIGRATIONS = ROOT / "crates/engine/src/store/migrations"
# Mainnet's genesis hash in internal byte order, as the `chain` table holds it.
GENESIS = bytes.fromhex("000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f")[::-1]
SPACING = 573  # mainnet's mean seconds per block so far
YEAR_OF_BLOCKS = 52_560


def schema(db):
    files = sorted(MIGRATIONS.glob("[0-9][0-9][0-9][0-9]_*.sql"))
    for f in files:
        db.executescript(f.read_text())
    db.execute(f"PRAGMA user_version = {len(files)}")


def blocks(db, rng, n, start):
    # The totals triggers would queue eight buckets a row; the totals are written whole below.
    triggers = db.execute(
        "SELECT name, sql FROM sqlite_schema WHERE type = 'trigger' AND tbl_name = 'blocks'"
    ).fetchall()
    for name, _ in triggers:
        db.execute(f"DROP TRIGGER {name}")
    pools = [rng.randrange(1, 160) for _ in range(60)]
    weights = [1 / (i + 1) for i in range(len(pools))]
    rated_from = max(0, n - YEAR_OF_BLOCKS)
    rows = []
    for h in range(n):
        time = start + h * SPACING
        txs = rng.randrange(1, 4000) if h > 100_000 else rng.randrange(1, 20)
        fees = txs * rng.randrange(200, 4000)
        subsidy = (50 * 100_000_000) >> (h // 210_000)
        rated = h >= rated_from
        rates = sorted(rng.uniform(1, 300) for _ in range(7)) if rated else [None] * 7
        bands = [rng.randrange(0, 200_000) for _ in range(12)] if rated else [None] * 12
        rows.append(
            (h, hashlib.sha256(h.to_bytes(8, "little")).digest(), time, txs, txs * 400,
             txs * 1500, txs * 1450, fees, subsidy, subsidy + fees, 1.0 + h * 1e8,
             rng.randbytes(40), b"\x16" + rng.randbytes(22), int(rated), *rates, *bands,
             rng.choices(pools, weights)[0] if h > 200_000 else 0)
        )
        if len(rows) == 50_000:
            insert_blocks(db, rows)
            rows = []
    insert_blocks(db, rows)
    for _, sql in triggers:
        db.execute(sql)
    # Every bucket the server's settle keeps, by the statement it keeps them with.
    periods = (
        "SELECT CASE WHEN ?1 = 0 THEN time / 86400 ELSE height / ?1 END AS k, count(*), "
        "min(height), max(height), min(time), max(time), sum(tx_count), sum(size), "
        "sum(weight), sum(fees), sum(subsidy), sum(reward), avg(difficulty), sum(rated), "
        "avg(fee_min), avg(fee_p10), avg(fee_p25), avg(fee_median), avg(fee_p75), "
        "avg(fee_p90), avg(fee_max) FROM blocks GROUP BY k"
    )
    for kind in (0, 18, 36, 72, 144, 216, 1008):
        db.execute(f"INSERT INTO block_totals SELECT ?1, * FROM ({periods})", (kind,))
    db.execute(
        "INSERT INTO pool_totals SELECT 1008, height / 1008, pool, count(*), sum(tx_count = 1), "
        "sum(reward) FROM blocks GROUP BY 2, 3"
    )
    db.execute(
        "INSERT INTO pool_totals SELECT 7, (time - 345600) / 604800, pool, count(*), "
        "sum(tx_count = 1), sum(reward) FROM blocks GROUP BY 2, 3"
    )


def insert_blocks(db, rows):
    db.executemany(
        "INSERT INTO blocks (height, hash, time, tx_count, size, weight, paying_weight, fees, "
        "subsidy, reward, difficulty, coinbase_tag, payouts, rated, fee_min, fee_p10, fee_p25, "
        "fee_median, fee_p75, fee_p90, fee_max, band0, band1, band2, band3, band4, band5, band6, "
        "band7, band8, band9, band0_12_15, band0_15_20, pool) VALUES "
        "(" + ", ".join("?" * 34) + ")",
        rows,
    )


def audit_txs(rng):
    """Up to 40 records as the audit stores them: txid, kind 1 to 5, vsize, rate, first seen."""
    return b"".join(
        rng.randbytes(32) + bytes([rng.randrange(1, 6)])
        + rng.randrange(100, 2000).to_bytes(4, "little")
        + rng.randrange(1000, 90000).to_bytes(4, "little") + bytes(4)
        for _ in range(rng.randrange(0, 40))
    )


def audits(db, rng, n, start):
    rows = []
    for h in range(max(0, n - YEAR_OF_BLOCKS), n):
        counts = [rng.randrange(0, 4000) for _ in range(22)]
        rows.append(
            (hashlib.sha256(h.to_bytes(8, "little")).digest(), h, start + h * SPACING,
             start + h * SPACING + 5, rng.uniform(80, 100), *counts[:3], counts[3] * 100,
             *counts[4:7], counts[7] * 100, *counts[8:22], audit_txs(rng))
        )
    db.executemany("INSERT INTO audits VALUES (" + ", ".join("?" * 28) + ")", rows)


def history(db, rng, end):
    """mempool_history and ln_statistics at their bounds: minutes (ten minutes) for 8 days,
    hours to 400 days."""
    minute_rows, ln_rows = [], []
    for level, step, span in ((0, 60, 8 * 86400), (1, 3600, 392 * 86400)):
        top = end - (0 if level == 0 else 8 * 86400)
        for t in range(top - span, top, step):
            k = 1 if level == 0 else 60
            minute_rows.append(
                (level, t - t % step, k, *(rng.randrange(1, 10**6) * k for _ in range(14)),
                 *(rng.randrange(0, 10**5) * k for _ in range(6)), None, None)
            )
            if level == 1 or t % 600 == 0:
                s = 1 if level == 0 else 6
                ln_rows.append(
                    (level, t - t % (600 if level == 0 else 3600), s,
                     *(rng.randrange(1, 10**6) * s for _ in range(13)))
                )
    db.executemany(
        "INSERT OR IGNORE INTO mempool_history VALUES (" + ", ".join("?" * 25) + ")", minute_rows
    )
    db.executemany(
        "INSERT OR IGNORE INTO ln_statistics VALUES (" + ", ".join("?" * 16) + ")", ln_rows
    )


def fees(db, rng, end, tip):
    db.executemany(
        "INSERT INTO fee_samples VALUES (?, 0, ?, ?, ?, ?)",
        [(t, *sorted((rng.randrange(1000, 90000) for _ in range(4)), reverse=True))
         for t in range(end - 30 * 86400, end, 30)],
    )
    db.executemany(
        "INSERT INTO fee_blocks VALUES (?, ?, ?)",
        [(h, end - (tip - h) * 600, rng.randrange(1000, 50000)) for h in range(tip - 4464, tip)],
    )


def replacements(db, rng, end):
    db.executemany(
        "INSERT INTO replacements (txid, by, seen, fee, vsize, weight, value, time, rbf, by_fee, "
        "by_vsize, by_weight, by_value, by_time, by_rbf, mined) "
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?, ?, ?, 1, ?)",
        [(rng.randbytes(32), rng.randbytes(32), end - i * 60, 500, 200, 800, 10**6, end - i * 60,
          900, 200, 800, 10**6, end - i * 60, i % 2) for i in range(10_000)],
    )


def channels(db, rng, n, tip, start):
    nodes = [b"\x02" + rng.randbytes(32) for _ in range(15_000)]
    pick = lambda: nodes[min(int(rng.paretovariate(0.9)) - 1, len(nodes) - 1) if rng.random() < 0.5
                         else rng.randrange(len(nodes))]
    rows, seen = [], set()
    while len(rows) < n:
        height = rng.randrange(505_000, tip - 6)
        scid = height << 40 | rng.randrange(1, 4000) << 16 | rng.randrange(0, 4)
        a, b = pick(), pick()
        if scid in seen or a == b:
            continue
        seen.add(scid)
        closed = rng.random() < 0.15
        ch = min(tip, height + rng.randrange(6, 100_000))
        rows.append(
            (scid, a, b, rng.randrange(20_000, 10**8),
             hashlib.sha256(height.to_bytes(8, "little")).digest(), start + height * SPACING,
             ch if closed else None, rng.randbytes(32) if closed else None,
             start + ch * SPACING if closed else None, rng.randbytes(32) if closed else None,
             rng.randrange(0, 4) if closed else None, 300 if closed else None,
             rng.randbytes(32) if len(rows) % 20 else None)
        )
    db.executemany(
        "INSERT INTO ln_channels (scid, node_one, node_two, capacity, open_hash, open_time, "
        "close_height, close_hash, close_time, close_txid, close_kind, close_fee, open_txid) "
        "VALUES (" + ", ".join("?" * 13) + ")",
        rows,
    )


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    p.add_argument("out", type=pathlib.Path)
    p.add_argument("--blocks", type=int, default=975_000)
    p.add_argument("--channels", type=int, default=40_000)
    a = p.parse_args()
    if a.out.exists():
        sys.exit(f"{a.out} exists")
    rng = random.Random(12)
    db = sqlite3.connect(a.out, isolation_level=None)
    # Written in one go: nothing to recover if it fails, so no journal until the end.
    db.execute("PRAGMA journal_mode = off")
    db.execute("PRAGMA synchronous = off")
    db.execute("PRAGMA cache_size = -262144")
    schema(db)  # before BEGIN: executescript commits whatever is open
    db.execute("BEGIN")
    db.execute("INSERT INTO chain (id, genesis) VALUES (0, ?)", (GENESIS,))
    # The tip is ten minutes old, so every series that ends now has its points.
    end = int(clock.time())
    start = end - 600 - a.blocks * SPACING
    blocks(db, rng, a.blocks, start)
    audits(db, rng, a.blocks, start)
    history(db, rng, end - end % 600)
    fees(db, rng, end, a.blocks)
    replacements(db, rng, end)
    channels(db, rng, a.channels, a.blocks, start)
    db.execute("COMMIT")
    db.execute("PRAGMA journal_mode = wal")
    db.close()


if __name__ == "__main__":
    main()
