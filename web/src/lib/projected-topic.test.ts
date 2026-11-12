import { describe, expect, it } from "vitest";
import { projectedTopic } from "./projected-topic";

const HASH = "ab".repeat(32);
const block = {
  txs: 3214,
  weight: 3_991_876,
  vsize: 997_969,
  fees: 1_543_210,
  min_rate: 1012,
  median_rate: 1500,
  max_rate: 250_000,
  bands: [0, 0, 0, 0, 312_000, 0, 0, 0, 0, 685_969],
};
const msg = (extra: object = {}) => ({
  topic: "projected",
  type: "projected",
  seq: 812,
  tip: { height: 975_530, hash: HASH },
  stale: false,
  blocks: [block, block],
  ...extra,
});

describe("projectedTopic", () => {
  it("reads the totals, with rates in sat/vB", () => {
    expect(projectedTopic.parse(msg())).toStrictEqual({
      type: "projected",
      projection: {
        seq: 812,
        tip: { height: 975_530, hash: HASH },
        stale: false,
        blocks: [
          {
            txs: 3214,
            weight: 3_991_876,
            vsize: 997_969,
            fees: 1_543_210,
            minRate: 1.012,
            medianRate: 1.5,
            maxRate: 250,
            bands: block.bands,
          },
          expect.anything(),
        ],
      },
    });
  });

  it("takes a missing tip and `off`", () => {
    expect(projectedTopic.parse(msg({ tip: null }))).toMatchObject({ projection: { tip: null } });
    expect(projectedTopic.parse({ topic: "projected", type: "off" })).toStrictEqual({
      type: "off",
    });
  });

  it("ignores anything malformed rather than show a wrong figure", () => {
    const bad = [
      msg({ seq: -1 }),
      msg({ stale: "no" }),
      msg({ tip: { height: 1, hash: "<b>" } }),
      msg({ blocks: [] }),
      msg({ blocks: [{ ...block, txs: 1.5 }] }),
      msg({ blocks: [{ ...block, bands: [1, 2] }] }),
      msg({ blocks: [{ ...block, bands: [...block.bands.slice(1), -1] }] }),
      msg({ blocks: [null] }),
      msg({ type: "something-new" }),
    ];
    for (const m of bad) expect(projectedTopic.parse(m)).toBeUndefined();
  });
});
