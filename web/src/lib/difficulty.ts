// The difficulty adjustment (docs/API.md "Difficulty adjustment"): its reply and how it reads.
import { formatUtc, groupDigits } from "./format";
import type { Network } from "./network";

/** `/api/v1/difficulty-adjustment`, the fields this page reads. */
export interface DifficultyAdjustment {
  progressPercent: number;
  difficultyChange: number;
  /** Unix milliseconds. */
  estimatedRetargetDate: number;
  remainingBlocks: number;
  previousRetarget: number;
  nextRetargetHeight: number;
  expectedBlocks: number;
}

export const PERIOD = 2016;

const KEYS = [
  "progressPercent",
  "difficultyChange",
  "estimatedRetargetDate",
  "remainingBlocks",
  "previousRetarget",
  "nextRetargetHeight",
  "expectedBlocks",
] as const;

export function isDifficultyAdjustment(v: unknown): v is DifficultyAdjustment {
  if (typeof v !== "object" || v === null) return false;
  const d = v as Record<string, unknown>;
  return KEYS.every((k) => typeof d[k] === "number" && Number.isFinite(d[k]));
}

/** Whether the network retargets at all. Regtest never does; its answer is always a change of 0. */
export const retargets = (network: Network): boolean => network !== "regtest";

export interface Change {
  direction: "up" | "down" | "none";
  /** Arrow and words, for the figure: `Up 2.31%`, `Down 7.05%`, `No change`. */
  text: string;
  arrow: string;
}

/** A change in difficulty, in percent, as words. Under 0.005% prints as no change. */
export function change(percent: number): Change {
  const size = Math.abs(percent);
  if (!Number.isFinite(percent) || size < 0.005) {
    return { direction: "none", text: "No change", arrow: "→" };
  }
  const amount = `${size.toFixed(2)}%`;
  return percent > 0
    ? { direction: "up", text: `Up ${amount}`, arrow: "↑" }
    : { direction: "down", text: `Down ${amount}`, arrow: "↓" };
}

/** Time until a moment, in unix ms from now (also ms): `in 12 min`, `in 5 h 20 min`, `in 3 days`. */
export function until(at: number, now: number): string {
  if (!Number.isFinite(at) || !Number.isFinite(now)) return "–";
  const m = Math.max(0, Math.floor((at - now) / 60_000));
  if (m < 1) return "any moment";
  if (m < 60) return `in ${m} min`;
  const h = Math.floor(m / 60);
  if (h < 48) return m % 60 === 0 ? `in ${h} h` : `in ${h} h ${m % 60} min`;
  return `in ${Math.floor(h / 24)} days`;
}

/** The estimated moment in UTC, to the minute: `13 Nov 2026, 14:14 UTC`. */
export const retargetUtc = (ms: number): string => formatUtc(Math.floor(ms / 1000), false);

/** Blocks mined in this period, from the reply: `1 795 of 2 016 blocks`. */
export function periodText(d: DifficultyAdjustment): string {
  const mined = Math.min(PERIOD, Math.max(0, PERIOD - d.remainingBlocks));
  return `${groupDigits(mined)} of ${groupDigits(PERIOD)} blocks`;
}

/** Where the period would be at one block every ten minutes, compared with where it is. */
export function paceText(d: DifficultyAdjustment): string {
  const mined = PERIOD - d.remainingBlocks;
  const ahead = Math.round(mined - d.expectedBlocks);
  if (ahead === 0) return "Blocks are arriving at the target pace of one every 10 minutes.";
  const n = Math.abs(ahead);
  const blocks = `${groupDigits(n)} ${n === 1 ? "block" : "blocks"}`;
  return ahead > 0
    ? `${blocks} ahead of one every 10 minutes, so blocks are coming faster than the target.`
    : `${blocks} behind one every 10 minutes, so blocks are coming slower than the target.`;
}
