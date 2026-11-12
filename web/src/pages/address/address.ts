// What the address page works out for itself: an address's type and network from its form, its
// net change in a transaction, and which of the node's refusals an API error is. Pure functions.
import { ApiError } from "../../lib/api";
import type { Transaction, Utxo } from "../../lib/esplora";
import { feeRate, vsizeFromWeight } from "../../lib/format";
import type { Network } from "../../lib/network";

export type AddressType = "p2pkh" | "p2sh" | "p2wpkh" | "p2wsh" | "p2tr" | "p2a" | "witness";

export interface Recognised {
  /** The form the API and the node use: bech32 in lower case, base58 as given. */
  address: string;
  type: AddressType;
  /** The networks an address of this form belongs to. Signet and testnet4 share their forms. */
  networks: readonly Network[];
}

export const TYPE_NAMES: Record<AddressType, { short: string; long: string }> = {
  p2pkh: { short: "P2PKH", long: "Pay to public key hash" },
  p2sh: { short: "P2SH", long: "Pay to script hash" },
  p2wpkh: { short: "P2WPKH", long: "Pay to witness public key hash (SegWit v0)" },
  p2wsh: { short: "P2WSH", long: "Pay to witness script hash (SegWit v0)" },
  p2tr: { short: "P2TR", long: "Pay to Taproot (SegWit v1)" },
  p2a: { short: "P2A", long: "Pay to anchor" },
  witness: { short: "SegWit", long: "A SegWit version not yet in use" },
};

const TEST_NETWORKS: readonly Network[] = ["signet", "testnet4", "regtest"];
const BECH32_NETWORKS: Record<string, readonly Network[]> = {
  bc: ["mainnet"],
  tb: ["signet", "testnet4"],
  bcrt: ["regtest"],
};
const BECH32_CHARSET = "qpzry9x8gf2tvdw0s3jn54khce6mua7l";

/**
 * Recognises an address's type and networks from its form alone, or null when it has no address's
 * form. The checksum is left to the server, which validates every address it is asked for.
 */
export function recognise(input: string): Recognised | null {
  const base58 = /^[1-9A-HJ-NP-Za-km-z]{25,35}$/.test(input);
  if (base58) {
    const first = input[0];
    if (first === "1") return { address: input, type: "p2pkh", networks: ["mainnet"] };
    if (first === "3") return { address: input, type: "p2sh", networks: ["mainnet"] };
    if (first === "m" || first === "n") {
      return { address: input, type: "p2pkh", networks: TEST_NETWORKS };
    }
    if (first === "2") return { address: input, type: "p2sh", networks: TEST_NETWORKS };
    return null;
  }
  // Bech32 is all lower or all upper case, at most 90 characters.
  const lower = input.toLowerCase();
  if (input.length > 90 || (input !== lower && input !== input.toUpperCase())) return null;
  const m = /^(bc|tb|bcrt)1([qpzry9x8gf2tvdw0s3jn54khce6mua7l]{7,})$/.exec(lower);
  const hrp = m?.[1];
  const data = m?.[2];
  if (!hrp || !data) return null;
  const networks = BECH32_NETWORKS[hrp] ?? [];
  const version = BECH32_CHARSET.indexOf(data[0] ?? "");
  // Witness version, then the program in 5-bit groups, then a 6-character checksum.
  const bytes = Math.floor(((data.length - 7) * 5) / 8);
  const found = (type: AddressType): Recognised => ({ address: lower, type, networks });
  if (version === 0) {
    if (bytes === 20) return found("p2wpkh");
    if (bytes === 32) return found("p2wsh");
    return null;
  }
  if (version === 1 && bytes === 32) return found("p2tr");
  // Pay to anchor is the 2-byte v1 program 0x4e73, which always encodes as "pfees".
  if (version === 1 && bytes === 2 && data.startsWith("pfees")) return found("p2a");
  if (version >= 1 && version <= 16 && bytes >= 2 && bytes <= 40) return found("witness");
  return null;
}

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
