// A block's audit: the block we expected against the one mined (docs/FRONTEND.md "Block audits").
// Pure: types, readers that never throw, and the words for each category.

/** One side's totals: the block we expected, or the block mined without its coinbase. */
export interface AuditSide {
  txs: number;
  vsize: number;
  weight: number;
  /** sats */
  fees: number;
}

export interface AuditPart {
  txs: number;
  vsize: number;
}

export const KINDS = [
  "matched",
  "displaced",
  "fresh",
  "conflicted",
  "missing",
  "added",
  "unseen",
] as const;
export type Kind = (typeof KINDS)[number];
/** The kinds the detail route lists: every one but `matched`. */
export type ListedKind = Exclude<Kind, "matched">;

export type AuditTotals = {
  /** Percent, two decimals, rounded down by the server. Print as is. */
  match_rate: number;
  expected: AuditSide;
  actual: AuditSide;
} & Record<Kind, AuditPart>;

export interface AuditTx {
  txid: string;
  kind: ListedKind;
  vsize: number;
  /** sat per 1,000 vB. */
  rate: number;
  /** Unix seconds our node accepted it; only for expected kinds still in the mempool after the block. */
  first_seen?: number;
}

export interface AuditDetail {
  totals: AuditTotals;
  /** Unix seconds the server learnt of the block. */
  seen_at: number;
  /** False when the server cut the list at 1,000. */
  complete: boolean;
  txs: AuditTx[];
}

/** The most transactions the detail route lists. */
export const LIST_CAP = 1000;

/** Each category in plain words, in the order the section shows them. */
export const CATEGORIES: readonly { kind: Kind; name: string; says: string }[] = [
  { kind: "matched", name: "Matched", says: "Expected, and mined." },
  {
    kind: "displaced",
    name: "Displaced",
    says: "Expected, but the block was full and gave its room to transactions we did not expect that paid at least as much.",
  },
  {
    kind: "fresh",
    name: "Fresh",
    says: "Expected, but our node first saw it less than a minute before the block: too new to expect a miner to have it.",
  },
  {
    kind: "conflicted",
    name: "Conflicted",
    says: "Expected, but the block spent one of its coins in another transaction (or, rarely, it was replaced or dropped at that moment).",
  },
  {
    kind: "missing",
    name: "Missing",
    says: "Expected, and left out for none of the reasons above. Only these lower the match rate.",
  },
  { kind: "added", name: "Added", says: "Mined, though we expected it in a later block." },
  {
    kind: "unseen",
    name: "Unseen",
    says: "Mined, but not in any block we projected: new to us, or it arrived a moment before the block.",
  },
];

export const categoryName = (kind: Kind): string =>
  CATEGORIES.find((c) => c.kind === kind)?.name ?? kind;

const isCount = (v: unknown): v is number => Number.isSafeInteger(v) && (v as number) >= 0;
const record = (v: unknown): Record<string, unknown> | undefined =>
  typeof v === "object" && v !== null && !Array.isArray(v)
    ? (v as Record<string, unknown>)
    : undefined;

function readPart(v: unknown): AuditPart | undefined {
  const p = record(v);
  return p && isCount(p["txs"]) && isCount(p["vsize"])
    ? { txs: p["txs"], vsize: p["vsize"] }
    : undefined;
}

function readSide(v: unknown): AuditSide | undefined {
  const s = record(v);
  const part = readPart(v);
  return s && part && isCount(s["weight"]) && isCount(s["fees"])
    ? { ...part, weight: s["weight"], fees: s["fees"] }
    : undefined;
}

/**
 * An audit's totals from a summary's `audit` or the detail route, or null when there is none:
 * `null`, left out by an older server, or not a shape this page can draw.
 */
export function readAudit(v: unknown): AuditTotals | null {
  const a = record(v);
  const rate = a?.["match_rate"];
  if (!a || typeof rate !== "number" || !(rate >= 0 && rate <= 100)) return null;
  const expected = readSide(a["expected"]);
  const actual = readSide(a["actual"]);
  if (!expected || !actual) return null;
  const parts: Partial<Record<Kind, AuditPart>> = {};
  for (const k of KINDS) {
    const p = readPart(a[k]);
    if (!p) return null;
    parts[k] = p;
  }
  return { match_rate: rate, expected, actual, ...(parts as Record<Kind, AuditPart>) };
}

const LISTED = new Set<string>(KINDS.filter((k) => k !== "matched"));

function readTx(v: unknown): AuditTx | undefined {
  const t = record(v);
  if (!t) return undefined;
  const { txid, kind, vsize, rate, first_seen: seen } = t;
  if (typeof txid !== "string" || !/^[0-9a-f]{64}$/.test(txid)) return undefined;
  if (typeof kind !== "string" || !LISTED.has(kind) || !isCount(vsize) || !isCount(rate)) {
    return undefined;
  }
  const tx: AuditTx = { txid, kind: kind as ListedKind, vsize, rate };
  if (isCount(seen)) tx.first_seen = seen;
  return tx;
}

/** The detail route's answer, or undefined when it is not one. Entries it cannot read are dropped. */
export function readAuditDetail(v: unknown): AuditDetail | undefined {
  const totals = readAudit(v);
  const a = record(v);
  if (!totals || !a || !Array.isArray(a["txs"]) || !isCount(a["seen_at"])) return undefined;
  const txs = a["txs"].map(readTx).filter((t): t is AuditTx => t !== undefined);
  return { totals, seen_at: a["seen_at"], complete: a["complete"] !== false, txs };
}

/** The score as the server sent it, two decimals, never rounded up: `99.87`, `100`. */
export const formatMatchRate = (rate: number): string =>
  Number.isInteger(rate) ? String(rate) : (Math.floor(rate * 100 + 1e-9) / 100).toFixed(2);
