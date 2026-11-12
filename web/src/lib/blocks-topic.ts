// The `blocks` topic (docs/WS.md "Topics"): new blocks, reorgs and resyncs, and how to keep up.
import type { Block } from "./esplora";
import type { Topic } from "./live";

export interface ChainTip {
  height: number;
  hash: string;
}

export type BlocksMessage =
  | { type: "block"; block: Block }
  | { type: "reorg"; forkHeight: number }
  | { type: "resync"; tip: ChainTip | null };

const isHash = (v: unknown): v is string => typeof v === "string" && /^[0-9a-f]{64}$/.test(v);
const isHeight = (v: unknown): v is number => Number.isSafeInteger(v) && (v as number) >= 0;
const isCount = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v) && v >= 0;

/** A block as the strip needs it; anything else in it is shown only as text. */
export function isBlock(v: unknown): v is Block {
  if (typeof v !== "object" || v === null) return false;
  const b = v as Record<string, unknown>;
  return (
    isHash(b.id) &&
    isHeight(b.height) &&
    (b.previousblockhash === null || isHash(b.previousblockhash)) &&
    isCount(b.timestamp) &&
    isCount(b.tx_count) &&
    isCount(b.weight)
  );
}

function isTip(v: unknown): v is ChainTip {
  if (typeof v !== "object" || v === null) return false;
  const t = v as Record<string, unknown>;
  return isHeight(t.height) && isHash(t.hash);
}

export const blocksTopic: Topic<BlocksMessage> = {
  name: "blocks",
  parse(m) {
    if (m.type === "block" && isBlock(m.block)) return { type: "block", block: m.block };
    if (m.type === "reorg" && isHeight(m.fork_height)) {
      return { type: "reorg", forkHeight: m.fork_height };
    }
    if (m.type === "resync" && (m.tip === null || isTip(m.tip))) {
      return { type: "resync", tip: m.tip };
    }
    return undefined;
  },
};

/** What a `block` message does to a list of blocks, newest first: the new list, or "gap". */
export function applyBlock(blocks: readonly Block[], block: Block, max: number): Block[] | "gap" {
  const tip = blocks[0];
  if (!tip) return "gap";
  if (block.id === tip.id) return [...blocks];
  if (block.height === tip.height + 1 && block.previousblockhash === tip.id) {
    return [block, ...blocks].slice(0, max);
  }
  if (block.height <= tip.height) {
    // A reorg replaced blocks: keep those below it, if it builds on them.
    const below = blocks.filter((b) => b.height < block.height);
    const parent = below[0];
    if (!parent || parent.height !== block.height - 1 || parent.id !== block.previousblockhash) {
      return "gap";
    }
    return [block, ...below];
  }
  return "gap";
}

/** Drops the blocks a reorg took off the best chain: every one above `forkHeight`. */
export function applyReorg(blocks: readonly Block[], forkHeight: number): Block[] {
  return blocks.filter((b) => b.height <= forkHeight);
}

/** Whether a reported tip differs from the newest block shown, so the list must be fetched again. */
export function differsFrom(blocks: readonly Block[], tip: ChainTip | null): boolean {
  return tip !== null && blocks[0]?.id !== tip.hash;
}
