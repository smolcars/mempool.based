import { describe, expect, it } from "vitest";
import { mempoolTopic, projectedTopic, transactionsTopic } from "./mempool-topics";

const bands = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
const block = {
  txs: 3,
  weight: 4000,
  vsize: 1000,
  fees: 900,
  min_rate: 100,
  median_rate: 1234,
  max_rate: 250_000,
  bands,
};
const hash = "ab".repeat(32);

describe("the mempool topics", () => {
  it("read the mempool totals, rates in sat/vB", () => {
    const m = {
      type: "mempool",
      seq: 1,
      count: 5,
      vsize: 10,
      fees: 20,
      min_fee_rate: 1000,
      bands,
      stale: false,
    };
    expect(mempoolTopic.parse(m)).toStrictEqual({
      type: "mempool",
      count: 5,
      vsize: 10,
      fees: 20,
      minFeeRate: 1,
      bands,
      stale: false,
    });
    expect(mempoolTopic.parse({ ...m, min_fee_rate: null })).toMatchObject({ minFeeRate: null });
    expect(mempoolTopic.parse({ ...m, bands: [1, 2] })).toBeUndefined();
    expect(mempoolTopic.parse({ ...m, count: -1 })).toBeUndefined();
    expect(mempoolTopic.parse({ ...m, stale: "no" })).toBeUndefined();
  });

  it("read a projection, its tip null before the server has reached the node", () => {
    const m = {
      type: "projected",
      seq: 7,
      tip: { height: 9, hash },
      stale: false,
      blocks: [block],
    };
    expect(projectedTopic.parse(m)).toStrictEqual({
      type: "projected",
      seq: 7,
      tip: { height: 9, hash },
      stale: false,
      blocks: [
        {
          txs: 3,
          weight: 4000,
          vsize: 1000,
          fees: 900,
          minRate: 0.1,
          medianRate: 1.234,
          maxRate: 250,
          bands,
        },
      ],
    });
    expect(projectedTopic.parse({ ...m, tip: null })).toMatchObject({ tip: null });
    expect(projectedTopic.parse({ ...m, tip: { height: 9, hash: "<b>" } })).toBeUndefined();
    expect(
      projectedTopic.parse({ ...m, blocks: [{ ...block, median_rate: 1.5 }] }),
    ).toBeUndefined();
    expect(projectedTopic.parse({ ...m, blocks: "none" })).toBeUndefined();
  });

  it("read the newest transactions and refuse a malformed one", () => {
    const tx = { txid: "c".repeat(64), time: 1, fee: 2, vsize: 3, value: 4 };
    expect(transactionsTopic.parse({ type: "transactions", txs: [tx] })).toStrictEqual({
      type: "transactions",
      txs: [tx],
    });
    expect(
      transactionsTopic.parse({ type: "transactions", txs: [{ ...tx, txid: "x" }] }),
    ).toBeUndefined();
  });

  it("read off, and ignore types they do not know", () => {
    for (const topic of [mempoolTopic, projectedTopic, transactionsTopic]) {
      expect(topic.parse({ type: "off" })).toStrictEqual({ type: "off" });
      expect(topic.parse({ type: "later" })).toBeUndefined();
    }
  });
});
