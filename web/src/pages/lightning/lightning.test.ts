import { describe, expect, it } from "vitest";
import {
  cleanAlias,
  msatShort,
  coverageSentence,
  cut,
  dataFrom,
  isChannelDetail,
  isChannelId,
  isNodeDetail,
  isOverview,
  isSeries,
  parseInterval,
  parseRanking,
  searchProblem,
} from "./lightning";
import { acinq, channel, closedOf, overview, series, T0 } from "./testdata";

describe("cleanAlias", () => {
  it("keeps an ordinary name as it is", () => {
    expect(cleanAlias("ACINQ")).toBe("ACINQ");
    expect(cleanAlias("Kraken 🐙⚡")).toBe("Kraken 🐙⚡");
    // The joiner emoji are built with stays.
    expect(cleanAlias("👨‍👩‍👧")).toBe("👨‍👩‍👧");
  });

  it("shows bidi overrides, isolates and invisible characters as a replacement mark", () => {
    expect(cleanAlias("‮QNICA")).toBe("�QNICA");
    expect(cleanAlias("AC​INQ")).toBe("AC�INQ");
    expect(cleanAlias("a⁦b⁩")).toBe("a�b�");
  });

  it("shows control characters as a replacement mark", () => {
    expect(cleanAlias("a\u0000b\u001B[31mc\u0085")).toBe("a�b�[31mc�");
  });

  it("thins stacked combining marks to two", () => {
    expect(cleanAlias(`Z${"́".repeat(40)}`)).toBe("Ź́");
  });

  it("gives null for no name, a non-string, or nothing visible", () => {
    expect(cleanAlias(null)).toBeNull();
    expect(cleanAlias(42)).toBeNull();
    expect(cleanAlias("   ")).toBeNull();
  });
});

describe("cut", () => {
  it("cuts by characters as read, never halving an emoji", () => {
    expect(cut("abc", 3)).toStrictEqual({ text: "abc", more: false });
    expect(cut("abcd", 3)).toStrictEqual({ text: "abc…", more: true });
    expect(cut("👨‍👩‍👧👨‍👩‍👧", 1)).toStrictEqual({ text: "👨‍👩‍👧…", more: true });
  });
});

describe("shape checks", () => {
  it("accept the server's answers", () => {
    expect(isOverview(overview())).toBe(true);
    expect(isOverview(overview({ statistics: null, closed: [] }))).toBe(true);
    expect(isNodeDetail(acinq())).toBe(true);
    expect(isChannelDetail(channel())).toBe(true);
    expect(isChannelDetail(channel({ closed: closedOf("penalty").closed }))).toBe(true);
    expect(isSeries(series())).toBe(true);
  });

  it("refuse what a page could not draw", () => {
    expect(isOverview({ ...overview(), opened: "x" })).toBe(false);
    expect(isOverview({ ...overview(), closed: [{ ...closedOf("force"), capacity: "1" }] })).toBe(
      false,
    );
    const odd = { block: { height: 1, hash: "h", time: 1 }, txid: "t", kind: "x", fee: null };
    expect(isOverview({ ...overview(), closed: [{ ...closedOf("force"), closed: odd }] })).toBe(
      false,
    );
    expect(isNodeDetail({ ...acinq(), features: [{ bit: "0" }] })).toBe(false);
    expect(isChannelDetail({ ...channel(), funding: null })).toBe(false);
    expect(isSeries({ ...series(), step: 0 })).toBe(false);
  });
});

describe("ids and searches", () => {
  it("knows a channel id in either form", () => {
    expect(isChannelId("811984x2037x0")).toBe(true);
    expect(isChannelId("892785849701564416")).toBe(true);
    expect(isChannelId("0811984x2037x0")).toBe(false);
    expect(isChannelId("<script>")).toBe(false);
  });

  it("says why a search cannot be asked", () => {
    expect(searchProblem("ac")).toMatch(/at least 3/);
    expect(searchProblem("a".repeat(67))).toMatch(/at most 66/);
    expect(searchProblem("ab\u0007c")).toMatch(/control/);
    expect(searchProblem("  acinq  ")).toBeUndefined();
    // A surrogate pair is one character, as the server counts.
    expect(searchProblem("🐙🐙🐙")).toBeUndefined();
  });

  it("reads the interval and the ranking from the address, with defaults", () => {
    expect(parseInterval("1m").id).toBe("1m");
    expect(parseInterval("nonsense").id).toBe("1w");
    expect(parseRanking("age").id).toBe("age");
    expect(parseRanking(undefined).id).toBe("capacity");
  });
});

describe("coverage", () => {
  it("says since when for a history ten minutes old, and hatches before it", () => {
    const s = series();
    expect(coverageSentence(s, "the last week")).toBe(
      "This server has recorded the network since 12 Nov 2026, 18:30 UTC, every ten minutes while its Lightning peer runs: the chart starts there.",
    );
    expect(dataFrom(s)).toBe(T0);
  });

  it("names gaps inside a period it fully spans", () => {
    const s = series({ first: T0 - 30 * 86_400, sampled: 900, complete: false });
    expect(coverageSentence(s, "the last week")?.replaceAll("\u202F", " ")).toBe(
      "This view holds 900 ten-minute snapshots of the 1 007 in the last week. The gaps are times this server was not recording.",
    );
    expect(dataFrom(s)).toBeUndefined();
    expect(coverageSentence({ ...s, complete: true }, "the last week")).toBeUndefined();
  });
});

describe("msatShort", () => {
  it("reads millisatoshis in magnitude: msat, then sats, then BTC", () => {
    expect(msatShort(900)).toBe("900 msat");
    expect(msatShort(1000)).toBe("1 sat");
    expect(msatShort(1500)).toBe("1.5 sats");
    expect(msatShort(958_366_000)).toBe("958\u202F366 sats");
    expect(msatShort(18_958_366_000)).toBe("0.190 BTC");
    expect(msatShort(2_100_000_000_000_000_000)).toBe("21\u202F000\u202F000 BTC");
  });
});
