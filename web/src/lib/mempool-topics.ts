// The `mempool` and `transactions` topics (docs/WS.md "Mempool topics"); `projected` is in
// projected-topic.ts. Each message is the whole state, so a page keeps the last one. Rates arrive
// in sat per 1,000 vB and are turned into sat/vB here, so pages print them like every other rate.
import type { RecentTx } from "./esplora";
import type { Topic } from "./live";

/** Sent once instead of any state when the server does not track this network's mempool. */
export interface Off {
  type: "off";
}

export interface MempoolState {
  type: "mempool";
  count: number;
  vsize: number;
  fees: number;
  /** The lowest rate the node accepts now, sat/vB; null before its first answer. */
  minFeeRate: number | null;
  /** Virtual size in each of the ten fee bands, cheapest first. */
  bands: number[];
  /** The server has lost the node: these are the last good figures. */
  stale: boolean;
}

export interface SeenTxs {
  type: "transactions";
  /** Newest first. */
  txs: (RecentTx & { time: number })[];
}

const isCount = (v: unknown): v is number => Number.isSafeInteger(v) && (v as number) >= 0;
const isHash = (v: unknown): v is string => typeof v === "string" && /^[0-9a-f]{64}$/.test(v);
const isBands = (v: unknown): v is number[] =>
  Array.isArray(v) && v.length === 10 && v.every((x) => isCount(x));
const rate = (kvb: number) => kvb / 1000;

export const mempoolTopic: Topic<MempoolState | Off> = {
  name: "mempool",
  parse(m) {
    if (m["type"] === "off") return { type: "off" };
    const { count, vsize, fees, min_fee_rate: min, bands, stale } = m;
    if (
      m["type"] !== "mempool" ||
      !isCount(count) ||
      !isCount(vsize) ||
      !isCount(fees) ||
      !(min === null || isCount(min)) ||
      !isBands(bands) ||
      typeof stale !== "boolean"
    ) {
      return undefined;
    }
    return {
      type: "mempool",
      count,
      vsize,
      fees,
      minFeeRate: min === null ? null : rate(min),
      bands,
      stale,
    };
  },
};

export const transactionsTopic: Topic<SeenTxs | Off> = {
  name: "transactions",
  parse(m) {
    if (m["type"] === "off") return { type: "off" };
    const raw = m["txs"];
    if (m["type"] !== "transactions" || !Array.isArray(raw)) return undefined;
    const txs: SeenTxs["txs"] = [];
    for (const v of raw) {
      if (typeof v !== "object" || v === null) return undefined;
      const { txid, time, fee, vsize, value } = v as Record<string, unknown>;
      if (!isHash(txid) || !isCount(time) || !isCount(fee) || !isCount(vsize) || !isCount(value)) {
        return undefined;
      }
      txs.push({ txid, time, fee, vsize, value });
    }
    return { type: "transactions", txs };
  },
};
