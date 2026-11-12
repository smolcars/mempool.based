import { describe, expect, it } from "vitest";
import {
  btcParts,
  FEE_BOUNDS,
  feeBand,
  feeColor,
  feeRate,
  formatBtc,
  formatFeeRate,
  formatRelative,
  formatSats,
  formatSize,
  formatUtc,
  groupDigits,
  hashGroups,
  isoTime,
  MISSING,
  NNBSP,
  truncateMiddle,
  vsizeFromWeight,
} from "./format";

// Spaces in expectations are written as "_" so the narrow no-break space is visible.
const s = (text: string) => text.replaceAll("_", NNBSP);

describe("groupDigits", () => {
  it("groups in threes with a narrow no-break space", () => {
    expect(groupDigits(0)).toBe("0");
    expect(groupDigits(999)).toBe("999");
    expect(groupDigits(1000)).toBe(s("1_000"));
    expect(groupDigits(975411)).toBe(s("975_411"));
    expect(groupDigits(2_100_000_000_000_000)).toBe(s("2_100_000_000_000_000"));
  });

  it("uses a real minus sign and rounds", () => {
    expect(groupDigits(-1234)).toBe(s("−1_234"));
    expect(groupDigits(1234.6)).toBe(s("1_235"));
  });

  it("never prints NaN or Infinity", () => {
    expect(groupDigits(Number.NaN)).toBe(MISSING);
    expect(groupDigits(Number.POSITIVE_INFINITY)).toBe(MISSING);
  });
});

describe("BTC and sats", () => {
  it("always shows eight decimals", () => {
    expect(formatBtc(0)).toBe("0.00000000");
    expect(formatBtc(1)).toBe("0.00000001");
    expect(formatBtc(3_500_000)).toBe("0.03500000");
    expect(formatBtc(100_000_000)).toBe("1.00000000");
    expect(formatBtc(2_100_000_000_000_000)).toBe(s("21_000_000.00000000"));
  });

  it("has no floating-point error where floats would have one", () => {
    // 0.1 + 0.2 in BTC, and amounts whose float division is inexact.
    expect(formatBtc(10_000_000 + 20_000_000)).toBe("0.30000000");
    expect(formatBtc(29_999_999)).toBe("0.29999999");
    expect(formatBtc(123_456_789_012_345)).toBe(s("1_234_567.89012345"));
    expect(formatBtc(Number.MAX_SAFE_INTEGER)).toBe(s("90_071_992.54740991"));
  });

  it("splits trailing zeros off so they can be dimmed", () => {
    expect(btcParts(3_500_000)).toEqual({
      sign: "",
      whole: "0",
      significant: "035",
      zeros: "00000",
    });
    expect(btcParts(100_000_000)).toEqual({
      sign: "",
      whole: "1",
      significant: "",
      zeros: "00000000",
    });
    expect(btcParts(-12_345)).toEqual({
      sign: "−",
      whole: "0",
      significant: "00012345",
      zeros: "",
    });
  });

  it("refuses anything that is not a safe integer", () => {
    for (const bad of [0.5, Number.NaN, Number.POSITIVE_INFINITY, 2 ** 53]) {
      expect(btcParts(bad)).toBeNull();
      expect(formatBtc(bad)).toBe(MISSING);
      expect(formatSats(bad)).toBe(MISSING);
    }
  });

  it("groups sats", () => {
    expect(formatSats(167)).toBe("167");
    expect(formatSats(1_250_000)).toBe(s("1_250_000"));
  });
});

describe("formatFeeRate", () => {
  it("prints two decimals under 10, one under 100, none above", () => {
    expect(formatFeeRate(0)).toBe("0.00");
    expect(formatFeeRate(0.1)).toBe("0.10");
    expect(formatFeeRate(0.8)).toBe("0.80");
    expect(formatFeeRate(4)).toBe("4.00");
    expect(formatFeeRate(12.345)).toBe("12.3");
    expect(formatFeeRate(99.94)).toBe("99.9");
    expect(formatFeeRate(250.4)).toBe("250");
    expect(formatFeeRate(1234)).toBe(s("1_234"));
  });

  it("keeps sub-sat resolution", () => {
    expect(formatFeeRate(0.123)).toBe("0.12");
    expect(formatFeeRate(0.125)).toBe("0.13");
    expect(formatFeeRate(0.1996)).toBe("0.20");
    expect(formatFeeRate(141 / 1000)).toBe("0.14");
  });

  it("rounds halves up even when the float is just below the half", () => {
    // 1.005 is stored as 1.00499999999999989…
    expect(formatFeeRate(1.005)).toBe("1.01");
    expect(formatFeeRate(201 / 200)).toBe("1.01");
    expect(formatFeeRate(0.285)).toBe("0.29");
    expect(formatFeeRate(10.05)).toBe("10.1");
  });

  it("chooses the precision from the rounded value", () => {
    expect(formatFeeRate(9.994)).toBe("9.99");
    expect(formatFeeRate(9.995)).toBe("10.0");
    expect(formatFeeRate(99.95)).toBe("100");
  });

  it("never prints NaN, Infinity or a negative rate", () => {
    for (const bad of [Number.NaN, Number.POSITIVE_INFINITY, -1]) {
      expect(formatFeeRate(bad)).toBe(MISSING);
    }
  });

  it("computes a rate from fee and size", () => {
    expect(feeRate(141, 141)).toBe(1);
    expect(feeRate(100, 0)).toBeNull();
    expect(feeRate(Number.NaN, 10)).toBeNull();
    expect(vsizeFromWeight(561)).toBe(141);
    expect(vsizeFromWeight(564)).toBe(141);
  });
});

describe("feeBand", () => {
  it("puts each lower bound in its own band", () => {
    expect(feeBand(0.1)).toBe(1);
    FEE_BOUNDS.forEach((bound, i) => {
      expect(feeBand(bound)).toBe(i + 2);
    });
    expect(feeBand(1000)).toBe(10);
  });

  it("puts values just under a bound in the band below", () => {
    expect(feeBand(0.19)).toBe(1);
    expect(feeBand(0.99)).toBe(4);
    expect(feeBand(9.99)).toBe(9);
  });

  it("bands the rate as printed, so the color never disagrees with the number", () => {
    // 0.1996 prints as 0.20, so it is band 2 although the raw value is below 0.2.
    expect(feeBand(0.1996)).toBe(2);
    expect(feeBand(0.1949)).toBe(1);
    expect(feeBand(0.995)).toBe(5);
    expect(feeBand(9.996)).toBe(10);
    expect(feeColor(0.1996)).toBe("var(--fee-2)");
  });

  it("puts nonsense in the first band instead of failing", () => {
    expect(feeBand(Number.NaN)).toBe(1);
  });
});

describe("formatSize", () => {
  it("uses grouped vB under ten thousand", () => {
    expect(formatSize(209)).toBe("209 vB");
    expect(formatSize(9999)).toBe(s("9_999 vB"));
  });

  it("uses kvB and MvB with two decimals above", () => {
    expect(formatSize(10_000)).toBe("10.00 kvB");
    expect(formatSize(14_620_000)).toBe("14.62 MvB");
    expect(formatSize(998_765)).toBe("998.77 kvB");
  });

  it("moves up a unit when rounding reaches the next one", () => {
    expect(formatSize(9999.6)).toBe("10.00 kvB");
    expect(formatSize(999_995)).toBe("1.00 MvB");
  });

  it("takes other units", () => {
    expect(formatSize(3_993_000, "WU")).toBe("3.99 MWU");
    expect(formatSize(285, "B")).toBe("285 B");
    expect(formatSize(-1)).toBe(MISSING);
  });
});

describe("hashes", () => {
  const txid = "65428488a6202f1d681d053b3ff5d8ea9f5b0e5e1e0dcf7bf2e4c1e5a3f9c2e1";

  it("truncates in the middle", () => {
    expect(truncateMiddle(txid)).toBe("65428488…a3f9c2e1");
    expect(truncateMiddle("bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh", 7, 5)).toBe(
      "bc1qxy2…x0wlh",
    );
  });

  it("leaves short values whole", () => {
    expect(truncateMiddle("abcdef")).toBe("abcdef");
    expect(truncateMiddle("a".repeat(17))).toBe("a".repeat(17));
    expect(truncateMiddle("a".repeat(18))).toBe("aaaaaaaa…aaaaaaaa");
  });

  it("groups by eight", () => {
    const groups = hashGroups(txid);
    expect(groups).toHaveLength(8);
    expect(groups[0]).toBe("65428488");
    expect(groups.join("")).toBe(txid);
    expect(hashGroups("abcdefghij")).toEqual(["abcdefgh", "ij"]);
    expect(hashGroups("")).toEqual([]);
  });
});

describe("time", () => {
  const t = 1_794_429_667; // 2026-11-11 20:41:07 UTC

  it("formats absolute UTC", () => {
    expect(formatUtc(t)).toBe("2026-11-11 20:41:07 UTC");
    expect(isoTime(t)).toBe("2026-11-11T20:41:07.000Z");
    expect(formatUtc(Number.NaN)).toBe(MISSING);
    expect(isoTime(Number.NaN)).toBeUndefined();
  });

  it("formats relative time at each step", () => {
    expect(formatRelative(t, t)).toBe("just now");
    expect(formatRelative(t, t + 9)).toBe("just now");
    expect(formatRelative(t, t + 10)).toBe("10 s ago");
    expect(formatRelative(t, t + 59)).toBe("59 s ago");
    expect(formatRelative(t, t + 60)).toBe("1 min ago");
    expect(formatRelative(t, t + 13 * 60 + 59)).toBe("13 min ago");
    expect(formatRelative(t, t + 3600)).toBe("1 h ago");
    expect(formatRelative(t, t + 2 * 3600 + 5 * 60)).toBe("2 h 5 min ago");
    expect(formatRelative(t, t + 47 * 3600 + 59 * 60)).toBe("47 h 59 min ago");
    expect(formatRelative(t, t + 48 * 3600)).toBe("2 days ago");
  });

  it("drops the suffix on request and never goes negative", () => {
    expect(formatRelative(t, t + 13 * 60, { suffix: false })).toBe("13 min");
    expect(formatRelative(t + 300, t)).toBe("just now");
    expect(formatRelative(Number.NaN, t)).toBe(MISSING);
  });
});
