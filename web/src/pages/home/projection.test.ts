import { describe, expect, it } from "vitest";
import type { Block } from "../../lib/esplora";
import { fakeBlock } from "../../lib/fakes";
import type { ProjectedBlock, Projection } from "../../lib/mempool-topics";
import {
  EMPTY_MODEL,
  HOLD_MS,
  type ProjectionModel,
  projectedView,
  provisionalFor,
  receive,
  releaseIn,
  sameTiles,
} from "./projection";

/** Mined blocks `from`..`to`, newest first, on `branch`. */
const chain = (from: number, to: number, branch = "a") =>
  Array.from({ length: to - from + 1 }, (_, i) => fakeBlock(to - i, branch));

/** A projected block of `txs` transactions at a median of `median` sat/vB. */
const pb = (txs: number, median = 1, weight = 3_990_000): ProjectedBlock => ({
  txs,
  weight: txs === 0 ? 0 : weight,
  vsize: txs === 0 ? 0 : weight / 4,
  fees: txs * 100,
  minRate: txs === 0 ? 0 : median / 2,
  medianRate: txs === 0 ? 0 : median,
  maxRate: txs === 0 ? 0 : median * 4,
  bands: Array.from({ length: 10 }, (_, i) => (txs > 0 && i === 4 ? weight / 4 : 0)),
});

/** A projection made on `tip`, eight blocks: medians 8, 7, …, 1, the last one ten blocks' worth. */
function projection(tip: Block | null, blocks?: ProjectedBlock[], stale = false): Projection {
  return {
    type: "projected",
    seq: 1,
    tip: tip ? { height: tip.height, hash: tip.id } : null,
    stale,
    blocks:
      blocks ??
      Array.from({ length: 8 }, (_, i) => (i === 7 ? pb(30_000, 1, 40_000_000) : pb(3000, 8 - i))),
  };
}

const keys = (m: ProjectionModel, mined: Block[] | undefined, now = 0) =>
  projectedView(m, mined, now).tiles.map((t) => t.key);
const medians = (m: ProjectionModel, mined: Block[] | undefined, now = 0) =>
  projectedView(m, mined, now).tiles.map((t) => t.medianRate);

describe("the projected tiles", () => {
  it("show the next three blocks and one for the rest, keyed by the height they project", () => {
    const mined = chain(1, 10);
    const m = receive(EMPTY_MODEL, projection(fakeBlock(10)), 0);
    const v = projectedView(m, mined, 0);
    expect(v.tiles.map((t) => t.key)).toStrictEqual([11, 12, 13, 14]);
    // The last tile's middle vbyte lies in the rest, which is most of it.
    expect(v.tiles.map((t) => t.medianRate)).toStrictEqual([8, 7, 6, 1]);
    // The last tile holds blocks 3 to 6 and the rest: four blocks and ten blocks' worth.
    expect(v.tiles[3]?.blocks).toBe(14);
    expect(v.tiles[3]?.txs).toBe(4 * 3000 + 30_000);
    expect(v.tiles[3]?.minRate).toBe(0.5);
    expect(v.tiles[3]?.maxRate).toBe(20);
    expect(v.tiles[3]?.bands[4]).toBe(4 * 997_500 + 10_000_000);
  });

  it("wait for the topic, and stand aside when the server does not track the mempool", () => {
    expect(projectedView(EMPTY_MODEL, chain(1, 10), 0)).toStrictEqual({
      mode: "waiting",
      stale: false,
      tiles: [],
    });
    const off = receive(EMPTY_MODEL, { type: "off" }, 0);
    expect(projectedView(off, chain(1, 10), 0).mode).toBe("off");
    expect(keys(off, chain(1, 10))).toStrictEqual([]);
  });

  it("say when the server has lost its node, and keep the last projection", () => {
    const m = receive(EMPTY_MODEL, projection(fakeBlock(10), undefined, true), 0);
    const v = projectedView(m, chain(1, 10), 0);
    expect(v.stale).toBe(true);
    expect(v.tiles).toHaveLength(4);
  });

  it("show a mempool smaller than a block as one short tile, and an empty one as one empty tile", () => {
    const small = [pb(4, 2, 2_000), ...Array.from({ length: 7 }, () => pb(0))];
    const m = receive(EMPTY_MODEL, projection(fakeBlock(10), small), 0);
    const v = projectedView(m, chain(1, 10), 0);
    expect(v.tiles).toHaveLength(1);
    expect(v.tiles[0]).toMatchObject({ key: 11, txs: 4, blocks: 1, weight: 2_000 });
    const none = receive(
      EMPTY_MODEL,
      projection(
        fakeBlock(10),
        Array.from({ length: 8 }, () => pb(0)),
      ),
      0,
    );
    const empty = projectedView(none, chain(1, 10), 0);
    expect(empty.tiles).toHaveLength(1);
    expect(empty.tiles[0]).toMatchObject({ key: 11, txs: 0, medianRate: 0 });
  });

  it("count a rest of under one block as one block", () => {
    const blocks = [pb(10), pb(10), pb(10), pb(0), pb(0), pb(0), pb(0), pb(5, 1, 400_000)];
    const m = receive(EMPTY_MODEL, projection(fakeBlock(10), blocks), 0);
    expect(projectedView(m, chain(1, 10), 0).tiles[3]).toMatchObject({ blocks: 1, txs: 5 });
  });

  it("change figures in place when a new projection on the same tip arrives", () => {
    const mined = chain(1, 10);
    const a = receive(EMPTY_MODEL, projection(fakeBlock(10)), 0);
    const blocks = projection(fakeBlock(10)).blocks;
    for (const b of blocks) b.medianRate = 9;
    const b = receive(a, projection(fakeBlock(10), blocks), 2_000);
    expect(sameTiles(projectedView(a, mined, 0), projectedView(b, mined, 2_000))).toBe(true);
    expect(medians(b, mined, 2_000)[0]).toBe(9);
    expect(b.entries).toHaveLength(1);
  });
});

describe("the strip under every ordering of block, summary and projection", () => {
  const before = chain(1, 10);
  const after = chain(2, 11);
  const onOld = receive(EMPTY_MODEL, projection(fakeBlock(10)), 0);
  const newer = projection(
    fakeBlock(11),
    Array.from({ length: 8 }, (_, i) => (i === 7 ? pb(20_000, 1, 30_000_000) : pb(3000, 9 - i))),
  );

  it("block first: the old projection shifts by one, its block 1 now next, until the new one comes", () => {
    // The tile for 11 has crossed; 12, 13, 14 slide toward the line and 15 forms at the far end.
    expect(keys(onOld, after)).toStrictEqual([12, 13, 14, 15]);
    expect(medians(onOld, after)).toStrictEqual([7, 6, 5, 1]);
    // The new projection takes the same places: its figures change, nothing moves.
    const m = receive(onOld, newer, 1_000);
    expect(keys(m, after, 1_000)).toStrictEqual([12, 13, 14, 15]);
    expect(medians(m, after, 1_000)).toStrictEqual([9, 8, 7, 1]);
  });

  it("projection first: the new one is held back, so the tile people watched is there to cross", () => {
    const m = receive(onOld, newer, 1_000);
    expect(keys(m, before, 1_500)).toStrictEqual([11, 12, 13, 14]);
    expect(medians(m, before, 1_500)).toStrictEqual([8, 7, 6, 1]);
    expect(releaseIn(m, before, 1_500)).toBe(HOLD_MS - 500);
    // The block lands: the held projection is shown, on its own tip.
    expect(keys(m, after, 2_000)).toStrictEqual([12, 13, 14, 15]);
    expect(medians(m, after, 2_000)).toStrictEqual([9, 8, 7, 1]);
    expect(releaseIn(m, after, 2_000)).toBeUndefined();
  });

  it("projection first, and the block never shows: the projection is shown after a while", () => {
    const m = receive(onOld, newer, 1_000);
    expect(medians(m, before, 1_000 + HOLD_MS)).toStrictEqual([9, 8, 7, 1]);
    // Keyed by its own tip: when the block finally lands, nothing has to move.
    expect(keys(m, before, 1_000 + HOLD_MS)).toStrictEqual([12, 13, 14, 15]);
    expect(keys(m, after, 1_000 + HOLD_MS)).toStrictEqual([12, 13, 14, 15]);
  });

  it("a projection ahead, with nothing older to hold: shown at once", () => {
    const m = receive(EMPTY_MODEL, newer, 0);
    expect(keys(m, before, 0)).toStrictEqual([12, 13, 14, 15]);
  });

  it("knows what the new block was projected to be, whichever came first", () => {
    const block = fakeBlock(11);
    // Block, then the new projection, then (any time) the summary: the old block 0.
    expect(provisionalFor(onOld, after, block)?.medianRate).toBe(8);
    const both = receive(onOld, newer, 1_000);
    expect(provisionalFor(both, after, block)?.medianRate).toBe(8);
    // The summary is not part of this model: it arrives for the tile, keyed by hash.
  });

  it("an empty mined block still crossed from what was projected; a block projected empty did not", () => {
    expect(provisionalFor(onOld, after, { ...fakeBlock(11), tx_count: 1 })?.txs).toBe(3000);
    const empty = receive(
      EMPTY_MODEL,
      projection(
        fakeBlock(10),
        Array.from({ length: 8 }, () => pb(0)),
      ),
      0,
    );
    expect(provisionalFor(empty, after, fakeBlock(11))).toBeUndefined();
  });

  it("a burst: each block takes its own projected block, and the tiles shift by as many", () => {
    const burst = chain(4, 13);
    expect(provisionalFor(onOld, burst, fakeBlock(11))?.medianRate).toBe(8);
    expect(provisionalFor(onOld, burst, fakeBlock(12))?.medianRate).toBe(7);
    expect(provisionalFor(onOld, burst, fakeBlock(13))?.medianRate).toBe(6);
    expect(keys(onOld, burst)).toStrictEqual([14, 15, 16, 17]);
    expect(medians(onOld, burst)).toStrictEqual([5, 4, 3, 1]);
  });

  it("a burst past everything projected shows nothing until the next projection", () => {
    const far = chain(10, 18);
    expect(keys(onOld, far)).toStrictEqual([]);
    expect(projectedView(onOld, far, 0).mode).toBe("on");
    // The rest of the mempool was never a block of its own: nothing to cross with.
    expect(provisionalFor(onOld, far, fakeBlock(18))).toBeUndefined();
  });

  it("a reorg: the projection's tip leaves the chain, so nothing claims to be the new branch's block", () => {
    const m = receive(EMPTY_MODEL, projection(fakeBlock(10)), 0);
    const reorged = [fakeBlock(10, "b", "a"), ...chain(1, 9)];
    expect(provisionalFor(m, reorged, fakeBlock(11, "b"))).toBeUndefined();
    // Still the best guess at what comes next, keyed on from the tip shown.
    expect(keys(m, reorged)).toStrictEqual([11, 12, 13, 14]);
    const longer = [fakeBlock(11, "b"), ...reorged];
    expect(keys(m, longer)).toStrictEqual([12, 13, 14, 15]);
    expect(provisionalFor(m, longer, fakeBlock(11, "b"))).toBeUndefined();
  });

  it("a reorg that drops blocks: the tiles keep their keys until the new branch arrives", () => {
    const m = receive(EMPTY_MODEL, projection(fakeBlock(10)), 0);
    expect(keys(m, chain(1, 8))).toStrictEqual([11, 12, 13, 14]);
  });

  it("before the mined blocks have loaded, the projection is shown as it is", () => {
    expect(keys(onOld, undefined)).toStrictEqual([11, 12, 13, 14]);
    const nullTip = receive(EMPTY_MODEL, projection(null), 0);
    expect(keys(nullTip, before)).toStrictEqual([11, 12, 13, 14]);
  });

  it("off after on drops every projection; on again starts afresh", () => {
    const off = receive(onOld, { type: "off" }, 0);
    expect(off).toStrictEqual({ mode: "off", entries: [] });
    expect(keys(receive(off, newer, 0), after)).toStrictEqual([12, 13, 14, 15]);
  });

  it("keeps a few tips only", () => {
    let m = EMPTY_MODEL;
    for (let h = 1; h <= 20; h++) m = receive(m, projection(fakeBlock(h)), h);
    expect(m.entries.length).toBeLessThanOrEqual(8);
    expect(m.entries.at(-1)?.p.tip?.height).toBe(20);
  });
});
