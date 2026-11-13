import { describe, expect, it } from "vitest";
import { formatStood, readTree, replacementsTopic, storyOf, type Tree } from "./replacements";

function must(t: Tree | undefined): Tree {
  if (!t) throw new Error("the tree did not read");
  return t;
}

const id = (n: number) => n.toString(16).padStart(64, "0");
const tx = (n: number, rate: number | null, time: number, rbf: boolean | null = true) => ({
  txid: id(n),
  fee: rate === null ? null : rate * 100,
  vsize: rate === null ? null : 100,
  value: 1000,
  rate,
  time,
  rbf,
});
const node = (t: ReturnType<typeof tx>, replaces: unknown[] = [], interval?: number) => ({
  tx: t,
  time: t.time,
  ...(interval === undefined ? {} : { interval }),
  fullRbf: false,
  replaces,
});

describe("storyOf", () => {
  it("reads a chain oldest first, with how long each version stood", () => {
    const tree = readTree({
      ...node(tx(3, 5, 130), [node(tx(2, 2, 120), [node(tx(1, 1, 100), [], 20)], 10)]),
      mined: true,
    });
    const s = storyOf(must(tree));
    expect(s.versions.map((v) => [v.tx.txid, v.stood])).toStrictEqual([
      [id(1), 20],
      [id(2), 10],
      [id(3), undefined],
    ]);
    expect([s.id, s.mined, s.rise, s.others]).toStrictEqual([id(3), "mined", 4, []]);
  });

  it("follows the deepest branch and lists the rest as also replaced", () => {
    // 9 replaced a child (5) that left with its parent, and a chain 7 ← 6.
    const tree = readTree(
      node(tx(9, 3, 200), [
        node(tx(5, 0.1, 150), [], 50),
        node(tx(7, 2, 190), [node(tx(6, 1, 180), [], 10)], 10),
      ]),
    );
    const s = storyOf(must(tree));
    expect(s.versions.map((v) => v.tx.txid)).toStrictEqual([id(6), id(7), id(9)]);
    expect(s.others.map((t) => t.txid)).toStrictEqual([id(5)]);
    expect(s.mined).toBe("waiting");
  });

  it("says unknown after a reorg, and no rise without both rates", () => {
    const tree = readTree({
      ...node(tx(2, null, 50), [node(tx(1, 1, 40), [], 10)]),
      mined: null,
    });
    const s = storyOf(must(tree));
    expect([s.mined, s.rise]).toStrictEqual(["unknown", undefined]);
  });

  it("refuses a shape it cannot draw", () => {
    expect(readTree({ ...node(tx(1, 1, 1)), replaces: [{ tx: { txid: "x" } }] })).toBeUndefined();
    expect(readTree(null)).toBeUndefined();
  });
});

describe("formatStood and the topic", () => {
  it("prints how long a version stood", () => {
    expect([3, 59, 60, 3725, 7200, 172_800].map((s) => formatStood(s))).toStrictEqual([
      "3 s",
      "59 s",
      "1 min",
      "1 h 2 min",
      "2 h",
      "2 days",
    ]);
  });

  it("reads the record's version and off", () => {
    expect(replacementsTopic.parse({ type: "replacements", version: 4 })).toStrictEqual({
      type: "version",
      version: 4,
    });
    expect(replacementsTopic.parse({ type: "off" })).toStrictEqual({ type: "off" });
    expect(replacementsTopic.parse({ type: "replacements" })).toBeUndefined();
  });
});

describe("readTree at the server's bound", () => {
  it("reads a line of 100 versions, the most a tree holds", () => {
    let tree: unknown = node(tx(1, 1, 1));
    for (let n = 2; n <= 100; n++) tree = node(tx(n, n, n), [tree], 1);
    expect(storyOf(must(readTree(tree))).versions).toHaveLength(100);
  });
});
