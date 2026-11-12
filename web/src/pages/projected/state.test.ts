import { describe, expect, it } from "vitest";
import type { ProjectedMessage } from "../../lib/projected-topic";
import {
  BURST_MS,
  dismissChange,
  initialState,
  movedTo,
  onProjected,
  type PageState,
  parsePosition,
  positionName,
} from "./state";

const tip = (height: number, branch = "a") => ({
  height,
  hash: `${branch}${height.toString(16).padStart(63, "0")}`,
});
const projected = (height: number, branch = "a", stale = false): ProjectedMessage => ({
  type: "projected",
  projection: { seq: height, tip: tip(height, branch), stale, blocks: [] },
});
const run = (msgs: [ProjectedMessage, number][], from: PageState = initialState) =>
  msgs.reduce((s, [m, at]) => onProjected(s, m, at), from);

describe("parsePosition and positionName", () => {
  it("reads 1 to 8 from the path, the next block by default", () => {
    expect(parsePosition(undefined)).toBe(1);
    expect(parsePosition("1")).toBe(1);
    expect(parsePosition("8")).toBe(8);
    for (const bad of ["0", "9", "01", "-1", "2.0", "x", ""]) expect(parsePosition(bad)).toBeNull();
  });

  it("names each place, in a title and in a sentence", () => {
    expect(positionName(0)).toBe("Next block");
    expect(positionName(2)).toBe("Third block from now");
    expect(positionName(7)).toBe("The rest of the mempool");
    expect(positionName(0, true)).toBe("the next block");
    expect(positionName(6, true)).toBe("the seventh block from now");
    expect(positionName(7, true)).toBe("the rest of the mempool");
  });
});

describe("onProjected", () => {
  it("is waiting until the first projection, which is not news", () => {
    expect(initialState.status).toBe("waiting");
    const s = run([[projected(100), 0]]);
    expect(s.status).toBe("live");
    expect(s.change).toBeUndefined();
  });

  it("tells a found block, and keeps telling it as the projection moves on", () => {
    const s = run([
      [projected(100), 0],
      [projected(101), 1000],
      [projected(101), 3000],
    ]);
    expect(s.change).toStrictEqual({
      change: { kind: "found", first: 101, tip: tip(101) },
      at: 1000,
    });
    expect(dismissChange(s).change).toBeUndefined();
  });

  it("tells a burst as one event, whether it comes in one jump or block by block", () => {
    expect(
      run([
        [projected(100), 0],
        [projected(103), 1],
      ]).change?.change,
    ).toStrictEqual({
      kind: "found",
      first: 101,
      tip: tip(103),
    });
    const stepwise = run([
      [projected(100), 0],
      [projected(101), 1000],
      [projected(102), 2000],
      [projected(103), 4000],
    ]);
    expect(stepwise.change).toStrictEqual({
      change: { kind: "found", first: 101, tip: tip(103) },
      at: 4000,
    });
    // Long after the last one, a new block is news of its own.
    const later = run([[projected(104), 4000 + BURST_MS]], stepwise);
    expect(later.change?.change).toMatchObject({ first: 104 });
  });

  it("tells a reorg, which leaves the page on the new branch", () => {
    const s = run([
      [projected(100), 0],
      [projected(100, "b"), 1],
    ]);
    expect(s.change?.change).toStrictEqual({
      kind: "reorg",
      replaced: tip(100),
      tip: tip(100, "b"),
    });
    expect(s.projection?.tip).toStrictEqual(tip(100, "b"));
    const shorter = run([[projected(99, "b"), 2]], s);
    expect(shorter.change?.change).toMatchObject({ kind: "reorg", tip: tip(99, "b") });
  });

  it("is off when the server says so", () => {
    expect(
      run([
        [projected(100), 0],
        [{ type: "off" }, 1],
      ]),
    ).toStrictEqual({ status: "off" });
  });

  it("keeps the last picture while stale, from when it went stale", () => {
    const s = run([
      [projected(100), 0],
      [projected(100, "a", true), 1000],
      [projected(100, "a", true), 5000],
    ]);
    expect(s.staleSince).toBe(1000);
    expect(s.projection?.stale).toBe(true);
    expect(run([[projected(100), 6000]], s).staleSince).toBeUndefined();
  });
});

describe("movedTo", () => {
  const found = (first: number, last: number) =>
    ({ kind: "found", first, tip: tip(last) }) as const;
  it("moves a later block up by as many blocks as were found", () => {
    expect(movedTo(found(101, 101), 3)).toBe(2);
    expect(movedTo(found(101, 103), 3)).toBe(0);
    expect(movedTo(found(101, 103), 2)).toBe("mined");
    expect(movedTo({ kind: "reorg", replaced: tip(1), tip: tip(1, "b") }, 3)).toBeUndefined();
  });
});
