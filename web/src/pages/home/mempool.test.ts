import { describe, expect, it } from "vitest";
import {
  bandShares,
  isMempoolSummary,
  isRecentList,
  lowestRate,
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
    expect(bands.map((b) => b.vsize)).toStrictEqual([250, 200, 0, 0, 300, 0, 0, 0, 0, 250]);
    expect(bands.reduce((a, b) => a + b.share, 0)).toBeCloseTo(1);
    expect(bands[0]).toMatchObject({ band: 1, from: 0, to: 0.2, x: 0, share: 0.25 });
    expect(bands[1]).toMatchObject({ band: 2, x: 0.25, share: 0.2 });
    expect(bands[9]).toMatchObject({ band: 10, from: 10, to: null, x: 0.75, share: 0.25 });
  });

  it("is ten empty bands for an empty mempool", () => {
    const bands = bandShares([]);
    expect(bands).toHaveLength(10);
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
