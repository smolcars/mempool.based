import { describe, expect, it } from "vitest";
import { applyBlock, applyReorg, blocksTopic, differsFrom } from "./blocks-topic";
import type { Block } from "./esplora";
import { fakeBlock as block } from "./fakes";

const chain = (from: number, to: number, branch = "a") =>
  Array.from({ length: to - from + 1 }, (_, i) => block(to - i, branch));
const heights = (r: Block[] | "gap") => (r === "gap" ? r : r.map((b) => `${b.height}${b.id[0]}`));

describe("blocksTopic.parse", () => {
  it("reads the three message types", () => {
    const b = block(5);
    expect(blocksTopic.parse({ topic: "blocks", type: "block", block: b })).toEqual({
      type: "block",
      block: b,
    });
    expect(blocksTopic.parse({ topic: "blocks", type: "reorg", fork_height: 3 })).toEqual({
      type: "reorg",
      forkHeight: 3,
    });
    expect(blocksTopic.parse({ topic: "blocks", type: "resync", tip: null })).toEqual({
      type: "resync",
      tip: null,
    });
    expect(blocksTopic.parse({ type: "resync", tip: { height: 5, hash: b.id, extra: 1 } })).toEqual(
      {
        type: "resync",
        tip: { height: 5, hash: b.id, extra: 1 },
      },
    );
  });

  it("ignores unknown types and malformed messages", () => {
    expect(blocksTopic.parse({ type: "summary" })).toBeUndefined();
    expect(
      blocksTopic.parse({ type: "block", block: { ...block(5), id: "<img>" } }),
    ).toBeUndefined();
    expect(
      blocksTopic.parse({ type: "block", block: { ...block(5), height: -1 } }),
    ).toBeUndefined();
    expect(blocksTopic.parse({ type: "reorg", fork_height: "3" })).toBeUndefined();
    expect(blocksTopic.parse({ type: "resync", tip: { height: 1 } })).toBeUndefined();
  });
});

describe("applyBlock", () => {
  it("appends the next block and keeps at most `max`", () => {
    expect(heights(applyBlock(chain(1, 10), block(11), 10))).toEqual(heights(chain(2, 11)));
  });

  it("leaves the list alone for the block already on top (a reconnect's snapshot)", () => {
    expect(heights(applyBlock(chain(1, 10), block(10), 10))).toEqual(heights(chain(1, 10)));
  });

  it("replaces blocks at and above a block that builds lower down", () => {
    expect(heights(applyBlock(chain(1, 10), block(9, "b", "a"), 10))).toEqual([
      "9b",
      ...heights(chain(1, 8)),
    ]);
  });

  it("reports a gap: a skipped height, a wrong parent, or nothing to build on", () => {
    expect(applyBlock(chain(1, 10), block(12), 10)).toBe("gap");
    expect(applyBlock(chain(1, 10), block(11, "b"), 10)).toBe("gap");
    expect(applyBlock(chain(1, 10), block(9, "b"), 10)).toBe("gap");
    expect(applyBlock(chain(5, 10), block(3), 10)).toBe("gap");
    expect(applyBlock([], block(3), 10)).toBe("gap");
  });

  it("keeps up with a burst, one block at a time", () => {
    let list: Block[] = chain(1, 10);
    for (let h = 11; h <= 15; h++) {
      const next = applyBlock(list, block(h), 10);
      if (next === "gap") throw new Error("unexpected gap");
      list = next;
    }
    expect(heights(list)).toEqual(heights(chain(6, 15)));
  });

  it("follows a reorg message and the new branch's blocks", () => {
    let list = applyReorg(chain(1, 10), 8);
    expect(heights(list)).toEqual(heights(chain(1, 8)));
    for (const b of [block(9, "b", "a"), block(10, "b"), block(11, "b")]) {
      const next = applyBlock(list, b, 10);
      if (next === "gap") throw new Error("unexpected gap");
      list = next;
    }
    expect(heights(list)).toEqual(["11b", "10b", "9b", ...heights(chain(2, 8))]);
  });
});

describe("differsFrom", () => {
  it("says whether a reported tip is not the block on top", () => {
    const list = chain(1, 10);
    expect(differsFrom(list, { height: 10, hash: block(10).id })).toBe(false);
    expect(differsFrom(list, { height: 11, hash: block(11).id })).toBe(true);
    expect(differsFrom(list, null)).toBe(false);
  });
});
