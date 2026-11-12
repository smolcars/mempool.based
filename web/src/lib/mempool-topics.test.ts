import { describe, expect, it } from "vitest";
import { mempoolTopic, transactionsTopic } from "./mempool-topics";

const bands = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];

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
    for (const topic of [mempoolTopic, transactionsTopic]) {
      expect(topic.parse({ type: "off" })).toStrictEqual({ type: "off" });
      expect(topic.parse({ type: "later" })).toBeUndefined();
    }
  });
});
