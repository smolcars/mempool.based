// What `/app/address/:address/ledger` says (docs/API.md, "What the page shows of such an address"):
// the server's own running totals of a very large address, or where adding it up stands. Parsed
// strictly; anything unexpected reads as nothing known.

export interface LedgerTx {
  txid: string;
  height: number;
  /** In the block, coinbase 0. */
  position: number;
  /** Net sats for the address: funded minus spent. */
  value: number;
  time: number;
}

export type LedgerHistory = "none" | "taking" | "whole" | "refused" | "dropped" | "too_large";
const HISTORIES: readonly LedgerHistory[] = [
  "none",
  "taking",
  "whole",
  "refused",
  "dropped",
  "too_large",
];

export interface LedgerPoint {
  height: number;
  time: number;
  /** Sats after the block. */
  balance: number;
}

export type Ledger =
  | {
      state: "tracked";
      /** The block the totals are exact at. */
      at: { height: number; time: number };
      /** Tracking began after this block. */
      seeded: { height: number; time: number };
      /** The most newest transactions the server keeps. */
      window: number;
      /** The server has yet to add the newest transactions from before `seeded`. */
      filling: boolean;
      /** Where the address's whole history stands: kept whole, its list being or to be taken,
       * refused by the node, or dropped to keep the server's store within its bound. */
      history: LedgerHistory;
      /** For a history dropped lately: Unix seconds from which it is taken again when viewed. */
      historyAgain: number | null;
      txs: LedgerTx[];
      points: LedgerPoint[];
    }
  | { state: "queued"; position: number }
  | { state: "seeding"; seconds: number }
  | { state: "refused"; creates: number | null; cap: number | null }
  | { state: "failed" }
  | { state: "busy" }
  | { state: "none" };

const NONE: Ledger = { state: "none" };

const count = (v: unknown): v is number => Number.isSafeInteger(v) && (v as number) >= 0;
const sats = (v: unknown): v is number => Number.isSafeInteger(v);
const record = (v: unknown): Record<string, unknown> | undefined =>
  typeof v === "object" && v !== null && !Array.isArray(v)
    ? (v as Record<string, unknown>)
    : undefined;
const isTxid = (v: unknown): v is string => typeof v === "string" && /^[0-9a-f]{64}$/.test(v);

function block(v: unknown): { height: number; time: number } | undefined {
  const r = record(v);
  return r && count(r["height"]) && count(r["time"])
    ? { height: r["height"], time: r["time"] }
    : undefined;
}

function tx(v: unknown): LedgerTx | undefined {
  const r = record(v);
  if (!r) return undefined;
  const { txid, height, position, value, time } = r;
  return isTxid(txid) && count(height) && count(position) && sats(value) && count(time)
    ? { txid, height, position, value, time }
    : undefined;
}

function point(v: unknown): LedgerPoint | undefined {
  const r = record(v);
  if (!r) return undefined;
  const { height, time, balance } = r;
  return count(height) && count(time) && sats(balance) ? { height, time, balance } : undefined;
}

/** All of a list, or nothing: a list with one bad entry is not shown in part. */
function all<T>(v: unknown, read: (x: unknown) => T | undefined): T[] | undefined {
  if (!Array.isArray(v)) return undefined;
  const out: T[] = [];
  for (const x of v) {
    const t = read(x);
    if (t === undefined) return undefined;
    out.push(t);
  }
  return out;
}

export function readLedger(v: unknown): Ledger {
  const r = record(v);
  if (!r) return NONE;
  switch (r["state"]) {
    case "tracked": {
      const at = block(r["at"]);
      const seeded = block(r["seeded"]);
      const txs = all(r["txs"], tx);
      const points = all(r["points"], point);
      const window = r["window"];
      const filling = r["filling"];
      if (!at || !seeded || !txs || !points || !count(window) || typeof filling !== "boolean") {
        return NONE;
      }
      // Anything else reads as not kept whole: the page then shows the newest transactions.
      const history = HISTORIES.find((h) => h === r["history"]) ?? "none";
      const again = r["history_again"];
      const historyAgain = count(again) ? again : null;
      return { state: "tracked", at, seeded, window, filling, history, historyAgain, txs, points };
    }
    case "queued": {
      return count(r["position"]) ? { state: "queued", position: r["position"] } : NONE;
    }
    case "seeding": {
      return count(r["seconds"]) ? { state: "seeding", seconds: r["seconds"] } : NONE;
    }
    case "refused": {
      const creates = count(r["creates"]) ? r["creates"] : null;
      const cap = count(r["cap"]) ? r["cap"] : null;
      return { state: "refused", creates, cap };
    }
    case "failed": {
      return { state: "failed" };
    }
    case "busy": {
      return { state: "busy" };
    }
    default: {
      return NONE;
    }
  }
}

/** While the server is adding the address up, the page asks again this often. */
export const LEDGER_POLL_MS = 3000;

/** After a seed failed, the page asks again this often: the server tries again when asked. */
export const FAILED_POLL_MS = 60_000;

/**
 * While a kept address's page is open, it asks again this often: the server takes (or takes
 * again) its whole history only while it is viewed, and never drops a history viewed in the last
 * 30 minutes (docs/API.md, "Very large addresses").
 */
export const KEPT_POLL_MS = 300_000;

/**
 * Whether the page asks for the ledger again now, `sinceAsk` ms after it last did: every few
 * seconds while a seed waits or runs, now and then after one failed, and every
 * {@link KEPT_POLL_MS} while the address is kept, so the server knows it is still viewed.
 */
export const asksAgain = (l: Ledger, sinceAsk: number): boolean =>
  waiting(l) ||
  (l.state === "failed" && sinceAsk >= FAILED_POLL_MS) ||
  (l.state === "tracked" && sinceAsk >= KEPT_POLL_MS);

/** Whether the page should keep asking: the seed is waiting or running. */
export const waiting = (l: Ledger): boolean =>
  l.state === "queued" || l.state === "seeding" || l.state === "busy";
