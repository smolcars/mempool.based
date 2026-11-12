import { describe, expect, it } from "vitest";
import { feeBand } from "../../lib/format";
import { type GridTx, gridSize, hitTest, type Layout, layout, MIN_SIDE, rank } from "./layout";
import { COINBASE_COLOR, colorOf } from "./scene";

/** A repeatable pseudo-random source (mulberry32). */
function random(seed: number) {
  let a = seed;
  return () => {
    a = Math.trunc(a + 0x6d2b79f5) % 2 ** 32;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t ^= t + Math.imul(t ^ (t >>> 7), 61 | t);
    return ((t ^ (t >>> 14)) >>> 0) / 4_294_967_296;
  };
}

/** A block like today's: mostly 110–300 vB at under 1 sat/vB, some large ones, `total` vB in all. */
function block(total: number, seed = 1): GridTx[] {
  const rnd = random(seed);
  const txs: GridTx[] = [{ id: 0, vsize: 180, fee: 0, value: 313_000_000, coinbase: true }];
  let used = 180;
  while (used < total) {
    const big = rnd() < 0.02;
    const vsize = Math.min(
      total - used,
      Math.round(big ? 1000 + rnd() * 20_000 : 60 + rnd() * 300),
    );
    if (vsize <= 0) break;
    const rate = rnd() < 0.6 ? 0.1 + rnd() * 0.9 : rnd() * 30;
    txs.push({ id: txs.length, vsize, fee: Math.round(rate * vsize), value: 10_000 });
    used += vsize;
  }
  return txs;
}

const FULL = block(999_000);

/** Checks the layout's own promises: no overlaps, all inside, the owner map agrees, sizes follow vsize. */
function check(l: Layout, txs: readonly GridTx[]) {
  const cover = new Int32Array(l.cols * l.rows).fill(-1);
  const faults: string[] = [];
  for (let r = 0; r < txs.length; r++) {
    const [x, y, s] = [l.x[r] ?? -1, l.y[r] ?? -1, l.side[r] ?? 0];
    if (s < MIN_SIDE || x < 0 || y < 0 || x + s > l.cols || y + s > l.rows) {
      faults.push(`square ${r} at ${x},${y} side ${s} is outside`);
      continue;
    }
    for (let dy = 0; dy < s; dy++) {
      for (let dx = 0; dx < s; dx++) {
        const cell = (y + dy) * l.cols + x + dx;
        if (cover[cell] !== -1) faults.push(`square ${r} overlaps square ${cover[cell]}`);
        cover[cell] = r;
      }
    }
    // Area follows virtual size: the side is the exact side rounded, so within half a cell.
    const v = txs[l.order[r] ?? 0]?.vsize ?? 0;
    const exact = Math.sqrt(v / l.unit);
    if (exact >= MIN_SIDE && Math.abs(s - exact) > 0.5)
      faults.push(`square ${r} side ${s} for ${exact}`);
  }
  // The hit map is exactly the squares: every cell answers with the square covering it, or none.
  for (let cell = 0; cell < cover.length; cell++) {
    const cx = cell % l.cols;
    const cy = (cell - cx) / l.cols;
    if (hitTest(l, cx + 0.5, cy + 0.5) !== cover[cell]) faults.push(`cell ${cx},${cy} hits wrong`);
  }
  expect(faults.slice(0, 5)).toStrictEqual([]);
}

describe("layout", () => {
  it("packs a full block without overlaps, inside the frame, sized by vsize", () => {
    const l = layout(FULL, 6 / 5, 1_000_000);
    check(l, FULL);
    // A full block reaches the top of the frame, within the rounding of its sides.
    expect(l.offset).toBeLessThan(l.rows * 0.03);
    // Over the whole block, drawn area is the virtual size to within 3%.
    let area = 0;
    for (const s of l.side) area += s * s;
    const total = FULL.reduce((sum, t) => sum + t.vsize, 0);
    expect(Math.abs((area * l.unit) / total - 1)).toBeLessThan(0.03);
  });

  it("is fast enough to run on the main thread", () => {
    const start = performance.now();
    layout(FULL, 6 / 5, 1_000_000);
    // About 3 ms in a browser; generous here because test runners share the machine.
    expect(performance.now() - start).toBeLessThan(200);
  });

  it("draws the same picture for the same block, in any input order", () => {
    const a = layout(FULL, 6 / 5, 1_000_000);
    const b = layout(FULL, 6 / 5, 1_000_000);
    expect(b).toStrictEqual(a);
    // oxlint-disable-next-line unicorn/no-array-reverse -- reverses a copy; toReversed is newer than the browsers we build for
    const shuffled = [...FULL].reverse();
    const c = layout(shuffled, 6 / 5, 1_000_000);
    const place = (l: Layout, txs: readonly GridTx[]) => {
      const at = new Map<GridTx["id"], string>();
      for (let r = 0; r < txs.length; r++) {
        at.set(txs[l.order[r] ?? 0]?.id ?? -1, `${l.x[r]},${l.y[r]},${l.side[r]}`);
      }
      return at;
    };
    expect(place(c, shuffled)).toStrictEqual(place(a, FULL));
  });

  it("starts with the coinbase, then the highest fee rate, at the top left", () => {
    const l = layout(FULL, 6 / 5, 1_000_000);
    const rates = [...l.order].map((i) => {
      const t = FULL[i];
      return t ? t.fee / t.vsize : 0;
    });
    expect(FULL[l.order[0] ?? -1]?.coinbase).toBe(true);
    for (let r = 2; r < rates.length; r++) {
      expect(rates[r]).toBeLessThanOrEqual(rates[r - 1] ?? Infinity);
    }
    // The coinbase's square is at the left, near the top: settling may sink it into a hole below.
    expect(l.x[0]).toBeLessThan(l.cols * 0.05);
    expect((l.y[0] ?? 0) - l.offset).toBeLessThan(l.rows * 0.1);
  });

  it("rests every square on the floor or on another square", () => {
    const l = layout(block(300_000, 7), 6 / 5, 1_000_000);
    const floor = l.rows;
    for (let r = 0; r < l.order.length; r++) {
      const [x, y, s] = [l.x[r] ?? 0, l.y[r] ?? 0, l.side[r] ?? 0];
      if (y + s === floor) continue;
      const below = Array.from({ length: s }, (_, dx) => hitTest(l, x + dx + 0.5, y + s + 0.5));
      expect(
        below.some((b) => b !== -1),
        `square ${r} floats`,
      ).toBe(true);
    }
  });

  it("fills a half-full block about half way, from the floor", () => {
    const l = layout(block(500_000, 3), 6 / 5, 1_000_000);
    const top = Math.min(...l.y);
    expect(1 - top / l.rows).toBeGreaterThan(0.4);
    expect(1 - top / l.rows).toBeLessThan(0.65);
  });

  it("holds at extreme frame shapes", () => {
    const txs = block(400_000, 5);
    for (const aspect of [1 / 20, 0.3, 1, 3, 20]) check(layout(txs, aspect, 1_000_000), txs);
    expect(layout(txs, 20, 1_000_000).cols).toBeGreaterThan(layout(txs, 1 / 20, 1_000_000).cols);
  });

  it("draws a block holding only its coinbase as one square on the floor", () => {
    const txs = [{ id: 0, vsize: 150, fee: 0, value: 5_000_000_000, coinbase: true }];
    const l = layout(txs, 6 / 5, 1_000_000);
    check(l, txs);
    expect(l.side[0]).toBe(5); // sqrt(150 / 6.25) = 4.9
    expect((l.y[0] ?? 0) + (l.side[0] ?? 0)).toBe(l.rows);
  });

  it("fits one enormous transaction beside thousands of tiny ones, the tiny ones at the smallest side", () => {
    const txs: GridTx[] = [{ id: 0, vsize: 600_000, fee: 600_000, value: 1 }];
    for (let i = 1; i <= 8000; i++) txs.push({ id: i, vsize: 10, fee: 20, value: 1 });
    for (const aspect of [6 / 5, 8]) {
      const l = layout(txs, aspect, 1_000_000);
      check(l, txs);
      expect(l.side[1]).toBe(MIN_SIDE);
    }
  });

  it("shrinks everything to fit when there is more than the capacity", () => {
    const txs = block(3_000_000, 9);
    const l = layout(txs, 6 / 5, 1_000_000);
    check(l, txs);
    expect(l.unit).toBeGreaterThan(3_000_000 / (l.cols * l.rows));
  });

  it("draws nothing for no transactions", () => {
    const l = layout([], 6 / 5, 1_000_000);
    expect(l.order).toHaveLength(0);
    expect(hitTest(l, 1, 1)).toBe(-1);
  });
});

describe("hitTest", () => {
  it("answers none outside the grid and above the squares", () => {
    const l = layout(block(100_000), 6 / 5, 1_000_000);
    expect(hitTest(l, -0.5, l.rows - 1)).toBe(-1);
    expect(hitTest(l, l.cols, l.rows - 1)).toBe(-1);
    expect(hitTest(l, 1, l.rows)).toBe(-1);
    expect(hitTest(l, 1, 0)).toBe(-1);
    expect(hitTest(l, Number.NaN, 1)).toBe(-1);
  });

  it("is fast on thousands of squares", () => {
    const l = layout(FULL, 6 / 5, 1_000_000);
    const start = performance.now();
    let found = 0;
    for (let i = 0; i < 100_000; i++)
      if (hitTest(l, (i * 7) % l.cols, (i * 13) % l.rows) !== -1) found++;
    expect(found).toBeGreaterThan(0);
    expect(performance.now() - start).toBeLessThan(100);
  });
});

describe("gridSize", () => {
  it("keeps the grid when the frame changes by a pixel", () => {
    expect(gridSize(600 / 500, 4000)).toStrictEqual(gridSize(601 / 500, 4000));
    expect(gridSize(320 / 266, 4000)).toStrictEqual(gridSize(600 / 500, 4000));
  });

  it("grows with very many transactions so the smallest still fit", () => {
    const { cols, rows } = gridSize(1, 100_000);
    expect(cols * rows).toBeGreaterThanOrEqual(100_000 * MIN_SIDE * MIN_SIDE);
  });
});

describe("rank and colors", () => {
  it("treats an omitted coinbase flag as false", () => {
    const txs: GridTx[] = [
      { id: 0, vsize: 100, fee: 1, value: 0 },
      { id: 1, vsize: 100, fee: 1000, value: 0, coinbase: false },
    ];
    expect([...rank(txs)]).toStrictEqual([1, 0]);
    // oxlint-disable-next-line unicorn/no-array-reverse -- reverses a copy; toReversed is newer than the browsers we build for
    expect([...rank([...txs].reverse())]).toStrictEqual([0, 1]);
  });

  it("breaks ties by id, numbers before strings", () => {
    const txs: GridTx[] = [
      { id: "b", vsize: 100, fee: 100, value: 0 },
      { id: 2, vsize: 100, fee: 100, value: 0 },
      { id: "a", vsize: 100, fee: 100, value: 0 },
    ];
    expect([...rank(txs)]).toStrictEqual([1, 2, 0]);
  });

  it("colors by the band of the rate as printed, as feeBand does", () => {
    for (const [fee, vsize] of [
      [1996, 10_000], // 0.1996 prints as 0.20: band 2
      [1994, 10_000],
      [9996, 1000], // 9.996 prints as 10.0: band 10
      [100, 1000],
      [0, 100],
    ] as const) {
      expect(colorOf({ id: 1, vsize, fee, value: 0 })).toBe(feeBand(fee / vsize) - 1);
    }
    expect(colorOf({ id: 0, vsize: 100, fee: 0, value: 0, coinbase: true })).toBe(COINBASE_COLOR);
    expect(colorOf({ id: 1, vsize: 10_000, fee: 1996, value: 0 })).toBe(1);
  });
});
