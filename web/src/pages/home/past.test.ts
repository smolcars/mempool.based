import { createRoot, createSignal, flush } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearApiCache } from "../../lib/api";
import type { Block } from "../../lib/esplora";
import { fakeBlock } from "../../lib/fakes";
import { createPastBlocks, inView, laneSlots, nextPage, PAGE, type Want } from "./past";

const chain = (from: number, to: number, branch = "a") =>
  Array.from({ length: to - from + 1 }, (_, i) => fakeBlock(to - i, branch));

describe("which tiles are in view", () => {
  it("counts tiles from the tip by the lane's pitch", () => {
    // The lane starts 600 px in; tiles are 144 px apart; the view is 1,000 px wide.
    expect(inView(0, 1000, 600, 144)).toStrictEqual([0, 2]);
    expect(inView(600 + 144 * 2000, 1000, 600, 144)).toStrictEqual([2000, 2006]);
  });

  it("says nothing before layout", () => {
    expect(inView(0, 0, 0, 0)).toBeUndefined();
    expect(inView(0, 390, 0, 0)).toBeUndefined();
  });
});

describe("the lane's items", () => {
  const blocks = new Map(chain(0, 1000).map((b) => [b.height, b]));
  const base = {
    tip: 1000,
    count: 1001,
    capped: false,
    pinned: [1000],
    block: (h: number) => blocks.get(h),
    failure: () => undefined,
  };

  it("holds the range asked for, each after the empty slots before it", () => {
    const items = laneSlots({ ...base, pinned: [], from: 0, to: 9 });
    expect(items.map((s) => s.index)).toStrictEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    expect(items.every((s) => s.skip === 0)).toBe(true);
  });

  it("keeps the newest and a focused tile wherever the view is, and nothing between", () => {
    const items = laneSlots({ ...base, pinned: [1000, 400], from: 400, to: 410 });
    const indices = items.map((s) => s.index);
    expect(indices).toStrictEqual([0, ...Array.from({ length: 11 }, (_, i) => 400 + i), 600]);
    expect(items[1]?.skip).toBe(399);
    expect(items.at(-1)?.skip).toBe(189);
  });

  it("puts a placeholder where a block is not loaded yet, and one item for a failed page", () => {
    const failure = (h: number) => (h < 990 && h >= 975 ? new Error("down") : undefined);
    const block = (h: number) => (h > 995 ? blocks.get(h) : undefined);
    const items = laneSlots({ ...base, pinned: [], from: 0, to: 30, block, failure });
    expect(items.slice(0, 5).every((s) => s.kind === "block")).toBe(true);
    expect(items.slice(5, 11).every((s) => s.kind === "wait")).toBe(true);
    const failed = items.find((s) => s.kind === "failed");
    expect(failed).toMatchObject({
      index: 11,
      top: 989,
      bottom: 975,
      span: 15,
      key: `f${975 / PAGE}`,
    });
    // The slot after the run follows it directly.
    const after = items[items.indexOf(failed as (typeof items)[number]) + 1];
    expect(after).toMatchObject({ index: 26, skip: 0, kind: "wait" });
  });

  it("ends at block 0, and says where older blocks are when it stops short of it", () => {
    const end = laneSlots({ ...base, pinned: [], from: 995, to: 1010 });
    expect(end.at(-1)).toMatchObject({ index: 1000, kind: "block" });
    const capped = laneSlots({ ...base, count: 500, capped: true, pinned: [], from: 495, to: 510 });
    expect(capped.at(-1)).toMatchObject({ kind: "end", index: 500, skip: 0 });
  });
});

describe("which page loads next", () => {
  const want = (top: number, bottom: number, middle: number, tip = 1000): Want => ({
    top,
    bottom,
    middle,
    tip,
  });

  it("asks for the page nearest the view, from its top, so the same request serves everyone", () => {
    // Heights 991 to 1000 are the live list's: the top page is asked for from the tip.
    expect(
      nextPage(
        want(1000, 950, 995),
        (h) => h >= 991,
        () => false,
      ),
    ).toStrictEqual({
      top: 1000,
      missing: 990,
    });
    // An earlier answer spilled into the next page: it is still asked for from its own top.
    expect(
      nextPage(
        want(1000, 950, 985),
        (h) => h >= 986,
        () => false,
      ),
    ).toStrictEqual({
      top: 989,
      missing: 985,
    });
    // Deep in the past, the page around the middle of the view first.
    expect(
      nextPage(
        want(520, 470, 500),
        () => false,
        () => false,
      ),
    ).toStrictEqual({
      top: 509,
      missing: 509,
    });
  });

  it("skips failed pages, stops at block 0, and asks nothing when all is here", () => {
    expect(
      nextPage(
        want(20, -30, 5, 20),
        (h) => h > 14,
        () => false,
      ),
    ).toStrictEqual({
      top: 14,
      missing: 14,
    });
    expect(
      nextPage(
        want(20, -30, 5, 20),
        (h) => h > 14,
        (p) => p === 0,
      ),
    ).toBeUndefined();
    expect(
      nextPage(
        want(1000, 950, 985),
        () => true,
        () => false,
      ),
    ).toBeUndefined();
  });
});

describe("the strip's past, loaded", () => {
  let requests: string[];
  let reply: (path: string) => Response;
  beforeEach(() => {
    clearApiCache();
    requests = [];
    reply = (path) => {
      const m = /\/api\/(?:v1\/)?blocks\/(\d+)$/.exec(path);
      const start = Number(m?.[1]);
      const n = path.includes("/v1/") ? 15 : 10;
      return Response.json(chain(Math.max(0, start - n + 1), start));
    };
    vi.stubGlobal("fetch", async (url: string) => {
      const path = url.replace(/^\/regtest/, "");
      requests.push(path);
      return reply(path);
    });
  });
  afterEach(() => vi.unstubAllGlobals());

  function open(live: Block[], view: Want | undefined) {
    return createRoot((dispose) => {
      const [list, setList] = createSignal<Block[] | undefined>(live);
      const [want, setWant] = createSignal<Want | undefined>(view);
      const past = createPastBlocks("regtest", list, want, () => undefined);
      return { past, setList, setWant, dispose };
    });
  }
  const settle = () => vi.waitFor(() => expect(requests.length).toBeGreaterThan(0));

  it("loads a page at a time, ahead of the view, from the route with extras", async () => {
    const s = open(chain(991, 1000), { top: 1000, bottom: 960, middle: 995, tip: 1000 });
    await settle();
    await vi.waitFor(() => expect(s.past.block(960)).toBeDefined());
    expect(requests).toStrictEqual([
      "/api/v1/blocks/1000",
      "/api/v1/blocks/989",
      "/api/v1/blocks/974",
    ]);
    // Live blocks are kept too, so one that leaves the live list stays.
    expect(s.past.block(995)?.id).toBe(fakeBlock(995).id);
    s.dispose();
  });

  it("asks the node's own pages when the server has no block table, and keeps asking them", async () => {
    reply = (path) =>
      path.includes("/v1/")
        ? Response.json(
            { error: "block extras need a block table on this server" },
            { status: 503 },
          )
        : Response.json(
            chain(Math.max(0, Number(path.split("/").at(-1)) - 9), Number(path.split("/").at(-1))),
          );
    const s = open(chain(991, 1000), { top: 1000, bottom: 975, middle: 995, tip: 1000 });
    await vi.waitFor(() => expect(s.past.block(975)).toBeDefined());
    expect(requests).toStrictEqual(["/api/v1/blocks/1000", "/api/blocks/990", "/api/blocks/980"]);
    s.dispose();
  });

  it("says which page failed, and asks again on retry", async () => {
    reply = () => Response.json({ error: "upstream unavailable" }, { status: 502 });
    const s = open(chain(991, 1000), { top: 1000, bottom: 976, middle: 995, tip: 1000 });
    await vi.waitFor(() => expect(s.past.failure(980)).toBeDefined());
    expect(s.past.failure(995)).toBeDefined();
    expect(s.past.failure(974)).toBeUndefined();
    const asked = requests.length;
    reply = (path) => Response.json(chain(976, Number(path.split("/").at(-1))));
    s.past.retry();
    await vi.waitFor(() => expect(s.past.block(976)).toBeDefined());
    expect(requests.length).toBeGreaterThan(asked);
    expect(s.past.failure(980)).toBeUndefined();
    s.dispose();
  });

  it("follows a new block, and drops what a reorg below the live list replaced", async () => {
    const s = open(chain(991, 1000), { top: 1000, bottom: 975, middle: 995, tip: 1000 });
    await vi.waitFor(() => expect(s.past.block(975)).toBeDefined());
    // A new block: the one it pushes off the live list is still here.
    s.setList(chain(992, 1001));
    flush();
    expect(s.past.block(991)?.id).toBe(fakeBlock(991).id);
    // A reorg from 985: the new live list's oldest does not build on what is kept below it.
    s.setWant(undefined);
    s.setList(chain(992, 1001, "b"));
    flush();
    expect(s.past.block(991)).toBeUndefined();
    expect(s.past.block(980)).toBeUndefined();
    s.dispose();
  });

  it("keeps no page asked for before a deep reorg", async () => {
    let answer: (r: Response) => void = () => undefined;
    reply = () => Response.json([]);
    vi.stubGlobal("fetch", async (url: string) => {
      requests.push(url);
      return new Promise<Response>((resolve) => {
        answer = resolve;
      });
    });
    const s = open(chain(991, 1000), { top: 1000, bottom: 975, middle: 995, tip: 1000 });
    await vi.waitFor(() => expect(requests).toHaveLength(1));
    // Block 990 is kept, then a reorg from 980 replaces it while the next page is on its way.
    answer(Response.json(chain(986, 1000)));
    await vi.waitFor(() => expect(requests).toHaveLength(2));
    // Block 992 of branch b does not build on the 991 kept: a reorg below the live list.
    s.setList(chain(992, 1001, "b"));
    flush();
    answer(Response.json(chain(975, 989)));
    await vi.waitFor(() => expect(requests.length).toBeGreaterThan(2));
    expect(s.past.block(980)).toBeUndefined();
    s.dispose();
  });

  it("lets go of blocks as the chain grows at now", () => {
    const s = createRoot((dispose) => {
      const [list, setList] = createSignal<Block[] | undefined>(chain(991, 1000));
      const [want, setWant] = createSignal<Want | undefined>(undefined);
      const past = createPastBlocks("regtest", list, want, () => undefined);
      return { past, setList, setWant, dispose };
    });
    for (let tip = 1001; tip <= 1300; tip++) {
      s.setWant({ top: tip, bottom: tip - 40, middle: tip - 5, tip });
      s.setList(chain(tip - 9, tip));
      flush();
    }
    expect(s.past.block(1000)).toBeUndefined();
    expect(s.past.block(1290)?.height).toBe(1290);
    expect(s.past.block(1200)?.height).toBe(1200);
    s.dispose();
  });

  it("keeps a focused tile's neighbours and the block under the live list, far from the view", async () => {
    const s = createRoot((dispose) => {
      const [want, setWant] = createSignal<Want | undefined>({
        top: 1000,
        bottom: 970,
        middle: 995,
        tip: 1000,
      });
      const past = createPastBlocks(
        "regtest",
        () => chain(991, 1000),
        want,
        () => 505,
      );
      return { past, setWant, dispose };
    });
    await vi.waitFor(() => expect(s.past.block(975)).toBeDefined());
    s.setWant({ top: 520, bottom: 490, middle: 505, tip: 1000 });
    await vi.waitFor(() => expect(s.past.block(490)).toBeDefined());
    s.setWant({ top: 220, bottom: 190, middle: 205, tip: 1000 });
    await vi.waitFor(() => expect(s.past.block(190)).toBeDefined());
    expect([504, 505, 506].map((h) => s.past.block(h)?.height)).toStrictEqual([504, 505, 506]);
    expect(s.past.block(500)).toBeUndefined();
    expect(s.past.block(990)?.height).toBe(990);
    expect(s.past.block(980)).toBeUndefined();
    s.dispose();
  });

  it("asks nothing below block 0", async () => {
    const s = open(chain(1, 10), { top: 10, bottom: -40, middle: 0, tip: 10 });
    await vi.waitFor(() => expect(s.past.block(0)).toBeDefined());
    expect(requests).toStrictEqual(["/api/v1/blocks/10"]);
    s.dispose();
  });
});
