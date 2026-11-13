import { describe, expect, it } from "vitest";
import {
  areaPath,
  extent,
  layerAt,
  linePath,
  maxGap,
  nearestIndex,
  runs,
  stackLayout,
} from "./geometry";
import {
  describeSeries,
  formatHashrate,
  formatShare,
  formatSi,
  linearAxis,
  logAxis,
  niceStep,
  scale,
  timeTicks,
  endLabels,
  readoutLines,
} from "./scale";

const DAY = 86_400;
const utc = (y: number, m: number, d: number, h = 0) => Date.UTC(y, m - 1, d, h) / 1000;

describe("scales", () => {
  it("maps linearly and back", () => {
    const s = scale([0, 100], [0, 200]);
    expect(s(25)).toBe(50);
    expect(s.invert(50)).toBe(25);
  });
  it("maps by the logarithm and back", () => {
    const s = scale([1, 1000], [300, 0], true);
    expect(s(10)).toBeCloseTo(200);
    expect(s.invert(100)).toBeCloseTo(100);
  });
  it("picks round steps", () => {
    expect(niceStep(0.13)).toBe(0.2);
    expect(niceStep(37)).toBe(50);
    expect(niceStep(7e20)).toBe(1e21);
  });
});

describe("value ticks", () => {
  it("covers the data with round ticks, and keeps zero when asked", () => {
    expect(linearAxis(3, 97, 4)).toStrictEqual({ domain: [0, 100], ticks: [0, 50, 100] });
    expect(linearAxis(40, 60, 4, true).ticks[0]).toBe(0);
  });
  it("never prints floating-point noise", () => {
    expect(linearAxis(0.1, 0.7, 6).ticks).toStrictEqual([0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7]);
  });
  it("reads well for hashrate in EH/s", () => {
    const { ticks } = linearAxis(9e20, 1.13e21, 5);
    expect(ticks.map((t) => formatHashrate(t))).toStrictEqual([
      "900\u202FEH/s",
      "950\u202FEH/s",
      "1\u202FZH/s",
      "1.05\u202FZH/s",
      "1.1\u202FZH/s",
      "1.15\u202FZH/s",
    ]);
  });
  it("widens a flat series so it is drawn mid-frame", () => {
    const { domain } = linearAxis(5, 5, 4);
    expect(domain[0]).toBeLessThan(5);
    expect(domain[1]).toBeGreaterThan(5);
  });
  it("uses powers of ten over many decades, with 2 and 5 over few", () => {
    expect(logAxis(3e6, 9e20).ticks).toHaveLength(14);
    expect(logAxis(0.1, 30).ticks).toStrictEqual([0.1, 0.2, 0.5, 1, 2, 5, 10, 20]);
  });
});

describe("a narrow readout", () => {
  it("takes a line for the time, each reading and all the fee bands together", () => {
    expect(readoutLines({ series: [1], notes: [1, 2], layers: [{ fee: 1 }, { fee: 2 }] })).toBe(5);
    expect(readoutLines({ series: [1], layers: [{}, {}] })).toBe(4);
    expect(readoutLines({})).toBe(1);
  });
});

describe("time ticks", () => {
  it("names the ends with their years, or months and years where there is little room", () => {
    const from = utc(2020, 11, 2, 0);
    const to = utc(2026, 11, 9, 0);
    expect(endLabels(from, to)).toStrictEqual(["2 Nov 2020", "9 Nov 2026"]);
    expect(endLabels(from, to, 136)).toStrictEqual(["Nov 2020", "Nov 2026"]);
    expect(endLabels(utc(2026, 11, 2, 0), to, 136)).toStrictEqual(["2 Nov", "9 Nov"]);
  });
  it("steps by hours over a day", () => {
    const from = utc(2026, 11, 11, 13);
    const { ticks, label } = timeTicks(from, from + DAY, 8);
    expect(ticks.length).toBeLessThanOrEqual(10);
    expect(ticks.every((t) => t % (3 * 3600) === 0)).toBe(true);
    expect(label(utc(2026, 11, 12, 6))).toBe("06:00");
    expect(label(utc(2026, 11, 12))).toBe("12 Nov");
  });
  it("steps by Mondays over a few months", () => {
    const { ticks, label } = timeTicks(utc(2026, 9, 1), utc(2026, 11, 12), 8);
    expect(ticks.every((t) => new Date(t * 1000).getUTCDay() === 1)).toBe(true);
    expect(label(utc(2026, 10, 5))).toBe("5 Oct");
  });
  it("steps by months over a year, naming the year at January", () => {
    const { ticks, label } = timeTicks(utc(2025, 11, 12), utc(2026, 11, 12), 8);
    expect(ticks.map((t) => label(t))).toContain("Jan 2026");
    expect(ticks.map((t) => label(t))).toContain("Jul");
  });
  it("steps by years over a decade", () => {
    const { ticks, label } = timeTicks(utc(2016, 6, 1), utc(2026, 11, 12), 8);
    expect(ticks.map((t) => label(t))).toStrictEqual(["2018", "2020", "2022", "2024", "2026"]);
  });
  it("steps by five years over all of Bitcoin's history", () => {
    const { ticks, label } = timeTicks(utc(2009, 1, 3), utc(2026, 11, 12), 5);
    expect(ticks.map((t) => label(t))).toStrictEqual(["2010", "2015", "2020", "2025"]);
  });
});

describe("formats", () => {
  it("prints large figures with an SI prefix", () => {
    expect(formatHashrate(9.4851e20, 4)).toBe("948.5 EH/s");
    expect(formatSi(148_360_000_000_000, "")).toBe("148 T");
    expect(formatSi(Number.NaN, "H/s")).toBe("–");
  });
  it("prints shares", () => {
    expect(formatShare(0.2553)).toBe("25.5%");
    expect(formatShare(0.0004)).toBe("under 0.1%");
    expect(formatShare(0)).toBe("0%");
  });
});

describe("gaps", () => {
  const xs = [0, 10, 20, 30, 100, 110];
  it("breaks where points are far apart or have no value", () => {
    expect(maxGap(xs)).toBe(30);
    expect(runs(xs, () => true, 30)).toStrictEqual([
      [0, 3],
      [4, 5],
    ]);
    expect(runs(xs, (i) => i !== 1, 30)).toStrictEqual([
      [0, 0],
      [2, 3],
      [4, 5],
    ]);
  });
  it("draws a line in pieces, never through a gap to zero", () => {
    const d = linePath(
      [0, 1, 2, 3],
      [5, null, 7, 8],
      (x) => x * 10,
      (y) => y,
      10,
    );
    expect(d).toBe("M0 5h0.1M20 7L30 8");
  });
  it("holds a step until the next point", () => {
    expect(
      linePath(
        [0, 1],
        [5, 7],
        (x) => x * 10,
        (y) => y,
        10,
        true,
      ),
    ).toBe("M0 5H10V7");
  });
  it("closes an area per run", () => {
    expect(
      areaPath(
        [0, 1],
        [5, 6],
        0,
        (x) => x,
        (y) => y,
        10,
      ),
    ).toBe("M0 5L1 6L1 0L0 0Z");
  });
  it("fills under a step as the step is drawn: flat, then a vertical edge at the next point", () => {
    // A node with one channel at midnight and two from an open: no slope between them.
    expect(
      areaPath(
        [0, 1, 2],
        [1, 2, 2],
        0,
        (x) => x,
        (y) => y,
        10,
        0,
        undefined,
        true,
      ),
    ).toBe("M0 1L1 1L1 2L2 2L2 2L2 0L2 0L1 0L1 0L0 0Z");
  });
  it("changes a layer on a vertical edge at a join, never across the minute before it", () => {
    // The layer arrives at 8 to 2 just before x 1, then is 6 to 2 at x 1: one vertical edge.
    const joins = new Map([[1, { in: { hi: 8, lo: 2 } }]]);
    expect(
      areaPath(
        [0, 1, 2],
        [8, 6, 6],
        [2, 2, 2],
        (x) => x,
        (y) => y,
        10,
        0,
        joins,
      ),
    ).toBe("M0 8L1 8L1 6L2 6L2 2L1 2L1 2L0 2Z");
  });
  it("leaves a point on a vertical edge at a join that changes after it", () => {
    // The layer is 8 to 2 at x 1, then leaves it at 6 to 2: the edge is at x 1, not across.
    const joins = new Map([[1, { out: { hi: 6, lo: 2 } }]]);
    expect(
      areaPath(
        [0, 1, 2],
        [8, 8, 6],
        [2, 2, 2],
        (x) => x,
        (y) => y,
        10,
        0,
        joins,
      ),
    ).toBe("M0 8L1 8L1 6L2 6L2 2L1 2L1 2L0 2Z");
  });
  it("shows a lone point in an area as a sliver when asked, and as nothing otherwise", () => {
    const area = (sliver?: number) =>
      areaPath(
        [0, 60, 300],
        [5, 6, 9],
        0,
        (x) => x,
        (y) => y,
        60,
        sliver,
      );
    expect(area(2)).toBe("M0 5L60 6L60 0L0 0ZM299 9L301 9L301 0L299 0Z");
    expect(area()).toBe("M0 5L60 6L60 0L0 0ZM300 9L300 0Z");
  });
  it("breaks at a fixed step: one missing minute is a gap, which three times the median would bridge", () => {
    const minutes = [0, 60, 180, 240];
    expect(maxGap(minutes)).toBe(180);
    expect(runs(minutes, () => true, 60)).toStrictEqual([
      [0, 1],
      [2, 3],
    ]);
  });
});

describe("stacked layout", () => {
  it("stacks layers from the bottom and leaves empty points as gaps", () => {
    const stack = stackLayout(
      [
        [0.5, 0.6, null],
        [0.3, 0.4, null],
      ],
      3,
    );
    expect(stack[0]).toStrictEqual({ lo: [0, 0, null], hi: [0.5, 0.6, null] });
    expect(stack[1]?.lo).toStrictEqual([0.5, 0.6, null]);
    expect(stack[1]?.hi[0]).toBeCloseTo(0.8);
  });
  it("finds the layer under the pointer", () => {
    const stack = stackLayout([[0.5], [0.3]], 1);
    expect(layerAt(stack, 0, 0.2)).toBe(0);
    expect(layerAt(stack, 0, 0.7)).toBe(1);
    expect(layerAt(stack, 0, 0.95)).toBe(-1);
  });
});

describe("hit testing", () => {
  it("finds the nearest point", () => {
    expect(nearestIndex([0, 10, 20], 14)).toBe(1);
    expect(nearestIndex([0, 10, 20], 16)).toBe(2);
    expect(nearestIndex([0, 10, 20], -5)).toBe(0);
    expect(nearestIndex([], 3)).toBe(-1);
  });
  it("takes the extent of values with data", () => {
    expect(extent([3, null, 9], [1])).toStrictEqual([1, 9]);
    expect(extent([null])).toBeUndefined();
  });
});

describe("summary", () => {
  it("says where a series went and its range", () => {
    const text = describeSeries("Hashrate", [utc(2026, 1, 1), utc(2026, 2, 1)], [8e20, 9e20], (v) =>
      formatHashrate(v),
    );
    expect(text).toBe(
      "Hashrate went from 800 EH/s on 1 Jan 2026 to 900 EH/s on 1 Feb 2026; lowest 800 EH/s, highest 900 EH/s.",
    );
    expect(describeSeries("Fees", [], [], String)).toBe("Fees: no data.");
  });
});

describe("small figures", () => {
  it("keep their significant digits", () => {
    expect(formatSi(0.0481, "")).toBe("0.0481");
    expect(formatSi(0.04805, "", 4)).toBe("0.04805");
    // Regtest's difficulty, and beyond the last prefix: a power of ten, short at any size.
    expect(formatSi(4.6565423739069247e-10, "", 5)).toBe("4.66\u202F×\u202F10⁻¹⁰");
    expect(formatSi(4.6565423739069247e-10, "")).toBe("4.66\u202F×\u202F10⁻¹⁰");
    expect(formatSi(2 ** 224, "")).toBe("2.7\u202F×\u202F10⁶⁷");
    expect(formatSi(-0.0002, "H/s")).toBe("−2\u202F×\u202F10⁻⁴\u202FH/s");
    expect(formatSi(999e24, "H/s")).toBe("999\u202FYH/s");
    expect(formatSi(0.001, "")).toBe("0.001");
  });
});
