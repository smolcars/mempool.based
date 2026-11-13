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
    expect(coverageSentence(history(), T0)).toBeUndefined();
    expect(coverageSentence(history({ complete: false, points: [] }), T0)).toBeUndefined();
  });
  it("says since when, and how much, when recording began inside the period", () => {
    const h = history({ first: T0 + 6060, complete: false, sampled: 18, minutes: 119 });
    expect(coverageSentence(h, T0)).toBe("15% recorded, from 12 Nov, 15:41 UTC.");
    expect(dataFrom(h)).toBe(T0 + 6060);
    // A year that is not this one stays.
    expect(coverageSentence(h, T0 + 60 * 86_400)).toBe(
      "15% recorded, from 12 Nov 2026, 15:41 UTC.",
    );
  });
  it("names gaps as gaps when recording began before the period", () => {
    const h = history({ first: T0 - 3600, complete: false, sampled: 1, minutes: 1440 });
    expect(coverageSentence(h, T0)).toBe("Under 1% recorded, with gaps while this server was off.");
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
    expect(coverageSentence(history({ ...all, complete: true }), T0)).toBe(
      "Every minute recorded since 12 Nov, 14:00 UTC.",
    );
    expect(
      coverageSentence(history({ ...all, complete: false, sampled: 30, minutes: 40 }), T0),
    ).toBe("75% recorded, since 12 Nov, 14:00 UTC.");
  });
  it("never rounds a share up to whole", () => {
    const h = history({ first: T0 - 60, complete: false, sampled: 1439, minutes: 1440 });
    expect(coverageSentence(h, T0)).toContain("99%");
  });
});

describe("series", () => {
  const ten = (u: number) => [u, 50, 0, 0, 0, 0, 0, 0, 0, 0];
  const twelve = (a: number, b: number) => [a, b, 0, 50, 0, 0, 0, 0, 0, 0, 0, 0];
  it("joins minutes from before the split to the three finer bands at one point", () => {
    const h = history({
      points: [
        point(T0, { bands: ten(900), vsize: 950 }),
        point(T0 + 60, { bands: twelve(300, 600), vsize: 950 }),
      ],
    });
    const s = seriesOf(h);
    expect(s.unsplit).toStrictEqual([900, 0]);
    expect(s.bands[0]).toStrictEqual([0, 300]);
    expect(s.bands[1]).toStrictEqual([0, 600]);
    // Just before the split point the same total, all under 0.2 as one layer: a vertical edge.
    expect(s.joins).toStrictEqual([
      { index: 1, before: [900, 0, 0, 0, 50, 0, 0, 0, 0, 0, 0, 0, 0] },
    ]);
  });
  it("joins the other way too, leaving the last split point as one layer", () => {
    const h = history({
      points: [
        point(T0, { bands: twelve(300, 600), vsize: 950 }),
        point(T0 + 60, { bands: ten(900), vsize: 950 }),
      ],
    });
    const s = seriesOf(h);
    // The split point keeps its recorded bands for the readout and the figures; only the
    // drawing leaves it as one layer.
    expect(s.unsplit).toStrictEqual([0, 900]);
    expect(s.bands[0]).toStrictEqual([300, 0]);
    expect(s.bands[1]).toStrictEqual([600, 0]);
    expect(s.joins).toStrictEqual([
      { index: 0, after: [900, 0, 0, 0, 50, 0, 0, 0, 0, 0, 0, 0, 0] },
    ]);
    // Every point's layers still add up to its total.
    for (let i = 0; i < 2; i++) {
      const sum = (s.unsplit?.[i] ?? 0) + s.bands.reduce((t, b) => t + (b[i] ?? 0), 0);
      expect(sum).toBe(950);
    }
  });

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
