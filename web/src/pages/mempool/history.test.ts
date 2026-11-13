import { describe, expect, it } from "vitest";
import {
  coverageSentence,
  dataFrom,
  flowSentence,
  isHistory,
  netFlow,
  parseInterval,
  pointTime,
  seriesOf,
} from "./history";
import { history, point, T0 } from "./testdata";

describe("intervals", () => {
  it("reads every interval the server serves and falls back to a day", () => {
    expect(parseInterval("2h").id).toBe("2h");
    expect(parseInterval("all").id).toBe("all");
    expect(parseInterval(["1w", "2h"]).id).toBe("1w");
    expect(parseInterval("5m").id).toBe("24h");
    expect(parseInterval(undefined).id).toBe("24h");
  });
});

describe("checking a reply", () => {
  it("accepts the server's shape", () => {
    expect(isHistory(history())).toBe(true);
    expect(isHistory(history({ first: null, points: [] }))).toBe(true);
  });
  it("refuses anything it could not draw", () => {
    expect(isHistory(null)).toBe(false);
    expect(isHistory({ ...history(), step: 0 })).toBe(false);
    expect(isHistory(history({ points: [{ ...point(T0), bands: [1, 2] }] }))).toBe(false);
    expect(isHistory(history({ points: [{ ...point(T0), vsize: Number.NaN }] }))).toBe(false);
    expect(isHistory({ ...history(), points: [{ ...point(T0), count: "1" }] })).toBe(false);
  });
});

describe("coverage", () => {
  it("says nothing when every minute is there, or when there is nothing to qualify", () => {
    expect(coverageSentence(history())).toBeUndefined();
    expect(coverageSentence(history({ complete: false, points: [] }))).toBeUndefined();
  });
  it("says since when, and how much, when recording began inside the period", () => {
    const h = history({ first: T0 + 6060, complete: false, sampled: 18, minutes: 119 });
    expect(coverageSentence(h)).toBe("Recorded from 12 Nov 2026, 15:41 UTC: 15% of the period.");
    expect(dataFrom(h)).toBe(T0 + 6060);
  });
  it("names gaps as gaps when recording began before the period", () => {
    const h = history({ first: T0 - 3600, complete: false, sampled: 1, minutes: 1440 });
    expect(coverageSentence(h)).toBe(
      "Under 1% of the period recorded: the gaps are when this server was off.",
    );
    expect(dataFrom(h)).toBeUndefined();
  });
  it("ends the hatch where the step holding the first record starts, as its point is drawn there", () => {
    const h = history({
      step: 600,
      first: T0 + 3780,
      complete: false,
      sampled: 5,
      minutes: 10_000,
    });
    expect(dataFrom(h)).toBe(T0 + 3600);
  });
  it("says since when for all of history, which owes only the minutes since the first record", () => {
    // A young server: its first record at 14:00, grouped into the day that starts at midnight.
    const midnight = T0 - 14 * 3600;
    const all = {
      interval: "all",
      from: midnight,
      step: 86_400,
      first: T0,
      points: [point(midnight)],
    };
    expect(dataFrom(history({ ...all, complete: true }))).toBeUndefined();
    expect(coverageSentence(history({ ...all, complete: true }))).toBe(
      "Recorded every minute since 12 Nov 2026, 14:00 UTC.",
    );
    expect(coverageSentence(history({ ...all, complete: false, sampled: 30, minutes: 40 }))).toBe(
      "Recorded since 12 Nov 2026, 14:00 UTC: 75% of the minutes.",
    );
  });
  it("never rounds a share up to whole", () => {
    const h = history({ first: T0 - 60, complete: false, sampled: 1439, minutes: 1440 });
    expect(coverageSentence(h)).toContain("99%");
  });
});

describe("series", () => {
  it("has one entry per point and no zero where a step is missing", () => {
    const h = history({ points: [point(T0), point(T0 + 60), point(T0 + 300)] });
    const s = seriesOf(h);
    expect(s.x).toStrictEqual([T0, T0 + 60, T0 + 300]);
    expect(s.bands).toHaveLength(12);
    expect(s.bands[6]).toStrictEqual([400_000, 400_000, 400_000]);
    expect(s.unsplit).toBeUndefined();
    expect(s.floor[0]).toBe(0.1);
    expect(s.entered[0]).toBe(100);
    expect(s.mined[0]).toBe(40);
    expect(s.otherwise[0]).toBe(10);
  });
  it("reads a point's time to the minute, or to the day for daily steps", () => {
    expect(pointTime(60)(T0)).toBe("12 Nov 2026, 14:00 UTC");
    expect(pointTime(86_400)(T0)).toBe("12 Nov 2026");
  });
});

describe("flow", () => {
  it("weights each point by its minutes and says which way the mempool went", () => {
    const h = history({
      points: [
        point(T0, { samples: 1, in_vsize: 6000, out_vsize: 0, mined_vsize: 0 }),
        point(T0 + 600, { samples: 3, in_vsize: 6000, out_vsize: 12_000, mined_vsize: 12_000 }),
      ],
    });
    expect(netFlow(h)).toStrictEqual({ entered: 100, mined: 150, otherwise: 0, net: -50 });
    expect(flowSentence(h, "the last week")).toBe(
      "Over the last week, 100 vB/s arrived, blocks took 150 vB/s and 0 vB/s left otherwise: the mempool drained by 50 vB/s on average.",
    );
  });
  it("has nothing to say about an empty period", () => {
    expect(flowSentence(history({ points: [] }), "x")).toBeUndefined();
  });
});
