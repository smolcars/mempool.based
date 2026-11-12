// The projected side of the block strip as a pure model: which projection to show against the
// mined blocks on screen, as which tiles, and what a newly mined block was projected to be.
//
// The server sends a block's `block` message before any projection on it (docs/WS.md "When a block
// is found"), so on the socket the strip always has the block first, and the projection on its
// parent is still here to say what the block was projected to be. A projection is still read
// against the mined blocks shown: one made on a block that is no longer the tip is shifted by the
// blocks mined since (its block 1 is now next), and one made on a block the strip has not shown
// yet is held back for a moment. That happens only while the strip catches up over HTTP (a gap, a
// reconnect), so the tile people were watching is still there to cross the line when it lands.
import type { Block } from "../../lib/esplora";
import type { ProjectedBlock, ProjectedMessage, Projection } from "../../lib/projected-topic";

/** Tiles left of the "now" line: three blocks and one for everything after them. */
export const SLOTS = 4;
/**
 * A projection ahead of the mined blocks shown waits this long for them before it is shown. Still
 * needed: the order the server promises holds on the socket, not for the HTTP refetch after a gap,
 * which is one `/blocks` request and, behind the server's one-second tip cache, one retry.
 */
export const HOLD_MS = 3_000;
/** Tips remembered, so a block mined in a burst can still find what it was projected to be. */
const KEPT = 8;
const BLOCK_WEIGHT = 4_000_000;

interface Entry {
  p: Projection;
  /** When the first projection on this tip arrived. */
  firstAt: number;
}

export interface ProjectionModel {
  /** `waiting` until the topic's first message; `off` when the server does not track the mempool. */
  mode: "waiting" | "off" | "on";
  /** The latest projection on each recent tip, oldest first. */
  entries: readonly Entry[];
}

export const EMPTY_MODEL: ProjectionModel = { mode: "waiting", entries: [] };

export function receive(model: ProjectionModel, m: ProjectedMessage, now: number): ProjectionModel {
  if (m.type === "off") return { mode: "off", entries: [] };
  const p = m.projection;
  const same = (e: Entry) => e.p.tip?.hash === p.tip?.hash;
  const kept = model.entries.find(same);
  const entries = [...model.entries.filter((e) => !same(e)), { p, firstAt: kept?.firstAt ?? now }];
  return { mode: "on", entries: entries.slice(-KEPT) };
}

export interface ProjectedTile {
  /**
   * The height this tile projects; for the last tile, the first height it holds. A tile keeps its
   * key as blocks are mined, so it keeps its element and slides toward the line.
   */
  key: number;
  /** 0 is the next block. */
  slot: number;
  /** How many blocks' worth it holds: 1, or more for the last tile. */
  blocks: number;
  txs: number;
  weight: number;
  vsize: number;
  fees: number;
  minRate: number;
  medianRate: number;
  maxRate: number;
  bands: number[];
}

export interface ProjectedView {
  mode: ProjectionModel["mode"];
  stale: boolean;
  /**
   * Next first: at least the next block, which may be empty. None while waiting or off, or when
   * every block of the projection shown has been mined since.
   */
  tiles: ProjectedTile[];
}

/** The projection to show against `mined`, and how many blocks have been mined on top of it. */
function choose(
  model: ProjectionModel,
  mined: readonly Block[] | undefined,
  now: number,
): { p: Projection; offset: number } | undefined {
  const newest = model.entries.at(-1);
  if (!newest) return undefined;
  const top = mined?.[0];
  if (!top) return { p: newest.p, offset: 0 };
  let chosen = newest;
  const ahead = (e: Entry) => e.p.tip !== null && e.p.tip.height > top.height;
  if (ahead(newest) && now - newest.firstAt < HOLD_MS) {
    // The newest one the strip has caught up with, if any.
    for (const e of model.entries) if (!ahead(e)) chosen = e;
  }
  const tip = chosen.p.tip;
  if (!tip) return { p: chosen.p, offset: 0 };
  const at = mined.findIndex((b) => b.id === tip.hash);
  return { p: chosen.p, offset: at === -1 ? Math.max(0, top.height - tip.height) : at };
}

/** The projected tiles to show beside `mined`, next first. */
export function projectedView(
  model: ProjectionModel,
  mined: readonly Block[] | undefined,
  now: number,
): ProjectedView {
  const c = choose(model, mined, now);
  if (model.mode !== "on" || !c) return { mode: model.mode, stale: false, tiles: [] };
  const base = (c.p.tip?.height ?? mined?.[0]?.height ?? 0) + 1 + c.offset;
  const left = c.p.blocks.slice(c.offset);
  // Every block it projected has been mined since: nothing to show until the next one.
  if (left.length === 0) return { mode: "on", stale: c.p.stale, tiles: [] };
  // The server's last block is the rest of the mempool, of any size.
  const restFrom = c.p.blocks.length - 1 - c.offset;
  const tiles: ProjectedTile[] = [];
  for (let slot = 0; slot < SLOTS && slot < left.length; slot++) {
    const parts = slot === SLOTS - 1 ? left.slice(slot) : [left[slot] as ProjectedBlock];
    const tile = merge(parts, slot, base + slot, restFrom - slot);
    if (slot === 0 || tile.txs > 0) tiles.push(tile);
  }
  return { mode: "on", stale: c.p.stale, tiles };
}

/** One tile from consecutive projected blocks; `restAt` is where in `parts` the rest begins. */
function merge(parts: readonly ProjectedBlock[], slot: number, key: number, restAt: number) {
  const held = parts.filter((b) => b.txs > 0);
  const sum = (f: (b: ProjectedBlock) => number) => held.reduce((s, b) => s + f(b), 0);
  const vsize = sum((b) => b.vsize);
  // The middle vbyte's block; blocks are in rate order, so its median is near the whole's.
  let below = 0;
  const middle = held.find((b) => (below += b.vsize) * 2 >= vsize);
  const blocks = parts.reduce(
    (n, b, i) => n + (b.txs === 0 ? 0 : i >= restAt ? Math.ceil(b.weight / BLOCK_WEIGHT) : 1),
    0,
  );
  return {
    key,
    slot,
    blocks: Math.max(1, blocks),
    txs: sum((b) => b.txs),
    weight: sum((b) => b.weight),
    vsize,
    fees: sum((b) => b.fees),
    minRate: held.length > 0 ? Math.min(...held.map((b) => b.minRate)) : 0,
    medianRate: middle?.medianRate ?? 0,
    maxRate: held.length > 0 ? Math.max(...held.map((b) => b.maxRate)) : 0,
    bands: Array.from({ length: 10 }, (_, i) => held.reduce((s, b) => s + (b.bands[i] ?? 0), 0)),
  };
}

/**
 * What `block` was projected to be, by the newest projection made on one of its ancestors in
 * `mined`: undefined when none was, when it was only part of the unbounded rest, or empty.
 */
export function provisionalFor(
  model: ProjectionModel,
  mined: readonly Block[],
  block: Block,
): ProjectedBlock | undefined {
  for (let i = model.entries.length - 1; i >= 0; i--) {
    const { p } = model.entries[i] as Entry;
    const tip = p.tip;
    if (!tip || tip.height >= block.height) continue;
    if (!mined.some((b) => b.id === tip.hash && b.height === tip.height)) continue;
    const index = block.height - tip.height - 1;
    const guess = index < p.blocks.length - 1 ? p.blocks[index] : undefined;
    // A block projected empty has no figures to show; the tile waits for its own.
    return guess && guess.txs > 0 ? guess : undefined;
  }
  return undefined;
}

/** When the projection shown would change without a new message: a held one being released. */
export function releaseIn(
  model: ProjectionModel,
  mined: readonly Block[] | undefined,
  now: number,
): number | undefined {
  const newest = model.entries.at(-1);
  const top = mined?.[0];
  if (!newest?.p.tip || !top || newest.p.tip.height <= top.height) return undefined;
  const wait = newest.firstAt + HOLD_MS - now;
  return wait > 0 ? wait : undefined;
}

/** Whether two views show the same tiles in the same places, so only their figures differ. */
export function sameTiles(a: ProjectedView | undefined, b: ProjectedView): boolean {
  return (
    !!a &&
    a.mode === b.mode &&
    a.tiles.length === b.tiles.length &&
    a.tiles.every((t, i) => t.key === b.tiles[i]?.key)
  );
}
