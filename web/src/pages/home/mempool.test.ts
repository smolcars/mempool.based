import { describe, expect, it } from "vitest";
import {
  bandShares,
  estimateFor,
  isFeeEstimates,
  isMempoolSummary,
  isRecentList,
  ladderPos,
  lowestRate,
  stackLabels,
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
    expect(bands.map((b) => b.vsize)).toEqual([250, 200, 0, 0, 300, 0, 0, 0, 0, 250]);
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

describe("estimateFor", () => {
  const est = { "1": 2.7, "2": 0.7, "3": 0.29, "6": 0.23, "144": 0.13, "1008": 0.11 };
  it("takes the exact target, else the nearest shorter one", () => {
    expect(estimateFor(est, 1)).toBe(2.7);
    expect(estimateFor(est, 6)).toBe(0.23);
    expect(estimateFor(est, 100)).toBe(0.23);
    expect(estimateFor({ "2": 1 }, 1)).toBeUndefined();
    expect(estimateFor({}, 144)).toBeUndefined();
  });
});

describe("ladderPos", () => {
  it("places rates on a log scale from 0.1 to 20, clamped", () => {
    expect(ladderPos(0.1)).toBe(0);
    expect(ladderPos(20)).toBeCloseTo(1);
    expect(ladderPos(0.01)).toBe(0);
    expect(ladderPos(361)).toBeCloseTo(1);
    expect(ladderPos(Math.sqrt(2))).toBeCloseTo(0.5);
  });
});

describe("stackLabels", () => {
  it("puts close labels in separate rows and far ones in the first", () => {
    const out = stackLabels(
      [
        { x: 0, width: 40 },
        { x: 30, width: 40 },
        { x: 60, width: 40 },
        { x: 200, width: 40 },
      ],
      400,
    );
    expect(out.map((o) => o.row)).toEqual([0, 1, 0, 0]);
  });

  it("flips a label that would run past the end", () => {
    expect(stackLabels([{ x: 390, width: 40 }], 400)).toEqual([{ row: 0, flip: true }]);
  });

  it("works in order of position whatever order the pins come in", () => {
    const out = stackLabels(
      [
        { x: 30, width: 40 },
        { x: 0, width: 40 },
      ],
      400,
    );
    expect(out.map((o) => o.row)).toEqual([1, 0]);
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
    expect(isFeeEstimates({ "1": 2.5, "144": 1 })).toBe(true);
    expect(isFeeEstimates({ fast: 2 })).toBe(false);
    expect(isRecentList([{ txid: "a".repeat(64), fee: 1, vsize: 1, value: 1 }])).toBe(true);
    expect(isRecentList([{ txid: "<b>", fee: 1, vsize: 1, value: 1 }])).toBe(false);
  });
});
