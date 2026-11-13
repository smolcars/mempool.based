// The mining routes' replies (docs/API.md "Mining"), the periods the page offers, and what the
// page computes from them. Pure.
import type { Coverage } from "../../lib/api";
import { sorted } from "../../components/chart/geometry";
import { formatDay } from "../../components/chart/scale";
import type { Network } from "../../lib/network";

const DAY = 86_400;
const WEEK = 7 * DAY;

export const PERIODS = [
  { id: "24h", label: "24 h", words: "the last 24 hours", seconds: DAY },
  { id: "1w", label: "1 w", words: "the last week", seconds: 7 * DAY },
  { id: "1m", label: "1 m", words: "the last 30 days", seconds: 30 * DAY },
  { id: "1y", label: "1 y", words: "the last year", seconds: 365 * DAY },
  { id: "all", label: "All", words: "all time", seconds: undefined },
] as const;

export type Period = (typeof PERIODS)[number];
export type PeriodId = Period["id"];
export const DEFAULT_PERIOD: PeriodId = "1w";

/** The period a `?period=` value names; the default for anything else. */
export function parsePeriod(value: unknown): Period {
  const id: unknown = Array.isArray(value) ? (value as unknown[])[0] : value;
  return PERIODS.find((p) => p.id === id) ?? periodById(DEFAULT_PERIOD);
}

export function periodById(id: PeriodId): Period {
  // PERIODS holds every id, so this always finds one.
  return PERIODS.find((p) => p.id === id) ?? PERIODS[1];
}

/**
 * The hashrate chart's interval. Each daily point is a 7-day average, so a day would be a single
 * point: the shortest the chart shows is a week.
 */
export const hashrateInterval = (p: Period): PeriodId => (p.id === "24h" ? "1w" : p.id);

/** Pool shares over time are weekly: shorter periods have one or two points, so no chart. */
export const sharesOverTime = (p: Period): boolean => p.id !== "24h" && p.id !== "1w";

/**
 * The time range a chart of the period draws, so a period the data does not fill shows where it
 * is missing: from the period's start to now (or the last point, if later). Undefined for all
 * time, which draws the data's own extent.
 */
export function periodDomain(
  p: Period,
  x: readonly number[],
  now: number,
): [number, number] | undefined {
  const first = x[0];
  const last = x.at(-1);
  if (p.seconds === undefined || first === undefined || last === undefined) return undefined;
  return [Math.min(first, now - p.seconds), Math.max(last, now)];
}

/** Block-series points by time (the server orders them by height), or undefined for none. */
export function byTime<T extends BlockPoint>(points: readonly T[] | undefined): T[] | undefined {
  return points && sorted(points, (a, b) => a.timestamp - b.timestamp || a.avgHeight - b.avgHeight);
}

/**
 * A pool's weekly points with the weeks it found no block filled in as zero: the route leaves
 * them out, and a chart would otherwise draw a line across them. Weeks before its first and
 * after its last stored week are not invented.
 */
export function fillWeeks(weeks: readonly PoolWeek[]): PoolWeek[] {
  const by = sorted(weeks, (a, b) => a.timestamp - b.timestamp);
  const first = by[0];
  const last = by.at(-1);
  if (!first || !last) return [];
  const at = new Map(by.map((w) => [w.timestamp, w]));
  const out: PoolWeek[] = [];
  for (let t = first.timestamp; t <= last.timestamp; t += WEEK) {
    out.push(at.get(t) ?? { timestamp: t, avgHashrate: 0, share: 0, poolName: first.poolName });
  }
  return out;
}

/** `reward-stats` sums at most a year of blocks. */
export const REWARD_MAX = 52_560;

// Reply shapes. Numbers the page reads are checked; text is rendered as text.

type Obj = Record<string, unknown>;
const isObj = (v: unknown): v is Obj => typeof v === "object" && v !== null && !Array.isArray(v);
const isNum = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const nums = (o: Obj, keys: readonly string[]) => keys.every((k) => isNum(o[k]));
const listOf =
  <T>(check: (v: unknown) => v is T) =>
  (v: unknown): v is T[] =>
    Array.isArray(v) && v.every((x) => check(x));

export interface Ranked {
  poolId: number;
  name: string;
  link: string;
  blockCount: number;
  rank: number;
  emptyBlocks: number;
  slug: string;
}

export interface Ranking {
  pools: Ranked[];
  blockCount: number;
  lastEstimatedHashrate: number | null;
  lastEstimatedHashrate3d: number | null;
  lastEstimatedHashrate1w: number | null;
}

const isRanked = (v: unknown): v is Ranked =>
  isObj(v) &&
  nums(v, ["poolId", "blockCount", "rank", "emptyBlocks"]) &&
  typeof v["name"] === "string" &&
  typeof v["slug"] === "string" &&
  typeof v["link"] === "string";

const numOrNull = (v: unknown) => v === null || isNum(v);

export const isRanking = (v: unknown): v is Ranking =>
  isObj(v) &&
  listOf(isRanked)(v["pools"]) &&
  isNum(v["blockCount"]) &&
  numOrNull(v["lastEstimatedHashrate"]) &&
  numOrNull(v["lastEstimatedHashrate1w"]);

export interface HashratePoint {
  timestamp: number;
  avgHashrate: number;
}
export interface Retarget {
  time: number;
  height: number;
  difficulty: number;
  adjustment: number;
}
export interface Hashrates {
  hashrates: HashratePoint[];
  difficulty: Retarget[];
  currentHashrate: number | null;
  currentDifficulty: number;
}

export const isHashrates = (v: unknown): v is Hashrates =>
  isObj(v) &&
  listOf((p): p is HashratePoint => isObj(p) && nums(p, ["timestamp", "avgHashrate"]))(
    v["hashrates"],
  ) &&
  listOf((p): p is Retarget => isObj(p) && nums(p, ["time", "height", "difficulty", "adjustment"]))(
    v["difficulty"],
  ) &&
  numOrNull(v["currentHashrate"]) &&
  isNum(v["currentDifficulty"]);

export interface PoolWeek {
  timestamp: number;
  avgHashrate: number;
  share: number;
  poolName: string;
}

export const isPoolWeeks = listOf(
  (p): p is PoolWeek =>
    isObj(p) && nums(p, ["timestamp", "avgHashrate", "share"]) && typeof p["poolName"] === "string",
);

export interface RewardStats {
  startBlock: number;
  endBlock: number;
  totalReward: string;
  totalFee: string;
  totalTx: string;
}

const isDigits = (v: unknown): v is string => typeof v === "string" && /^\d{1,20}$/.test(v);

export const isRewardStats = (v: unknown): v is RewardStats =>
  isObj(v) &&
  nums(v, ["startBlock", "endBlock"]) &&
  isDigits(v["totalReward"]) &&
  isDigits(v["totalFee"]) &&
  isDigits(v["totalTx"]);

/** A point of a block series: the middle height and time of the blocks it averages. */
export interface BlockPoint {
  avgHeight: number;
  timestamp: number;
}

export const isPoints = (field: string) =>
  listOf((p): p is BlockPoint & Obj => isObj(p) && nums(p, ["avgHeight", "timestamp", field]));

export const RATE_KEYS = [
  "avgFee_0",
  "avgFee_10",
  "avgFee_25",
  "avgFee_50",
  "avgFee_75",
  "avgFee_90",
  "avgFee_100",
] as const;

export const isRatePoints = listOf(
  (p): p is BlockPoint & Record<(typeof RATE_KEYS)[number], number> =>
    isObj(p) && nums(p, ["avgHeight", "timestamp", ...RATE_KEYS]),
);

export const isSizes = (
  v: unknown,
): v is {
  sizes: (BlockPoint & { avgSize: number })[];
  weights: (BlockPoint & { avgWeight: number })[];
} => isObj(v) && isPoints("avgSize")(v["sizes"]) && isPoints("avgWeight")(v["weights"]);

export interface PoolStats {
  pool: {
    id: number;
    name: string;
    link: string;
    addresses: string[];
    regexes: string[];
    slug: string;
  };
  blockCount: { all: number; "24h": number; "1w": number };
  blockShare: { all: number; "24h": number; "1w": number };
  estimatedHashrate: number | null;
  totalReward: string;
}

const isPeriods = (v: unknown) => isObj(v) && nums(v, ["all", "24h", "1w"]);
const isStrings = listOf((s): s is string => typeof s === "string");

export const isPoolStats = (v: unknown): v is PoolStats => {
  if (!isObj(v)) return false;
  const p = v["pool"];
  return (
    isObj(p) &&
    isNum(p["id"]) &&
    typeof p["name"] === "string" &&
    typeof p["link"] === "string" &&
    typeof p["slug"] === "string" &&
    isStrings(p["addresses"]) &&
    isStrings(p["regexes"]) &&
    isPeriods(v["blockCount"]) &&
    isPeriods(v["blockShare"]) &&
    numOrNull(v["estimatedHashrate"]) &&
    isDigits(v["totalReward"])
  );
};

export interface PoolBlock {
  id: string;
  height: number;
  timestamp: number;
  tx_count: number;
  size: number;
  weight: number;
  extras: { totalFees: number; medianFee: number | null; reward: number };
}

export const isPoolBlocks = listOf(
  (b): b is PoolBlock =>
    isObj(b) &&
    typeof b["id"] === "string" &&
    nums(b, ["height", "timestamp", "tx_count", "size", "weight"]) &&
    isObj(b["extras"]) &&
    nums(b["extras"], ["totalFees", "reward"]) &&
    numOrNull(b["extras"]["medianFee"]),
);

// What the page computes.

/**
 * Hashrate points and the difficulty in force at each, from the hashrate answer. Before the
 * first retarget in the answer, the difficulty is the one that retarget replaced; with no retarget
 * in the answer, it is the current one throughout.
 */
export function hashrateSeries(h: Hashrates): {
  x: number[];
  hashrate: number[];
  difficulty: (number | null)[];
} {
  const points = sorted(h.hashrates, (a, b) => a.timestamp - b.timestamp);
  const retargets = sorted(h.difficulty, (a, b) => a.time - b.time);
  const first = retargets[0];
  // No retarget in the answer: every point is in the current period.
  const before = first
    ? first.adjustment > 0
      ? first.difficulty / first.adjustment
      : null
    : h.currentDifficulty;
  let k = -1;
  const difficulty = points.map((p) => {
    while (k + 1 < retargets.length && (retargets[k + 1]?.time ?? Infinity) <= p.timestamp) k++;
    return k >= 0 ? (retargets[k]?.difficulty ?? null) : before;
  });
  return {
    x: points.map((p) => p.timestamp),
    hashrate: points.map((p) => p.avgHashrate),
    difficulty,
  };
}

export const OTHER_POOLS = "Other pools";

/**
 * The six pools drawn in a colour of their own, the same at every period and in every share
 * graphic, so a colour always names one pool: the six with the most blocks in the year to
 * November 2026, biggest first. Every other pool is counted in "Other pools", in grey; the table
 * names them all.
 * shortcut: a fixed list, revise it when a pool outside it joins the year's top six.
 */
export const COLOURED_POOLS = [
  "Foundry USA",
  "AntPool",
  "F2Pool",
  "ViaBTC",
  "SpiderPool",
  "MARA Pool",
] as const;

/** A pool's colour, 1–6, or 0 for the grey every other pool shares. */
export function poolColour(name: string): number {
  return (COLOURED_POOLS as readonly string[]).indexOf(name) + 1;
}

/**
 * Weekly pool shares as stacked layers: each coloured pool found in the period, the biggest over
 * the period at the bottom, then every other pool as one grey layer.
 */
export function poolLayers(weeks: readonly PoolWeek[]): {
  x: number[];
  layers: { name: string; values: number[]; pool: number }[];
} {
  const x = sorted(new Set(weeks.map((w) => w.timestamp)), (a, b) => a - b);
  const at = new Map(x.map((t, i) => [t, i]));
  const totals = new Map<string, number>();
  for (const w of weeks) totals.set(w.poolName, (totals.get(w.poolName) ?? 0) + w.share);
  const named = sorted(totals.entries(), (a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
    .map(([name]) => name)
    .filter((name) => poolColour(name) > 0);
  const layers = named.map((name) => ({
    name,
    values: x.map(() => 0),
    pool: poolColour(name),
  }));
  const index = new Map(named.map((n, i) => [n, i]));
  const rest = x.map(() => 0);
  for (const w of weeks) {
    const i = at.get(w.timestamp) ?? 0;
    const layer = layers[index.get(w.poolName) ?? -1];
    if (layer) layer.values[i] = (layer.values[i] ?? 0) + w.share;
    else rest[i] = (rest[i] ?? 0) + w.share;
  }
  if (totals.size > named.length) layers.push({ name: OTHER_POOLS, values: rest, pool: 0 });
  return { x, layers };
}

/**
 * A pool's estimated hashrate over the period: its share of the blocks times the network's
 * hashrate over the same stretch (the last 144 blocks for a day, 1,008 for a week, the average of
 * the chart's points beyond).
 */
export function networkHashrate(
  p: Period,
  ranking: Ranking,
  hashrates: Hashrates | undefined,
): number | undefined {
  if (p.id === "24h") return ranking.lastEstimatedHashrate ?? undefined;
  if (p.id === "1w") return ranking.lastEstimatedHashrate1w ?? undefined;
  const points = hashrates?.hashrates ?? [];
  if (points.length === 0) return undefined;
  return points.reduce((sum, h) => sum + h.avgHashrate, 0) / points.length;
}

/** The figures of a period's rewards, in sats; undefined for a reply that is not whole numbers. */
export function rewardFigures(r: RewardStats): {
  blocks: number;
  reward: number;
  fees: number;
  subsidy: number;
  txs: number;
  feeShare: number;
  perBlock: number;
} {
  const reward = Number(r.totalReward);
  const fees = Number(r.totalFee);
  const blocks = Math.max(0, r.endBlock - r.startBlock + 1);
  return {
    blocks,
    reward,
    fees,
    subsidy: Math.max(0, reward - fees),
    txs: Number(r.totalTx),
    feeShare: reward > 0 ? fees / reward : 0,
    perBlock: blocks > 0 ? reward / blocks : 0,
  };
}

/**
 * What a reply's coverage says about the period asked for, as a sentence; undefined when the
 * data covers it. "Data from 3 Oct 2026 onward: …".
 */
export function coverageNote(c: Coverage | undefined, p: Period): string | undefined {
  if (!c || c.complete) return undefined;
  if (c.fromTime === undefined) return `No blocks are stored for ${p.words} yet.`;
  return `Data from ${formatDay(c.fromTime)} onward. This server is still filling in its history, so ${p.words} is not all here yet.`;
}

const NOTES: Record<Network, string | undefined> = {
  mainnet: undefined,
  signet:
    "Signet's blocks are signed by a small set of authorized signers, not won by competing miners. The pool list names none of them, so most blocks show as Unknown, and the hashrate says little about the network.",
  testnet4:
    "Testnet4 lets anyone mine a block at the minimum difficulty when none has been found for 20 minutes, so its hashrate estimate swings widely and pool shares change from day to day.",
  regtest:
    "Regtest blocks are made on demand by whoever runs the node, at the minimum difficulty. There are no pools and the hashrate is not meaningful here; the block figures are real.",
};

/** What makes a network's mining figures different from mainnet's, in plain words. */
export const networkNote = (network: Network): string | undefined => NOTES[network];

/** Whether pools and hashrate mean anything on a network. */
export const competitive = (network: Network): boolean => network !== "regtest";

/**
 * A data-derived link, only if it is plain http or https (SECURITY.md rule 14): its URL and the
 * host to show; undefined for anything else.
 */
export function safeLink(link: string): { href: string; host: string } | undefined {
  let url: URL;
  try {
    url = new URL(link);
  } catch {
    return undefined;
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") return undefined;
  if (url.username !== "" || url.password !== "") return undefined;
  return { href: url.href, host: url.host.replace(/^www\./, "") };
}
