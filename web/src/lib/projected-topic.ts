// The `projected` topic (docs/WS.md "Mempool topics"): every projected block's totals, the tip they
// sit on and whether the node is answering. Each message is the whole state.
import type { ChainTip } from "./blocks-topic";
import type { Topic } from "./live";
import { FEE_BANDS } from "./format";

/** One projected block's totals. Rates are sat/vB here (the wire's sat per 1,000 vB ÷ 1,000). */
export interface ProjectedBlock {
  txs: number;
  weight: number;
  vsize: number;
  fees: number;
  minRate: number;
  medianRate: number;
  maxRate: number;
  /** vsize in each of the ten fee bands, cheapest first: the strata. */
  bands: number[];
}

export interface Projection {
  seq: number;
  /** The block the projection sits on, or null before the server has one. */
  tip: ChainTip | null;
  /** The server has lost touch with its node: these are the last good figures. */
  stale: boolean;
  /** The next block first; the last holds everything beyond the others. */
  blocks: ProjectedBlock[];
}

export type ProjectedMessage = { type: "projected"; projection: Projection } | { type: "off" };

const isCount = (v: unknown): v is number => Number.isSafeInteger(v) && (v as number) >= 0;

function tipOf(v: unknown): ChainTip | null | undefined {
  if (v === null) return null;
  if (typeof v !== "object") return undefined;
  const t = v as Record<string, unknown>;
  const hash = t["hash"];
  return isCount(t["height"]) && typeof hash === "string" && /^[0-9a-f]{64}$/.test(hash)
    ? { height: t["height"], hash }
    : undefined;
}

function blockOf(v: unknown): ProjectedBlock | undefined {
  if (typeof v !== "object" || v === null) return undefined;
  const b = v as Record<string, unknown>;
  const bands = b["bands"];
  const fields = ["txs", "weight", "vsize", "fees", "min_rate", "median_rate", "max_rate"];
  if (!fields.every((f) => isCount(b[f]))) return undefined;
  if (
    !Array.isArray(bands) ||
    bands.length !== FEE_BANDS ||
    !bands.every((x): x is number => isCount(x))
  )
    return undefined;
  const n = (f: string) => b[f] as number;
  return {
    txs: n("txs"),
    weight: n("weight"),
    vsize: n("vsize"),
    fees: n("fees"),
    minRate: n("min_rate") / 1000,
    medianRate: n("median_rate") / 1000,
    maxRate: n("max_rate") / 1000,
    bands,
  };
}

export const projectedTopic: Topic<ProjectedMessage> = {
  name: "projected",
  parse(m) {
    if (m["type"] === "off") return { type: "off" };
    if (m["type"] !== "projected" || !isCount(m["seq"]) || typeof m["stale"] !== "boolean") {
      return undefined;
    }
    const tip = tipOf(m["tip"]);
    const raw = m["blocks"];
    if (tip === undefined || !Array.isArray(raw) || raw.length === 0 || raw.length > 64) {
      return undefined;
    }
    const blocks = raw.map((b) => blockOf(b));
    if (!blocks.every((b): b is ProjectedBlock => b !== undefined)) return undefined;
    return { type: "projected", projection: { seq: m["seq"], tip, stale: m["stale"], blocks } };
  },
};
