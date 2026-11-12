// What the projected-block page knows, and how a new projection changes it. Pure, so a found block,
// a burst, a reorg, `off` and `stale` can be tested without a socket.
import type { ChainTip } from "../../lib/blocks-topic";
import type { ProjectedMessage, Projection } from "../../lib/projected-topic";

/** Projected blocks the server sends: seven with a map, and the rest of the mempool. */
export const PROJECTED_BLOCKS = 8;

/** A position in the URL, 1 for the next block: `/block/next` or `/block/next/:n`. */
export function parsePosition(n: string | undefined): number | null {
  if (n === undefined) return 1;
  return /^[1-8]$/.test(n) ? Number(n) : null;
}

const ORDINALS = ["Second", "Third", "Fourth", "Fifth", "Sixth", "Seventh"];

/** The block at `index` (0 for the next) in words, as a title or inside a sentence. */
export function positionName(index: number, inSentence = false): string {
  const name =
    index === 0
      ? "Next block"
      : index >= PROJECTED_BLOCKS - 1
        ? "The rest of the mempool"
        : `${ORDINALS[index - 1] ?? ""} block from now`;
  return inSentence
    ? `the ${name.replace(/^The /, "").replace(/^./, (c) => c.toLowerCase())}`
    : name;
}

/** How the chain moved under the projection. */
export type TipChange =
  /** Blocks `first` to `tip.height` were found; the newest is `tip`. */
  | { kind: "found"; first: number; tip: ChainTip }
  /** The chain switched branches: `replaced` left it and the projection now sits on `tip`. */
  | { kind: "reorg"; replaced: ChainTip; tip: ChainTip };

/**
 * From one projection's tip to the next. A longer chain reads as blocks found even when a branch
 * switched under it: the newest block is still the one found, and the projection is the server's.
 */
export function tipChange(prev: ChainTip | null, next: ChainTip | null): TipChange | undefined {
  if (!prev || !next || prev.hash === next.hash) return undefined;
  if (next.height > prev.height) return { kind: "found", first: prev.height + 1, tip: next };
  return { kind: "reorg", replaced: prev, tip: next };
}

/**
 * Where the block once shown at `index` went after `change`: its new index, or "mined" when as
 * many blocks were found as there were ahead of it, so it was the guess at one of them.
 */
export function movedTo(change: TipChange, index: number): number | "mined" | undefined {
  if (change.kind !== "found") return undefined;
  const k = change.tip.height - change.first + 1;
  return index - k >= 0 ? index - k : "mined";
}

export interface PageState {
  /** `waiting` for the first projection, `off` when the server has no mempool for this network. */
  status: "waiting" | "off" | "live";
  projection?: Projection;
  /**
   * The last change of tip seen while watching, with when (ms) and the projection that first sat
   * on the new tip, until the reader dismisses it.
   */
  change?: { change: TipChange; at: number; seq: number };
  /** When the server last said it lost its node (ms), while it still says so. */
  staleSince?: number;
}

export const initialState: PageState = { status: "waiting" };

/** Blocks found within this long of the last are told as one burst. */
export const BURST_MS = 60_000;

/** The state after a `projected` message at time `now` (ms). */
export function onProjected(state: PageState, msg: ProjectedMessage, now: number): PageState {
  if (msg.type === "off") return { status: "off" };
  const p = msg.projection;
  let change = tipChange(state.projection?.tip ?? null, p.tip);
  // Blocks found one after another in a burst are one event: "blocks 101 to 103 were found".
  const last = state.change;
  if (
    change?.kind === "found" &&
    last?.change.kind === "found" &&
    change.first === last.change.tip.height + 1 &&
    now - last.at < BURST_MS
  ) {
    change = { ...change, first: last.change.first };
  }
  const next: PageState = { status: "live", projection: p };
  const kept = change ? { change, at: now, seq: p.seq } : state.change;
  if (kept) next.change = kept;
  const since = p.stale ? (state.staleSince ?? now) : undefined;
  if (since !== undefined) next.staleSince = since;
  return next;
}

/** The state once the reader has closed the notice of a change. */
export function dismissChange(state: PageState): PageState {
  const { change: _, ...rest } = state;
  return rest;
}
