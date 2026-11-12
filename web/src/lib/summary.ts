// A mined block's fee facts from GET /app/block(s)/…/summary (docs/FRONTEND.md "Block summaries").
import { fetchApp } from "./api";
import type { Hash } from "./esplora";
import type { Network } from "./network";

/** Fee rates in sat/vB, unrounded; percentiles are weighted by virtual size. */
export interface FeeRates {
  min: number;
  p10: number;
  p25: number;
  median: number;
  p75: number;
  p90: number;
  max: number;
}

export interface BlockSummary {
  id: Hash;
  height: number;
  tx_count: number;
  /** sats, the coinbase left out, as are `vsize` and `weight`. */
  fees: number;
  vsize: number;
  weight: number;
  /** Null for a block holding only its coinbase. */
  fee_rate: FeeRates | null;
  /** Virtual size paying in each of the ten fee bands, cheapest first: the strata. */
  fee_bands: number[];
}

/** The batch route takes at most this many hashes. */
export const SUMMARY_BATCH = 10;

const RATE_KEYS = ["min", "p10", "p25", "median", "p75", "p90", "max"] as const;
const isCount = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v) && v >= 0;

/** Whether a reply (or a WebSocket message's `summary`) is a summary this app can draw. */
export function isSummary(v: unknown): v is BlockSummary {
  if (typeof v !== "object" || v === null) return false;
  const s = v as Record<string, unknown>;
  const rates = s["fee_rate"];
  const bands = s["fee_bands"];
  return (
    typeof s["id"] === "string" &&
    /^[0-9a-f]{64}$/.test(s["id"]) &&
    Number.isSafeInteger(s["height"]) &&
    ["tx_count", "fees", "vsize", "weight"].every((k) => isCount(s[k])) &&
    (rates === null ||
      (typeof rates === "object" &&
        rates !== null &&
        RATE_KEYS.every((k) => isCount((rates as Record<string, unknown>)[k])))) &&
    Array.isArray(bands) &&
    bands.length === 10 &&
    bands.every((b) => isCount(b))
  );
}

// shortcut: insertion-ordered, capped like api.ts's cache; summaries never change, so nothing goes stale.
const KNOWN_MAX = 512;
const known = new Map<string, BlockSummary>();
const key = (network: Network, hash: string) => `${network}:${hash}`;

/** A summary this page already has: fetched, or pushed over the WebSocket. */
export function knownSummary(network: Network, hash: string): BlockSummary | undefined {
  return known.get(key(network, hash));
}

/** Keeps a summary for the life of the page. */
export function rememberSummary(network: Network, summary: BlockSummary): void {
  if (known.size >= KNOWN_MAX) {
    const oldest = known.keys().next();
    if (!oldest.done) known.delete(oldest.value);
  }
  known.set(key(network, summary.id), summary);
}

/** Forgets every summary. For tests. */
export function clearSummaries(): void {
  known.clear();
}

/**
 * The summaries of up to ten blocks in one request, retrying when the server says it is busy (503)
 * or this client asked for too much (429), as long as it says to wait. Summaries already known are
 * not asked for again. Resolves with every summary it has, keyed by hash.
 */
export async function fetchSummaries(
  network: Network,
  hashes: readonly string[],
  signal?: AbortSignal,
): Promise<Map<string, BlockSummary>> {
  const missing = [...new Set(hashes)].filter((h) => !known.has(key(network, h)));
  for (let i = 0; i < missing.length; i += SUMMARY_BATCH) {
    const part = missing.slice(i, i + SUMMARY_BATCH);
    const path = `/blocks/summary?hashes=${part.map((h) => encodeURIComponent(h)).join(",")}`;
    // oxlint-disable-next-line no-await-in-loop -- one batch at a time, so a page never floods the server
    const reply = await fetchApp<unknown>(network, path, signal ? { signal } : {});
    if (Array.isArray(reply)) {
      for (const s of reply) if (isSummary(s)) rememberSummary(network, s);
    }
  }
  const found = new Map<string, BlockSummary>();
  for (const h of hashes) {
    const s = knownSummary(network, h);
    if (s) found.set(h, s);
  }
  return found;
}
