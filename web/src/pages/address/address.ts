// What the address page works out for itself: an address's net change in a transaction, and which
// of the node's refusals an API error is. Pure functions. Recognising an address is in lib/address.
import { ApiError } from "../../lib/api";
import type { Transaction, Utxo } from "../../lib/esplora";
import { feeRate, vsizeFromWeight } from "../../lib/format";

/**
 * The address's net change in a transaction, in sats: what its outputs pay to the address minus
 * what its inputs spend from it. Null when the node has pruned the inputs, so the spent side is
 * unknown.
 */
export function netChange(tx: Transaction, address: string): number | null {
  if (tx.pruned) return null;
  let sats = 0;
  for (const out of tx.vout) if (out.scriptpubkey_address === address) sats += out.value;
  for (const input of tx.vin ?? []) {
    const prev = input.prevout;
    if (prev?.scriptpubkey_address === address) sats -= prev.value;
  }
  return sats;
}

/** A transaction's fee rate in sat/vB, or null for a coinbase or when the node left the fee out. */
export function txFeeRate(tx: Transaction): number | null {
  if (tx.vin?.[0]?.is_coinbase) return null;
  if (tx.fee === undefined || tx.weight === undefined) return null;
  return feeRate(tx.fee, vsizeFromWeight(tx.weight));
}

export const isCoinbase = (tx: Transaction): boolean => tx.vin?.[0]?.is_coinbase === true;

/**
 * Which refusal an API error is. The server's messages are fixed (docs/API.md, "Error bodies are
 * ours"), so they are matched as text.
 */
export type Refusal = "too-large" | "no-index" | "wrong-network" | "invalid" | "failed";

export function refusal(error: unknown): Refusal {
  if (!(error instanceof ApiError)) return "failed";
  if (error.status === 503 && /too large/i.test(error.message)) return "too-large";
  if (error.status === 503 && /index is disabled/i.test(error.message)) return "no-index";
  if (error.status === 400 && /another network/i.test(error.message)) return "wrong-network";
  if (error.status === 400) return "invalid";
  return "failed";
}

/**
 * Unspent outputs in the order a reader wants them: unconfirmed first, then the newest, then the
 * largest. Returns a new array.
 */
export function sortUtxos(utxos: readonly Utxo[]): Utxo[] {
  const height = (u: Utxo) => (u.status.confirmed ? (u.status.block_height ?? 0) : Infinity);
  // oxlint-disable-next-line unicorn/no-array-sort -- sorts a fresh copy; toSorted is newer than the browsers we build for
  return [...utxos].sort((a, b) => height(b) - height(a) || b.value - a.value);
}

/** Confirmations of an output at `height` with the tip at `tip`, or 0 when unconfirmed. */
export function confirmations(utxo: Utxo, tip: number): number {
  const h = utxo.status.block_height;
  return utxo.status.confirmed && h !== undefined && tip >= h ? tip - h + 1 : 0;
}

/**
 * The history on screen: a fresh first page, then the pages loaded after it without the
 * transactions the fresh page already holds (new ones push older ones off the first page).
 */
export function mergePages(
  first: readonly Transaction[],
  later: readonly Transaction[],
): Transaction[] {
  const ids = new Set(first.map((t) => t.txid));
  return [...first, ...later.filter((t) => !ids.has(t.txid))];
}

/**
 * Whether history goes on after this page. A page holds the unconfirmed transactions (first page
 * only) and then up to `size` confirmed ones; a full set of confirmed ones means there may be more.
 */
export function pageIsFull(page: readonly Transaction[], size: number): boolean {
  return page.filter((t) => t.status.confirmed).length >= size;
}

/**
 * The loaded history to keep after a refresh brings a new first page: what was shown after the
 * fresh page's last confirmed transaction. Nothing when the fresh page is the whole history (a
 * short page) or when it no longer meets what was shown (more arrived than a page holds), so
 * paging starts again from the fresh page rather than skipping or keeping stale rows.
 */
export function keptAfterRefresh(
  fresh: readonly Transaction[],
  shown: readonly Transaction[],
  size: number,
): Transaction[] {
  if (!pageIsFull(fresh, size)) return [];
  // oxlint-disable-next-line unicorn/prefer-array-find -- findLast is newer than the browsers we build for
  const edge = fresh.filter((t) => t.status.confirmed).at(-1)?.txid;
  const i = shown.findIndex((t) => t.txid === edge);
  return i === -1 ? [] : shown.slice(i + 1).filter((t) => t.status.confirmed);
}
