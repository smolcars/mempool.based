import { describe, expect, it } from "vitest";
import { asksAgain, FAILED_POLL_MS, KEPT_POLL_MS, readLedger, waiting } from "./ledger";

const TXID = "0def111d15485da613e9dc24a587a9657d3726772dcc68e0c55c2daa67b1dca9";
const tracked = {
  state: "tracked",
  chain_stats: {},
  balance: 5,
  at: { height: 110, hash: "00", time: 1_767_291_600 },
  seeded: { height: 100, time: 1_767_285_600 },
  window: 200,
  filling: false,
  txs: [{ txid: TXID, height: 110, position: 1, value: -5, time: 1_767_291_600 }],
  points: [{ height: 100, time: 1_767_285_600, balance: 10 }],
};

describe("readLedger", () => {
  it("reads every state the server sends", () => {
    expect(readLedger(tracked)).toStrictEqual({
      state: "tracked",
      at: { height: 110, time: 1_767_291_600 },
      seeded: { height: 100, time: 1_767_285_600 },
      window: 200,
      filling: false,
      history: "none",
      historyAgain: null,
      txs: [{ txid: TXID, height: 110, position: 1, value: -5, time: 1_767_291_600 }],
      points: [{ height: 100, time: 1_767_285_600, balance: 10 }],
    });
    for (const h of ["none", "taking", "whole", "refused", "dropped", "too_large"]) {
      expect(readLedger({ ...tracked, history: h })).toMatchObject({ history: h });
    }
    // An unknown word reads as not kept whole.
    expect(readLedger({ ...tracked, history: "<b>" })).toMatchObject({ history: "none" });
    expect(
      readLedger({ ...tracked, history: "dropped", history_again: 1_767_300_000 }),
    ).toMatchObject({ history: "dropped", historyAgain: 1_767_300_000 });
    expect(readLedger({ ...tracked, history_again: "soon" })).toMatchObject({ historyAgain: null });
    expect(readLedger({ state: "queued", position: 2 })).toStrictEqual({
      state: "queued",
      position: 2,
    });
    expect(readLedger({ state: "seeding", seconds: 30 })).toStrictEqual({
      state: "seeding",
      seconds: 30,
    });
    expect(readLedger({ state: "refused", creates: 20_000, cap: 10_000 })).toStrictEqual({
      state: "refused",
      creates: 20_000,
      cap: 10_000,
    });
    expect(readLedger({ state: "refused", creates: null, cap: null })).toStrictEqual({
      state: "refused",
      creates: null,
      cap: null,
    });
    expect(readLedger({ state: "failed" }).state).toBe("failed");
    expect(readLedger({ state: "busy" }).state).toBe("busy");
    expect(readLedger({ state: "none" }).state).toBe("none");
  });

  it("reads anything unexpected as nothing known, never in part", () => {
    for (const bad of [
      null,
      "tracked",
      [],
      { state: "unheard of" },
      { state: "queued", position: -1 },
      { state: "seeding", seconds: 1.5 },
      { ...tracked, at: null },
      { ...tracked, window: "200" },
      { ...tracked, filling: undefined },
      { ...tracked, txs: [{ ...tracked.txs[0], txid: "<script>" }] },
      { ...tracked, txs: [...tracked.txs, { txid: TXID }] },
      { ...tracked, points: [{ height: 1, time: 1, balance: Number.MAX_VALUE }] },
    ]) {
      expect(readLedger(bad)).toStrictEqual({ state: "none" });
    }
  });

  it("keeps asking only while the seed waits or runs", () => {
    expect(waiting({ state: "queued", position: 1 })).toBe(true);
    expect(waiting({ state: "seeding", seconds: 0 })).toBe(true);
    expect(waiting({ state: "busy" })).toBe(true);
    expect(waiting({ state: "failed" })).toBe(false);
    expect(waiting({ state: "none" })).toBe(false);
  });
});

describe("asksAgain", () => {
  it("keeps a kept address's page known as viewed, and a waiting one up to date", () => {
    const kept = readLedger(tracked);
    // An open page of a kept address asks every few minutes: the server takes its whole history
    // only while it is viewed, and drops none viewed in the last half hour.
    expect(asksAgain(kept, KEPT_POLL_MS - 1)).toBe(false);
    expect(asksAgain(kept, KEPT_POLL_MS)).toBe(true);
    expect(KEPT_POLL_MS).toBeLessThan(30 * 60_000);
    expect(asksAgain({ state: "queued", position: 1 }, 0)).toBe(true);
    expect(asksAgain({ state: "failed" }, FAILED_POLL_MS - 1)).toBe(false);
    expect(asksAgain({ state: "failed" }, FAILED_POLL_MS)).toBe(true);
    expect(asksAgain({ state: "none" }, 10 * KEPT_POLL_MS)).toBe(false);
  });
});
