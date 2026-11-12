// Packs a block's transactions into squares on a grid of cells (docs/FRONTEND.md "Transaction
// grid"). Pure and deterministic: the same transactions and frame shape give the same picture.

/** One transaction as the grid needs it. */
export interface GridTx {
  /** Stable across changes: the index in the block for a mined block, the txid for a projected one. */
  id: number | string;
  /** Virtual size, vB. */
  vsize: number;
  /** sats; 0 for the coinbase. */
  fee: number;
  /** Total output value, sats, when known (a projected block's records do not carry it). */
  value?: number;
  /**
   * The fee rate to order and color by, sat/vB, when it is not fee over vsize: a projected
   * transaction's effective rate, the rate of the chunk it is mined in.
   */
  rate?: number;
  /** The coinbase pays no fee rate: it is drawn first, in a neutral color. */
  coinbase?: boolean;
}

/** Squares placed on a `cols` × `rows` grid. Arrays are indexed by rank: 0 is the top left. */
export interface Layout {
  cols: number;
  rows: number;
  /** vB per cell: a square of side `s` stands for about `s² × unit` vB. */
  unit: number;
  /** Rows left empty above the squares, which sit on the floor of the frame like sediment. */
  offset: number;
  /** Index in the input of the transaction at each rank. */
  order: Int32Array;
  /** Column of each square's left edge. */
  x: Int32Array;
  /** Row of each square's top edge, `offset` included. */
  y: Int32Array;
  /** Side in cells. */
  side: Int32Array;
  /** The rank covering each cell of the packed area (`offset` not included), or -1. */
  owner: Int32Array;
}

/** About this many cells in a frame: the median transaction gets a side of four or five. */
export const GRID_CELLS = 160_000;
/**
 * The smallest side drawn, in cells. A transaction smaller than this is drawn at this size, its
 * area overstated, so it stays visible and can be pointed at. At the block page's scale only
 * transactions under 14 vB would be, and none exist.
 */
export const MIN_SIDE = 2;
const GROW = 1.02;
const MAX_PASSES = 40;

/** A fee rate in sat/vB, 0 when the size is not positive. */
export const rateOf = (tx: GridTx): number =>
  tx.rate !== undefined && Number.isFinite(tx.rate)
    ? tx.rate
    : tx.vsize > 0
      ? tx.fee / tx.vsize
      : 0;

const compareIds = (a: number | string, b: number | string): number => {
  if (typeof a === "number" && typeof b === "number") return a - b;
  if (typeof a === "number") return -1;
  if (typeof b === "number") return 1;
  return a < b ? -1 : a > b ? 1 : 0;
};

/** Input indices in drawing order: the coinbase, then by fee rate, highest first, then by id. */
export function rank(txs: readonly GridTx[]): Int32Array {
  const rates = txs.map((tx) => rateOf(tx));
  const order = [...txs.keys()];
  order.sort((i, j) => {
    const a = txs[i];
    const b = txs[j];
    if (!a || !b) return 0;
    const coinbase = a.coinbase === true;
    if (coinbase !== (b.coinbase === true)) return coinbase ? -1 : 1;
    return (rates[j] ?? 0) - (rates[i] ?? 0) || compareIds(a.id, b.id);
  });
  return Int32Array.from(order);
}

/**
 * The grid for a frame of this shape (width over height). The shape is rounded to steps of about
 * 4%, so a frame that changes size by a pixel or two keeps its picture and is only rescaled.
 */
export function gridSize(aspect: number, count: number): { cols: number; rows: number } {
  const a = Number.isFinite(aspect) && aspect > 0 ? aspect : 1;
  const q = 2 ** (Math.round(Math.log2(Math.min(64, Math.max(1 / 64, a))) * 16) / 16);
  const cells = Math.max(GRID_CELLS, count * MIN_SIDE * MIN_SIDE * 2);
  const cols = Math.max(1, Math.round(Math.sqrt(cells * q)));
  return { cols, rows: Math.max(1, Math.round(cells / cols)) };
}

/**
 * First fit, row by row from the top left: each square goes in the first place it fits, so later,
 * smaller squares fill the holes beside bigger ones. Returns the row below the lowest square, or
 * -1 when they do not fit in the `owner` grid at all.
 */
function pack(
  side: Int32Array,
  cols: number,
  owner: Int32Array,
  x: Int32Array,
  y: Int32Array,
): number {
  owner.fill(-1);
  const cells = owner.length;
  const rows = cells / cols;
  // Every place before `from[s]` is known not to fit a side of `s` or more: cells only fill up.
  const from = new Int32Array(Math.max(...side, 0) + 1);
  let bottom = 0;
  for (let r = 0; r < side.length; r++) {
    const s = side[r] ?? MIN_SIDE;
    let p = from[s] ?? 0;
    for (;;) {
      const px = p % cols;
      const held = owner[p] ?? -1;
      if (held !== -1) {
        // Step over the square in the way, to the first cell after it on this row.
        p = p - px + (x[held] ?? 0) + (side[held] ?? 1);
        continue;
      }
      if (p >= cells) return -1;
      const py = (p - px) / cols;
      // Rows only go down from here, so if it overflows now it overflows everywhere after.
      if (py + s > rows) return -1;
      if (px + s > cols) {
        p = (py + 1) * cols;
        continue;
      }
      const blocked = blocker(owner, cols, px, py, s);
      if (blocked === -1) {
        for (let dy = 0; dy < s; dy++)
          owner.fill(r, (py + dy) * cols + px, (py + dy) * cols + px + s);
        x[r] = px;
        y[r] = py;
        bottom = Math.max(bottom, py + s);
        // `from` never decreases with the side, so the update stops at the first already past p.
        for (let k = s; k < from.length && (from[k] ?? 0) < p; k++) from[k] = p;
        break;
      }
      // Every start up to the blocking column would cover it too.
      p = py * cols + blocked + 1;
    }
  }
  return bottom;
}

/**
 * Lets every square drop straight down until it rests on another or on the floor, lowest first,
 * so the holes first fit leaves under big squares close up and the block's floor is flat, like
 * sediment. Squares only move down into free cells, so none overlap.
 */
function settle(
  owner: Int32Array,
  cols: number,
  floor: number,
  x: Int32Array,
  y: Int32Array,
  side: Int32Array,
): void {
  // oxlint-disable-next-line unicorn/no-array-sort -- sorts a fresh array; toSorted is newer than the browsers we build for
  const ranks = [...side.keys()].sort(
    (a, b) => (y[b] ?? 0) + (side[b] ?? 0) - ((y[a] ?? 0) + (side[a] ?? 0)) || a - b,
  );
  for (const r of ranks) {
    const s = side[r] ?? 0;
    if (s === 0) continue;
    const px = x[r] ?? 0;
    const top = y[r] ?? 0;
    let below = top + s;
    const free = (row: number) => {
      for (let c = px; c < px + s; c++) if (owner[row * cols + c] !== -1) return false;
      return true;
    };
    while (below < floor && free(below)) below++;
    const drop = below - top - s;
    if (drop === 0) continue;
    for (let dy = 0; dy < s; dy++)
      owner.fill(-1, (top + dy) * cols + px, (top + dy) * cols + px + s);
    for (let dy = 0; dy < s; dy++) {
      owner.fill(r, (top + drop + dy) * cols + px, (top + drop + dy) * cols + px + s);
    }
    y[r] = top + drop;
  }
}

/** The rightmost occupied column met in an s × s square at (px, py), or -1 if it is all free. */
function blocker(owner: Int32Array, cols: number, px: number, py: number, s: number): number {
  for (let dy = 0; dy < s; dy++) {
    const row = (py + dy) * cols;
    for (let dx = s - 1; dx >= 0; dx--) if (owner[row + px + dx] !== -1) return px + dx;
  }
  return -1;
}

/**
 * Packs `txs` into a frame of shape `aspect` (width over height) that holds `capacity` vB, or the
 * transactions' total if that is more. Each square's area follows its virtual size: its side is
 * the exact side rounded to whole cells, at least {@link MIN_SIDE}. When rounding and the holes
 * first fit leaves make the squares overflow, every square shrinks a little and they are packed
 * again, so a full block fills the frame and a half-full one about half of it, from the floor up.
 */
export function layout(txs: readonly GridTx[], aspect: number, capacity: number): Layout {
  const n = txs.length;
  const { cols, rows } = gridSize(aspect, n);
  const order = rank(txs);
  const x = new Int32Array(n);
  const y = new Int32Array(n);
  const side = new Int32Array(n);
  let total = 0;
  let largest = 0;
  for (const tx of txs) {
    const v = Number.isFinite(tx.vsize) && tx.vsize > 0 ? tx.vsize : 0;
    total += v;
    largest = Math.max(largest, v);
  }
  const fit = Math.min(cols, rows);
  let unit = Math.max(
    (Math.max(capacity, total) || 1) / (cols * rows),
    largest / (fit * fit),
    Number.MIN_VALUE,
  );
  // Packed into twice the frame's height, so an overflow says by how much the squares must shrink.
  const owner = new Int32Array(cols * rows * 2);
  let bottom = -1;
  for (let pass = 0; pass < MAX_PASSES; pass++) {
    for (let r = 0; r < n; r++) {
      const v = txs[order[r] ?? 0]?.vsize ?? 0;
      const exact = v > 0 ? Math.sqrt(v / unit) : 0;
      side[r] = Math.min(fit, Math.max(MIN_SIDE, Math.round(exact)));
    }
    bottom = pack(side, cols, owner, x, y);
    if (bottom !== -1 && bottom <= rows) break;
    // Height goes as total area over width, so as 1 / unit; a little more covers the rounding.
    unit *= bottom === -1 ? 2 : Math.max(GROW, (bottom / rows) * GROW);
  }
  if (bottom === -1 || bottom > rows) {
    // Only when even the smallest squares cannot fit, which the grid's size rules out.
    owner.fill(-1);
    side.fill(0);
    bottom = 0;
  }
  settle(owner, cols, bottom, x, y, side);
  const offset = rows - bottom;
  for (let r = 0; r < n; r++) y[r] = (y[r] ?? 0) + offset;
  // Only the frame's own rows are kept for hit testing; the rest was room to measure an overflow.
  return { cols, rows, unit, offset, order, x, y, side, owner: owner.slice(0, cols * rows) };
}

/** The rank of the square under a point given in cells, or -1 for none. */
export function hitTest(l: Layout, cx: number, cy: number): number {
  const col = Math.floor(cx);
  const row = Math.floor(cy) - l.offset;
  if (!(col >= 0 && col < l.cols && row >= 0 && row < l.rows - l.offset)) return -1;
  return l.owner[row * l.cols + col] ?? -1;
}
