// The mempool topics (docs/WS.md "Mempool topics"): `mempool`, `projected` and `transactions`.
// Each message is the whole state, so a page keeps the last one. Rates arrive in sat per 1,000 vB
// and are turned into sat/vB here, so pages print them like every other rate.
import type { ChainTip } from "./blocks-topic";
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

export interface ProjectedBlock {
  txs: number;
  weight: number;
  vsize: number;
  fees: number;
  /** Effective rates, sat/vB: lowest, at the middle vbyte, highest. 0 in an empty block. */
  minRate: number;
  medianRate: number;
  maxRate: number;
  bands: number[];
}

export interface Projection {
  type: "projected";
  seq: number;
  /** The block they were projected on; null before the server has reached the node. */
  tip: ChainTip | null;
  stale: boolean;
  /** The next block first; the last holds everything beyond the others. */
  blocks: ProjectedBlock[];
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

function tipOf(v: unknown): ChainTip | null | undefined {
  if (v === null) return null;
  if (typeof v !== "object") return undefined;
  const t = v as Record<string, unknown>;
  return isCount(t["height"]) && isHash(t["hash"])
    ? { height: t["height"], hash: t["hash"] }
    : undefined;
}

function blockOf(v: unknown): ProjectedBlock | undefined {
  if (typeof v !== "object" || v === null) return undefined;
  const b = v as Record<string, unknown>;
  const { txs, weight, vsize, fees, min_rate: min, median_rate: median, max_rate: max } = b;
  if (
    !isCount(txs) ||
    !isCount(weight) ||
    !isCount(vsize) ||
    !isCount(fees) ||
    !isCount(min) ||
    !isCount(median) ||
    !isCount(max) ||
    !isBands(b["bands"])
  ) {
    return undefined;
  }
  return {
    txs,
    weight,
    vsize,
    fees,
    minRate: rate(min),
    medianRate: rate(median),
    maxRate: rate(max),
    bands: b["bands"],
  };
}

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

export const projectedTopic: Topic<Projection | Off> = {
  name: "projected",
  parse(m) {
    if (m["type"] === "off") return { type: "off" };
    const tip = tipOf(m["tip"]);
    const raw = m["blocks"];
    if (
      m["type"] !== "projected" ||
      !isCount(m["seq"]) ||
      tip === undefined ||
      typeof m["stale"] !== "boolean" ||
      !Array.isArray(raw)
    ) {
      return undefined;
    }
    const blocks = raw.map((b) => blockOf(b));
    if (!blocks.every((b) => b !== undefined)) return undefined;
    return { type: "projected", seq: m["seq"], tip, stale: m["stale"], blocks };
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
