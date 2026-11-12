// What the search field loads on first use: what a query looks like as it is typed, and where it
// leads on Enter. Kept out of the first load (components/SearchField.tsx imports it lazily).
import { TYPE_NAMES } from "../lib/address";
import { fetchApi, isNotFound } from "../lib/api";
import { groupDigits } from "../lib/format";
import { NETWORK_NAMES, type Network, paths } from "../lib/network";
import { classify, type HashKind, MAX_QUERY, type Query } from "./query";

export type { Query } from "./query";

/** The field's reading of a query as it is typed: a short label, and whether Enter will act. */
export interface Reading {
  label: string;
  ready: boolean;
}

/** Names networks for a sentence: "Signet or Testnet4". */
export function networkNames(list: readonly Network[]): string {
  const all = list.map((n) => NETWORK_NAMES[n]);
  return all.length > 1 ? `${all.slice(0, -1).join(", ")} or ${all.at(-1)}` : (all[0] ?? "");
}

const HASH_LABELS: Record<HashKind, string> = { block: "Block hash", tx: "Transaction" };

export function describe(raw: string, network: Network): Reading {
  const q = classify(raw);
  if (q?.kind === "height") return { label: `Block ${groupDigits(q.height)}`, ready: true };
  if (q?.kind === "hash") {
    return { label: q.as ? HASH_LABELS[q.as] : "Transaction or block", ready: true };
  }
  if (q?.kind === "address") {
    const { type, networks } = q.address;
    const theirs = networks.includes(network) ? null : networkNames(networks);
    return { label: `${theirs ?? TYPE_NAMES[type].short} address`, ready: true };
  }
  if (raw.length > MAX_QUERY) return { label: "Too long", ready: false };
  const hex = /^(?:0x)?([0-9a-f]{1,63})$/i.exec(raw.trim())?.[1];
  if (hex !== undefined) return { label: `${hex.length} of 64 characters`, ready: false };
  return { label: "Not recognised", ready: false };
}

export type Outcome =
  | { kind: "go"; path: string }
  | { kind: "missing"; query: Query | null }
  | { kind: "failed"; error: unknown };

/** Asks the network's API for a path; rejects with ApiError (404 is "no such thing"). */
export type Probe = (path: string) => Promise<unknown>;

/**
 * Which kind of thing to ask for first. A link or an outpoint says; otherwise a block hash meets
 * proof of work, so five leading zero digits (2^-20 for a txid) mean a block, and anything else is
 * most likely a transaction, by far the most searched for.
 */
export function probeOrder(hash: string, as: HashKind | null): [HashKind, HashKind] {
  const first = as ?? (hash.startsWith("00000") ? "block" : "tx");
  return first === "block" ? ["block", "tx"] : ["tx", "block"];
}

/**
 * Where a query leads: a page path (network-free), nothing, or a failure. Heights and addresses
 * need no request: the block page says when a height is not mined yet, and the address page
 * offers the right network for another network's address. A 64-hex id is asked for as the likelier
 * kind, then the other: one request when the guess is right, two when it is wrong or nothing
 * matches. The block probe is the block page's own first request (cached in memory, immutable);
 * the transaction probe is its small status, not a reply that can be megabytes.
 */
export async function resolve(query: Query | null, probe: Probe): Promise<Outcome> {
  if (query === null) return { kind: "missing", query };
  if (query.kind === "height") return { kind: "go", path: paths.block(query.height) };
  if (query.kind === "address") return { kind: "go", path: paths.address(query.address.address) };
  const { hash } = query;
  for (const kind of probeOrder(hash, query.as)) {
    try {
      // oxlint-disable-next-line no-await-in-loop -- the second request is only made when the first finds nothing
      await probe(kind === "block" ? `/block/${hash}` : `/tx/${hash}/status`);
      return { kind: "go", path: kind === "block" ? paths.block(hash) : paths.tx(hash) };
    } catch (error) {
      if (!isNotFound(error)) throw error;
    }
  }
  return { kind: "missing", query };
}

export type Get = (network: Network, path: string, signal: AbortSignal) => Promise<unknown>;

const get: Get = (network, path, signal) => fetchApi(network, path, { signal });

/**
 * One search at a time: starting a search, or cancelling, aborts the one in flight, whose `run`
 * then answers undefined so a slow answer can never replace a newer one.
 */
export function createSearch(fetch: Get = get) {
  let current: AbortController | undefined;
  const cancel = () => {
    current?.abort();
    current = undefined;
  };
  const run = async (raw: string, network: Network): Promise<Outcome | undefined> => {
    cancel();
    const controller = new AbortController();
    current = controller;
    const { signal } = controller;
    let outcome: Outcome;
    try {
      outcome = await resolve(classify(raw), (path) => fetch(network, path, signal));
    } catch (error) {
      outcome = { kind: "failed", error };
    }
    if (signal.aborted) return undefined;
    current = undefined;
    return outcome;
  };
  return { run, cancel };
}

/** What the field gets from this module once it loads. */
export interface Logic {
  describe: typeof describe;
  createSearch: typeof createSearch;
}
