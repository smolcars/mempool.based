// What the home page computes from the mempool summary, and where the fee ladder puts its pins.
import type { MempoolSummary, RecentTx } from "../../lib/esplora";
import { FEE_BOUNDS, feeBand } from "../../lib/format";
import type { MempoolState } from "../../lib/mempool-topics";

const isNum = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v) && v >= 0;

export function isMempoolSummary(v: unknown): v is MempoolSummary {
  if (typeof v !== "object" || v === null) return false;
  const m = v as Record<string, unknown>;
  return (
    isNum(m["count"]) &&
    isNum(m["vsize"]) &&
    isNum(m["total_fee"]) &&
    Array.isArray(m["fee_histogram"]) &&
    m["fee_histogram"].every((p) => Array.isArray(p) && isNum(p[0]) && isNum(p[1]))
  );
}

export function isRecentList(v: unknown): v is RecentTx[] {
  return (
    Array.isArray(v) &&
    v.every((t) => {
      if (typeof t !== "object" || t === null) return false;
      const r = t as Record<string, unknown>;
      return (
        typeof r["txid"] === "string" &&
        /^[0-9a-f]{64}$/.test(r["txid"]) &&
        isNum(r["fee"]) &&
        isNum(r["vsize"])
      );
    })
  );
}

/** One of the ten fee bands of the mempool bar. */
export interface FeeBandShare {
  /** 1–10, as `feeBand` numbers them. */
  band: number;
  /** Lower bound in sat/vB; band 1 starts at 0. */
  from: number;
  /** Upper bound, or null for the top band. */
  to: number | null;
  vsize: number;
  /** Where the band starts on the bar and how much of it it takes, both 0–1. */
  x: number;
  share: number;
}

/**
 * The mempool's fee histogram on the design's ten bands. Each histogram entry is the virtual size
 * of the transactions from its rate up to the previous entry's, so it goes to the band of its
 * lower bound, banded as printed. An entry that straddles a band boundary lands wholly below it.
 */
export function bandShares(histogram: readonly (readonly [number, number])[]): FeeBandShare[] {
  const vsize = Array.from({ length: 10 }, () => 0);
  for (const [rate, size] of histogram) {
    const i = feeBand(rate) - 1;
    vsize[i] = (vsize[i] ?? 0) + size;
  }
  return sharesOf(vsize);
}

/** The bar from the virtual size in each of the ten bands, cheapest first. */
export function sharesOf(vsize: readonly number[]): FeeBandShare[] {
  const total = vsize.reduce((a, b) => a + b, 0);
  let x = 0;
  return Array.from({ length: 10 }, (_, i) => {
    const v = vsize[i] ?? 0;
    const share = total > 0 ? v / total : 0;
    const band = {
      band: i + 1,
      from: i === 0 ? 0 : (FEE_BOUNDS[i - 1] ?? 0),
      to: FEE_BOUNDS[i] ?? null,
      vsize: v,
      x,
      share,
    };
    x += share;
    return band;
  });
}

/** The cheapest fee rate waiting: the last histogram entry's lower bound. */
export function lowestRate(histogram: readonly (readonly [number, number])[]): number | undefined {
  return histogram.length > 0 ? histogram.at(-1)?.[0] : undefined;
}

/** What the mempool panel shows, from the `mempool` topic or the polled `/mempool`. */
export interface MempoolFigures {
  count: number;
  vsize: number;
  fees: number;
  /** The lowest rate shown, sat/vB, and what it is. */
  lowest: number | undefined;
  lowestNote: string;
  bands: FeeBandShare[];
}

export function figuresFromSummary(s: MempoolSummary): MempoolFigures {
  return {
    count: s.count,
    vsize: s.vsize,
    fees: s.total_fee,
    lowest: lowestRate(s.fee_histogram),
    lowestNote: "cheapest waiting",
    bands: bandShares(s.fee_histogram),
  };
}

export function figuresFromTopic(m: MempoolState): MempoolFigures {
  return {
    count: m.count,
    vsize: m.vsize,
    fees: m.fees,
    lowest: m.minFeeRate ?? undefined,
    lowestNote: "node minimum",
    bands: sharesOf(m.bands),
  };
}

/**
 * Paces new rows into a list of the newest transactions, newest first, so each can be read on a
 * busy day: the queue holds the ones not shown yet, oldest first, at most `max` of them (older
 * ones would be pushed out of the list at once anyway). Returns the new queue.
 */
export function enqueueTxs<T extends { txid: string }>(
  shown: readonly T[],
  queue: readonly T[],
  incoming: readonly T[],
  max: number,
): T[] {
  const known = new Set([...shown, ...queue].map((t) => t.txid));
  // oxlint-disable-next-line unicorn/no-array-reverse -- reverses a fresh array; toReversed is newer than the browsers we build for
  const fresh = incoming.filter((t) => !known.has(t.txid)).reverse();
  return [...queue, ...fresh].slice(-max);
}
