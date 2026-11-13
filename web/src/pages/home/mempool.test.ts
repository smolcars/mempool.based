import { describe, expect, it } from "vitest";
import {
  bandShares,
  depthRows,
  isMempoolSummary,
  isRecentList,
  lowestRate,
  nextBlockReach,
  sharesOf,
} from "./mempool";

describe("bandShares", () => {
  it("puts each histogram entry in the band of its rate as printed", () => {
    // Dearest first, as the API sends it.
    const bands = bandShares([
      [25, 100],
      [10, 100],
      [9.999, 50],
      [1, 300],
      [0.1996, 200],
      [0.12, 250],
    ]);
    expect(bands.map((b) => b.vsize)).toStrictEqual([0, 250, 0, 200, 0, 0, 300, 0, 0, 0, 0, 250]);
    expect(bands.reduce((a, b) => a + b.share, 0)).toBeCloseTo(1);
    expect(bands[0]).toMatchObject({ band: 1, from: 0, to: 0.12, x: 0, share: 0 });
    expect(bands[1]).toMatchObject({ band: 2, from: 0.12, to: 0.15, x: 0, share: 0.25 });
    expect(bands[3]).toMatchObject({ band: 4, x: 0.25, share: 0.2 });
    expect(bands[11]).toMatchObject({ band: 12, from: 10, to: null, x: 0.75, share: 0.25 });
  });

  it("is twelve empty bands for an empty mempool", () => {
    const bands = bandShares([]);
    expect(bands).toHaveLength(12);
    expect(bands.every((b) => b.share === 0 && b.vsize === 0)).toBe(true);
  });
});

describe("lowestRate", () => {
  it("is the last entry's rate, or nothing when empty", () => {
    expect(
      lowestRate([
        [5, 1],
        [0.127, 2],
      ]),
    ).toBe(0.127);
    expect(lowestRate([])).toBeUndefined();
  });
});

describe("reply checks", () => {
  it("accept the API's shapes and refuse anything else", () => {
    expect(isMempoolSummary({ count: 1, vsize: 2, total_fee: 3, fee_histogram: [[1, 2]] })).toBe(
      true,
    );
    expect(isMempoolSummary({ count: 1, vsize: 2, total_fee: 3, fee_histogram: [["1", 2]] })).toBe(
      false,
    );
    expect(isRecentList([{ txid: "a".repeat(64), fee: 1, vsize: 1, value: 1 }])).toBe(true);
    expect(isRecentList([{ txid: "<b>", fee: 1, vsize: 1, value: 1 }])).toBe(false);
  });
});

describe("depthRows", () => {
  it("measures each band in blocks' worth, dearest first, and how deep the queue is by its end", () => {
    const vsize = [
      29_000_000, 0, 0, 5_000, 25_000, 117_000, 286_000, 62_000, 376_000, 625_000, 327_000, 37_000,
    ];
    const rows = depthRows(sharesOf(vsize));
    expect(rows.map((r) => r.band)).toStrictEqual([12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1]);
    expect(rows[0]?.blocks).toBeCloseTo(0.037, 6);
    expect(rows[2]?.deep).toBeCloseTo(0.989, 6);
    // On a log scale from 100 vB to a block: the bulk row fills its bar and no more, and rows of
    // a few kvB keep a length that can be compared.
    expect(rows[11]?.bar).toBe(1);
    expect(rows[2]?.bar).toBeCloseTo(Math.log10(6250) / 4, 6);
    expect(rows[8]?.bar).toBeCloseTo(Math.log10(50) / 4, 6);
    expect(rows[11]?.deep).toBeCloseTo(30.86, 2);
    // The next block's worth runs out in the 2–3 band.
    expect(nextBlockReach(rows)?.band).toBe(9);
  });

  it("says nothing reaches a block when everything waiting fits in one", () => {
    const rows = depthRows(sharesOf([1000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2000]));
    expect(nextBlockReach(rows)).toBeUndefined();
    expect(rows.at(-1)?.deep).toBeCloseTo(0.003, 6);
  });
});
