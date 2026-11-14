// The Lightning routes' answers (docs/API.md "Lightning"), checks on their shape, and what the
// pages say about them. Pure. Aliases are gossip, chosen by whoever runs the node (SECURITY.md
// rule 14): they reach the page only as text, through cleanAlias.
import { cap, recentMoment, share } from "../mempool/history";
import { formatBtcShort, groupDigits, MISSING } from "../../lib/format";

export type State = "off" | "starting" | "syncing" | "synced" | "failed" | "stopped";

export interface Status {
  state: State;
  peers_configured?: number;
  peers_connected?: number | null;
  /** The peer's graph is synced: with `state` `failed`, the server could not index it yet. */
  synced?: boolean | null;
  channels?: number | null;
  nodes?: number | null;
  validation?: {
    verified: number;
    unchecked: number;
    pending: number;
    early: number;
  } | null;
}

/** A statistics snapshot, or a point of a series: the mean of the snapshots in its step. */
export interface Stats {
  time: number;
  samples: number;
  channels: number;
  nodes: number;
  capacity: number;
  avg_capacity: number;
  med_capacity: number;
  tor_nodes: number;
  clearnet_nodes: number;
  clearnet_tor_nodes: number;
  unannounced_nodes: number;
  policies: number;
  avg_fee_rate: number;
  avg_base_fee_msat: number;
  med_fee_rate: number;
  med_base_fee_msat: number;
}

export interface Series {
  interval: string;
  from: number;
  to: number;
  step: number;
  first: number | null;
  slots: number;
  sampled: number;
  complete: boolean;
  points: Stats[];
}

export interface NodeRef {
  id: string;
  alias: string | null;
}

export interface BlockRef {
  height: number;
  hash: string;
  time: number;
}

export type CloseKind = "cooperative" | "force" | "penalty" | "other";

export interface Close {
  block: BlockRef;
  txid: string;
  kind: CloseKind;
  fee: number | null;
}

export interface Sweep {
  block: BlockRef;
  txid: string;
}

/** A channel in the lists of opens and closes, and on the transaction page. */
export interface ListedChannel {
  id: string;
  scid: string;
  node_one: NodeRef;
  node_two: NodeRef;
  capacity: number;
  opened: BlockRef | null;
  closed: Close | null;
  penalty: Sweep | null;
}

export interface RankedNode {
  id: string;
  alias: string | null;
  capacity: number;
  channels: number;
  first_seen?: number | null;
  last_update?: number | null;
}

export interface Overview {
  statistics: Stats | null;
  top_capacity: RankedNode[];
  top_channels: RankedNode[];
  opened: ListedChannel[];
  closed: ListedChannel[];
}

export type ChannelState = "active" | "inactive" | "closed";

/** A channel in a node's list: the other end is `peer`, `fee_rate` the node's own policy. */
export interface NodeChannel {
  id: string;
  scid: string;
  capacity: number;
  state: ChannelState;
  peer: NodeRef;
  fee_rate: number | null;
  closed: Close | null;
}

export interface NodeChannels {
  status: "open" | "closed";
  page: number;
  per_page: number;
  total: number;
  channels: NodeChannel[];
}

export interface Feature {
  bit: number;
  name: string | null;
  required: boolean;
}

export interface NodeDetail {
  id: string;
  alias: string | null;
  /** Null for a node that sent no announcement. */
  addresses: string[] | null;
  features: Feature[];
  last_update: number | null;
  first_seen: number | null;
  capacity: number;
  channels: {
    open: number;
    active: number;
    closed: number;
    cooperative: number;
    force: number;
    penalty: number;
    other: number;
  };
  open_channels: NodeChannels;
  fees: { incoming: FeeBucket[]; outgoing: FeeBucket[] };
}

/**
 * Channels whose fee rate one way falls in `bucket` (ppm, rounded down: exact under 10, then to
 * tens, hundreds and thousands), or have no policy that way (`null`), and their capacity.
 */
export interface FeeBucket {
  bucket: number | null;
  count: number;
  capacity: number;
}

/** A node's open channels and capacity at each UTC midnight since `from`, then now. */
export interface NodeHistory {
  /** When this server's record begins: its first statistics snapshot; null before it. */
  from: number | null;
  points: { time: number; channels: number; capacity: number }[];
}

export interface Policy {
  fee_rate: number;
  base_fee_msat: number;
  cltv_delta: number;
  min_htlc_msat: number;
  max_htlc_msat: number;
  enabled: boolean;
  last_update: number;
}

export interface End extends NodeRef {
  policy: Policy | null;
}

export interface ChannelDetail {
  id: string;
  scid: string;
  capacity: number;
  state: ChannelState;
  last_update: number | null;
  node_one: End;
  node_two: End;
  funding: { txid: string | null; vout: number; block: BlockRef | null };
  closed: Close | null;
  penalty: Sweep | null;
}

export interface SearchResult {
  nodes: RankedNode[];
  channels: { id: string; scid: string; capacity: number; state: ChannelState }[];
}

export interface Rankings {
  by: string;
  nodes: RankedNode[];
}

export interface TxChannels {
  opened: ListedChannel[];
  closed: ListedChannel[];
}

// Shape checks: enough that a page never renders an object where it expects a figure. Text
// fields are rendered as text whatever they hold.

type Obj = Record<string, unknown>;
const obj = (v: unknown): v is Obj => typeof v === "object" && v !== null && !Array.isArray(v);
const num = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const str = (v: unknown): v is string => typeof v === "string";
const nullable = (v: unknown, f: (v: unknown) => boolean): boolean => v === null || f(v);
const list = <T>(v: unknown, f: (v: unknown) => v is T): v is T[] =>
  Array.isArray(v) && v.every((x) => f(x));

const STATES: ReadonlySet<unknown> = new Set([
  "off",
  "starting",
  "syncing",
  "synced",
  "failed",
  "stopped",
]);
export const isStatus = (v: unknown): v is Status => obj(v) && STATES.has(v["state"]);

const STAT_KEYS = [
  "time",
  "samples",
  "channels",
  "nodes",
  "capacity",
  "avg_capacity",
  "med_capacity",
  "tor_nodes",
  "clearnet_nodes",
  "clearnet_tor_nodes",
  "unannounced_nodes",
  "policies",
  "avg_fee_rate",
  "avg_base_fee_msat",
  "med_fee_rate",
  "med_base_fee_msat",
] as const;
export const isStats = (v: unknown): v is Stats => obj(v) && STAT_KEYS.every((k) => num(v[k]));

export const isSeries = (v: unknown): v is Series =>
  obj(v) &&
  num(v["from"]) &&
  num(v["to"]) &&
  num(v["step"]) &&
  v["step"] > 0 &&
  nullable(v["first"], num) &&
  num(v["slots"]) &&
  num(v["sampled"]) &&
  typeof v["complete"] === "boolean" &&
  list(v["points"], isStats);

const isNodeRef = (v: unknown): v is NodeRef => obj(v) && str(v["id"]) && nullable(v["alias"], str);
const isBlock = (v: unknown): v is BlockRef =>
  obj(v) && num(v["height"]) && str(v["hash"]) && num(v["time"]);
const KINDS: ReadonlySet<unknown> = new Set(["cooperative", "force", "penalty", "other"]);
const isClose = (v: unknown): v is Close =>
  obj(v) && isBlock(v["block"]) && str(v["txid"]) && KINDS.has(v["kind"]);
const isSweep = (v: unknown): v is Sweep => obj(v) && isBlock(v["block"]) && str(v["txid"]);

export const isListedChannel = (v: unknown): v is ListedChannel =>
  obj(v) &&
  str(v["id"]) &&
  isNodeRef(v["node_one"]) &&
  isNodeRef(v["node_two"]) &&
  num(v["capacity"]) &&
  nullable(v["opened"], isBlock) &&
  nullable(v["closed"], isClose) &&
  nullable(v["penalty"] ?? null, isSweep);

const isRanked = (v: unknown): v is RankedNode =>
  obj(v) && num(v["capacity"]) && num(v["channels"]) && isNodeRef(v);

export const isOverview = (v: unknown): v is Overview =>
  obj(v) &&
  nullable(v["statistics"], isStats) &&
  list(v["top_capacity"], isRanked) &&
  list(v["top_channels"], isRanked) &&
  list(v["opened"], isListedChannel) &&
  list(v["closed"], isListedChannel);

const isNodeChannel = (v: unknown): v is NodeChannel =>
  obj(v) &&
  str(v["id"]) &&
  num(v["capacity"]) &&
  str(v["state"]) &&
  isNodeRef(v["peer"]) &&
  nullable(v["fee_rate"], num) &&
  nullable(v["closed"], isClose);

export const isNodeChannels = (v: unknown): v is NodeChannels =>
  obj(v) && num(v["page"]) && num(v["total"]) && list(v["channels"], isNodeChannel);

const isFeature = (v: unknown): v is Feature =>
  obj(v) && num(v["bit"]) && nullable(v["name"], str) && typeof v["required"] === "boolean";

export const isNodeDetail = (v: unknown): v is NodeDetail =>
  obj(v) &&
  nullable(v["addresses"], (x) => list(x, str)) &&
  list(v["features"], isFeature) &&
  num(v["capacity"]) &&
  obj(v["channels"]) &&
  isNodeChannels(v["open_channels"]) &&
  obj(v["fees"]) &&
  list(v["fees"]["incoming"], isFeeBucket) &&
  list(v["fees"]["outgoing"], isFeeBucket) &&
  isNodeRef(v);

const isFeeBucket = (v: unknown): v is FeeBucket =>
  obj(v) && nullable(v["bucket"], (b) => num(b) && b >= 0) && num(v["count"]) && num(v["capacity"]);

export const isNodeHistory = (v: unknown): v is NodeHistory =>
  obj(v) &&
  nullable(v["from"], num) &&
  list(
    v["points"],
    (p): p is NodeHistory["points"][number] =>
      obj(p) && num(p["time"]) && num(p["channels"]) && num(p["capacity"]),
  );

const isPolicy = (v: unknown): v is Policy =>
  obj(v) &&
  num(v["fee_rate"]) &&
  num(v["base_fee_msat"]) &&
  num(v["cltv_delta"]) &&
  typeof v["enabled"] === "boolean";
const isEnd = (v: unknown): v is End => obj(v) && nullable(v["policy"], isPolicy) && isNodeRef(v);

export const isChannelDetail = (v: unknown): v is ChannelDetail => {
  if (!obj(v) || !obj(v["funding"])) return false;
  const f = v["funding"];
  return (
    str(v["id"]) &&
    num(v["capacity"]) &&
    str(v["state"]) &&
    isEnd(v["node_one"]) &&
    isEnd(v["node_two"]) &&
    nullable(f["txid"], str) &&
    nullable(f["block"], isBlock) &&
    nullable(v["closed"], isClose) &&
    nullable(v["penalty"], isSweep)
  );
};

export const isSearchResult = (v: unknown): v is SearchResult =>
  obj(v) &&
  list(v["nodes"], isRanked) &&
  list(
    v["channels"],
    (c): c is SearchResult["channels"][number] => obj(c) && str(c["id"]) && num(c["capacity"]),
  );

export const isRankings = (v: unknown): v is Rankings => obj(v) && list(v["nodes"], isRanked);

export const isTxChannels = (v: unknown): v is TxChannels =>
  obj(v) && list(v["opened"], isListedChannel) && list(v["closed"], isListedChannel);

// Names from gossip.

// Bidi overrides, embeddings and isolates, and every other invisible format character but the
// joiner emoji are built with: they could reorder or hide what a name shows.
const FORMAT = /(?!\u{200D})\p{Cf}/gu;
const CONTROL = /\p{Cc}/gu;
// Stacked combining marks draw over the lines around them: keep at most two on a letter.
const MARKS = /(\p{M}{2})\p{M}+/gu;

/**
 * An alias fit to show as text: control and invisible format characters (bidi overrides
 * included) become U+FFFD, so a reader sees that something is there, and stacked marks are
 * thinned. Null for no alias, or one with nothing visible left.
 */
export function cleanAlias(alias: unknown): string | null {
  if (typeof alias !== "string") return null;
  const clean = alias
    .replaceAll(CONTROL, "\u{FFFD}")
    .replaceAll(FORMAT, "\u{FFFD}")
    .replaceAll(MARKS, "$1");
  return clean.trim() === "" ? null : clean;
}

const graphemes = new Intl.Segmenter(undefined, { granularity: "grapheme" });

/** The first `max` characters as a reader counts them (so no emoji is cut in half), and whether more follow. */
export function cut(text: string, max: number): { text: string; more: boolean } {
  const chars = Array.from(graphemes.segment(text), (g) => g.segment);
  return chars.length > max
    ? { text: `${chars.slice(0, max).join("")}…`, more: true }
    : { text, more: false };
}

/** Characters of a name shown in lists; LDK allows 32 bytes, so a real alias always fits. */
export const NAME_IN_LIST = 32;
/** Characters of a name shown as a page's title before "Show the whole name". */
export const NAME_IN_TITLE = 64;

// Words.

export const CLOSE_WORDS: Record<CloseKind, { name: string; sure: string; why: string }> = {
  cooperative: {
    name: "Cooperative close",
    sure: "Likely",
    why: "Both ends signed a transaction that spends the channel's funds alone and is not a commitment: most likely a mutual close. A splice that only takes funds out looks exactly the same on the chain, so it could be one.",
  },
  force: {
    name: "Force close",
    sure: "Certain",
    why: "The closing transaction is a commitment transaction, which only one end broadcasting the channel's latest state on its own produces. The chain does not say which end closed it or why.",
  },
  penalty: {
    name: "Penalty",
    sure: "Certain",
    why: "One end broadcast an old, revoked state of the channel, and the other end swept its outputs with the revocation key, which only the cheated end holds.",
  },
  other: {
    name: "Other",
    sure: "Certain it is not a plain close",
    why: "The transaction spends other coins besides the channel's funds, which no closing transaction does: most likely a splice that added funds. The channel id ends here all the same.",
  },
};

/** A close kind as one word for a list. */
export const KIND_SHORT: Record<CloseKind, string> = {
  cooperative: "Cooperative",
  force: "Force",
  penalty: "Penalty",
  other: "Other",
};

export const STATE_WORDS: Record<ChannelState, string> = {
  active: "Active",
  inactive: "Inactive",
  closed: "Closed",
};

export const STATE_WHY: Record<ChannelState, string> = {
  active: "At least one end has an enabled forwarding policy.",
  inactive:
    "Neither end has an enabled forwarding policy: both disabled it, or neither sent one in the last two weeks.",
  closed: "Its funding output was spent on the chain.",
};

/** A count with its noun: `1 channel`, `1 807 channels`. */
export const count = (n: number, one: string, many = `${one}s`): string =>
  `${groupDigits(n)} ${n === 1 ? one : many}`;

/** Parts per million, as fee rates on Lightning are written. */
export const ppm = (n: number): string => `${groupDigits(n)} ppm`;
/** Millisatoshis: `1 000 msat`. */
/** A capacity axis's ticks, in BTC: three significant digits below 100, never a power of ten. */
export const btcAxis = (sats: number): string => {
  const btc = sats / 1e8;
  if (btc >= 100) return groupDigits(btc);
  // Below 0.001 JavaScript may write a number as `5e-9`: to the satoshi instead.
  if (btc < 0.001) return btc.toFixed(8).replace(/\.?0+$/, "");
  return String(Number(btc.toPrecision(3)));
};

export const msat = (n: number): string => `${groupDigits(n)} msat`;

/**
 * Millisatoshis as a program reads them: digits only, no grouping or unit. The same decimal digits
 * `msat` shows, past 2^53 too (a BigInt there gave the binary value's digits, not the shown ones).
 */
export const plainMsat = (n: number): string =>
  Number.isFinite(n) ? Math.round(n).toString() : MISSING;

/**
 * Millisatoshis in magnitude, as a reader compares limits: msat under a sat (`900 msat`), sats
 * under a million (`1 sat`, `1.5 sats`, `958 366 sats`), then BTC (`0.190 BTC`, `21 000 000 BTC`).
 * The exact figure is {@link msat}.
 */
export function msatShort(n: number): string {
  if (!Number.isFinite(n) || n < 1000) return msat(n);
  const sats = n / 1000;
  if (sats >= 1_000_000) return `${formatBtcShort(Math.round(sats))} BTC`;
  const whole = Number.isInteger(sats);
  const text = whole ? groupDigits(sats) : sats.toFixed(3).replace(/0+$/, "");
  return `${text} ${sats === 1 ? "sat" : "sats"}`;
}

/** A channel id as the API writes it: `811984x2037x0`; or the 64-bit one in decimal. */
export function isChannelId(v: string): boolean {
  return (
    /^(?:0|[1-9]\d{0,19})$/.test(v) || /^(?:0|[1-9]\d*)x(?:0|[1-9]\d*)x(?:0|[1-9]\d*)$/.test(v)
  );
}

/** A public key: 66 hex digits. */
export const isPublicKey = (v: string): boolean => /^[0-9a-f]{66}$/i.test(v);

/** What the search route accepts, or why not, said before anything is asked. */
export function searchProblem(q: string): string | undefined {
  const t = q.trim();
  // oxlint-disable-next-line no-control-regex -- refusing control characters is the point
  if (/[\u0000-\u001F\u007F-\u009F]/.test(t)) return "A search cannot hold control characters.";
  // Code points, as the server counts: a surrogate pair is one.
  const n = t.replaceAll(/[\uD800-\uDBFF][\uDC00-\uDFFF]/g, "_").length;
  if (n < 3) return "Type at least 3 characters of a name, a public key or a channel id.";
  if (n > 66) return "A search is at most 66 characters: a public key is the longest.";
  return undefined;
}

// The network's history.

export const INTERVALS = [
  { id: "24h", label: "24 h", words: "the last 24 hours" },
  { id: "1w", label: "1 w", words: "the last week" },
  { id: "1m", label: "1 m", words: "the last month" },
  { id: "3m", label: "3 m", words: "the last 3 months" },
  { id: "1y", label: "1 y", words: "the last year" },
  { id: "all", label: "All", words: "all of history" },
] as const;
export type Interval = (typeof INTERVALS)[number];

/** The interval a `?interval=` value names; a week for anything else. */
export function parseInterval(value: unknown): Interval {
  const id: unknown = Array.isArray(value) ? (value as unknown[])[0] : value;
  return INTERVALS.find((p) => p.id === id) ?? INTERVALS[1];
}

export const RANKINGS = [
  { id: "capacity", label: "Capacity", words: "by capacity" },
  { id: "channels", label: "Channels", words: "by open channels" },
  { id: "age", label: "Age", words: "oldest first" },
] as const;
export type Ranking = (typeof RANKINGS)[number];

export function parseRanking(value: unknown): Ranking {
  const id: unknown = Array.isArray(value) ? (value as unknown[])[0] : value;
  return RANKINGS.find((r) => r.id === id) ?? RANKINGS[0];
}

/**
 * Where the hatch for "not recorded yet" ends: the start of the step holding the first record;
 * undefined when that is not inside the period.
 */
export function dataFrom(s: Series): number | undefined {
  if (s.first === null) return undefined;
  const start = Math.floor(s.first / s.step) * s.step;
  return start > s.from ? start : undefined;
}

/**
 * What the series covers, in one short line, as the mempool history says it; undefined when it
 * holds the whole period. History starts when this server's peer first synced, which is said, not
 * hidden. (The answer's `sampled` leaves out the ten minutes still running, so it is quoted only
 * for gaps inside the period.)
 */
export function coverageSentence(s: Series, now = Date.now() / 1000): string | undefined {
  if (s.first === null || s.points.length === 0) return undefined;
  if (s.interval === "all") return `Recorded since ${recentMoment(s.first, now)}.`;
  if (s.first > s.from) return `Recorded from ${recentMoment(s.first, now)}.`;
  if (s.complete) return undefined;
  return `${cap(share(s.sampled, s.slots))} recorded, with gaps while this server was off.`;
}

// A node's fee rates.

/**
 * The fee histogram's rows, by lower bound in ppm: few enough to read on a phone whatever the
 * node, each a union of the server's buckets, so a channel is never split between two.
 */
const FEE_ROWS = [0, 1, 10, 50, 100, 200, 500, 1_000, 2_000, 5_000, 10_000] as const;

/** One way's channels in a row of the histogram. */
export interface FeeCell {
  count: number;
  capacity: number;
}

export interface FeeRow {
  /** `0`, `1–9`, `10 000+`, or "No policy". */
  label: string;
  /** The row's lowest rate, ppm; null for channels with no policy that way. */
  from: number | null;
  outgoing: FeeCell;
  incoming: FeeCell;
}

const rowOf = (bucket: number): number => FEE_ROWS.filter((from) => bucket >= from).length - 1;

const rowLabel = (k: number): string => {
  const from = FEE_ROWS[k] ?? 0;
  const next = FEE_ROWS[k + 1];
  if (next === undefined) return `${groupDigits(from)}+`;
  return next - 1 === from ? groupDigits(from) : `${groupDigits(from)}–${groupDigits(next - 1)}`;
};

/**
 * The node's fees as the histogram draws them, cheapest first: the rows from the cheapest that
 * holds a channel either way to the dearest, those between kept empty so the scale stays even,
 * then "No policy" when any channel has none.
 */
export function feeRows(fees: NodeDetail["fees"]): FeeRow[] {
  const rows = [...FEE_ROWS.keys(), -1].map((k): FeeRow => ({
    label: k < 0 ? "No policy" : rowLabel(k),
    from: k < 0 ? null : (FEE_ROWS[k] ?? 0),
    outgoing: { count: 0, capacity: 0 },
    incoming: { count: 0, capacity: 0 },
  }));
  for (const way of ["outgoing", "incoming"] as const) {
    for (const b of fees[way]) {
      const row = rows.at(b.bucket === null ? -1 : rowOf(b.bucket));
      if (row === undefined) continue;
      row[way].count += b.count;
      row[way].capacity += b.capacity;
    }
  }
  const held = (r: FeeRow) => r.outgoing.count + r.incoming.count > 0;
  const priced = rows.slice(0, -1);
  const kept = priced.flatMap((r, i) => (held(r) ? [i] : []));
  const none = rows.at(-1);
  return [
    ...priced.slice(kept[0] ?? 0, (kept.at(-1) ?? -1) + 1),
    ...(none !== undefined && held(none) ? [none] : []),
  ];
}

/** The row holding the median channel one way, among those with a policy; null with none. */
export function medianRow(rows: FeeRow[], way: "outgoing" | "incoming"): FeeRow | null {
  const priced = rows.filter((r) => r.from !== null);
  const total = priced.reduce((n, r) => n + r[way].count, 0);
  if (total === 0) return null;
  let seen = 0;
  for (const r of priced) {
    seen += r[way].count;
    if (seen * 2 >= total) return r;
  }
  return null;
}

/** What the node charges and is charged, in one sentence, from the histogram's rows. */
export function feeSentence(rows: FeeRow[]): string {
  // Each channel is in one row each way: the outgoing counts are the node's channels.
  const one = rows.reduce((sum, x) => sum + x.outgoing.count, 0) === 1;
  const say = (way: "outgoing" | "incoming") => {
    const r = medianRow(rows, way);
    const n = rows.reduce((sum, x) => sum + (x.from === null ? 0 : x[way].count), 0);
    if (r === null) return null;
    return `${n === 1 ? "" : "a median of "}${r.label} ppm`;
  };
  const out = say("outgoing");
  const inc = say("incoming");
  const charges =
    out === null
      ? `It has set no fee on its ${one ? "channel" : "channels"}`
      : `It charges ${out} to forward`;
  const charged = one
    ? inc === null
      ? "its peer has set none towards it"
      : `its peer charges ${inc} towards it`
    : inc === null
      ? "its peers have set none towards it"
      : `its peers charge ${inc} towards it`;
  return `${charges}; ${charged}.`;
}
