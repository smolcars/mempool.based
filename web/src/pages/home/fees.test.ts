import { describe, expect, it } from "vitest";
import { formatFeeRateUp } from "../../lib/format";
import { type FeeAnswer, groupLevels } from "./fees";

const answer = (high: number, medium: number, low: number, noRush: number): FeeAnswer => ({
  source: "projected",
  levels: [
    { name: "high", blocks: 1, minutes: 10, rate: high },
    { name: "medium", blocks: 3, minutes: 30, rate: medium },
    { name: "low", blocks: 6, minutes: 60, rate: low },
    { name: "no_rush", blocks: 144, minutes: 1440, rate: noRush },
  ],
});

describe("groupLevels", () => {
  it("shows four answers, slowest first, when every rate prints differently", () => {
    const g = groupLevels(answer(3.31, 0.246, 0.215, 0.133), formatFeeRateUp);
    expect(g.map((x) => [x.name, x.when, x.span])).toStrictEqual([
      ["No rush", "within a day", 1],
      ["Low", "within an hour", 1],
      ["Medium", "within 30 min", 1],
      ["High", "next block", 1],
    ]);
  });

  it("reads four equal rates as one answer (signet's 0.100 on every level)", () => {
    const g = groupLevels(answer(0.1, 0.1, 0.1, 0.1), formatFeeRateUp);
    expect(g).toStrictEqual([{ name: "Any wait", when: "next block", span: 4, rate: 0.1 }]);
  });

  it("merges neighbours that print the same, and promises the fastest wait among them", () => {
    const g = groupLevels(answer(1.001, 0.5, 0.1, 0.1), formatFeeRateUp);
    expect(g.map((x) => [x.name, x.when, x.span])).toStrictEqual([
      ["No rush or low", "within an hour", 2],
      ["Medium", "within 30 min", 1],
      ["High", "next block", 1],
    ]);
    // Low and Medium alone are not a row of the panel's two by two: they stay apart, same rate.
    // 0.121 and 0.129 both print 0.13 rounded up.
    const middle = groupLevels(answer(1.001, 0.129, 0.121, 0.1), formatFeeRateUp);
    expect(middle.map((x) => [x.name, x.span, x.rate])).toStrictEqual([
      ["No rush", 1, 0.1],
      ["Low", 1, 0.121],
      ["Medium", 1, 0.129],
      ["High", 1, 1.001],
    ]);
    const three = groupLevels(answer(2, 0.1, 0.1, 0.1), formatFeeRateUp);
    expect(three[0]?.name).toBe("No rush, low or medium");
  });

  it("keeps the four places with no answer yet", () => {
    const g = groupLevels(undefined, formatFeeRateUp);
    expect(g).toHaveLength(4);
    expect(g.every((x) => x.rate === undefined)).toBe(true);
  });
});
