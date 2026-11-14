import { describe, expect, it } from "vitest";
import { readLedger, waiting } from "./ledger";

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
      txs: [{ txid: TXID, height: 110, position: 1, value: -5, time: 1_767_291_600 }],
      points: [{ height: 100, time: 1_767_285_600, balance: 10 }],
    });
    for (const h of ["none", "taking", "whole", "refused", "dropped"]) {
      expect(readLedger({ ...tracked, history: h })).toMatchObject({ history: h });
    }
    // An unknown word reads as not kept whole.
    expect(readLedger({ ...tracked, history: "<b>" })).toMatchObject({ history: "none" });
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
