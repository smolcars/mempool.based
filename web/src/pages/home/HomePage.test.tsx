import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mount } from "../../components/test-utils";
import { clearApiCache } from "../../lib/api";
import type { Block } from "../../lib/esplora";
import { FakeSocket, fakeBlock } from "../../lib/fakes";
import HomePage from "./HomePage";

const chain = (from: number, to: number, branch = "a") =>
  Array.from({ length: to - from + 1 }, (_, i) => fakeBlock(to - i, branch));

const MEMPOOL = {
  count: 3,
  vsize: 1500,
  total_fee: 4200,
  fee_histogram: [
    [5, 500],
    [1, 500],
    [0.15, 500],
  ],
};
const FEES = { "1": 5.2, "3": 1.1, "6": 0.5, "144": 0.12 };
const RECENT = [{ txid: "c".repeat(64), fee: 300, vsize: 150, value: 12_345 }];

let replies: Record<string, () => Response>;
let blocksServed: Block[];
let requests: string[];

function serve() {
  vi.stubGlobal("fetch", async (url: string) => {
    const path = url.replace(/^\/regtest\/api/, "");
    requests.push(path);
    const reply = replies[path];
    if (reply) return reply();
    return Response.json({ error: "not found" }, { status: 404 });
  });
}

beforeEach(() => {
  clearApiCache();
  FakeSocket.all = [];
  vi.stubGlobal("WebSocket", FakeSocket);
  requests = [];
  blocksServed = chain(1, 10);
  replies = {
    "/blocks": () => Response.json(blocksServed),
    "/mempool": () => Response.json(MEMPOOL),
    "/fee-estimates": () => Response.json(FEES),
    "/mempool/recent": () => Response.json(RECENT),
  };
  serve();
});
afterEach(() => {
  vi.unstubAllGlobals();
});

const tileHeights = (el: HTMLElement) =>
  [...el.querySelectorAll<HTMLElement>(".home-strip__lane > li[data-height]")].map((li) =>
    Number(li.dataset.height),
  );
const blocksFetches = () => requests.filter((p) => p === "/blocks").length;

async function open() {
  const el = mount(() => <HomePage />, "/regtest/");
  await vi.waitFor(() => expect(tileHeights(el)).toHaveLength(10));
  const ws = FakeSocket.last();
  ws.hello();
  return { el, ws };
}

describe("home page", () => {
  it("shows skeletons shaped like the content while loading", () => {
    replies["/blocks"] = () => new Promise<Response>(() => {}) as unknown as Response;
    vi.stubGlobal("fetch", () => new Promise(() => {}));
    const el = mount(() => <HomePage />, "/regtest/");
    expect(el.querySelectorAll(".home-tile--skeleton")).toHaveLength(10);
    expect(el.querySelector(".home-stats")?.getAttribute("aria-busy")).toBe("true");
    expect(el.querySelectorAll(".home-recent tbody tr")).toHaveLength(10);
    expect(el.querySelector(".home-live")?.textContent).toContain("Connecting");
  });

  it("shows the blocks, the mempool, the node's estimates and the latest transactions", async () => {
    const { el } = await open();
    expect(tileHeights(el)).toEqual(chain(1, 10).map((b) => b.height));
    expect(el.querySelector(".home-tile--mined a")?.getAttribute("href")).toBe(
      `/regtest/block/${fakeBlock(10).id}`,
    );
    await vi.waitFor(() => expect(el.querySelector(".home-stats")?.textContent).toContain("3"));
    expect(el.querySelector(".home-stats")?.textContent).toContain("1\u202f500 vB");
    expect(el.querySelector(".home-stats")?.textContent).toContain("≈ 1 block");
    expect(el.querySelector(".home-ladder__list")?.textContent).toContain("5.20");
    expect(el.querySelector(".home-fees__note")?.textContent).toContain("node's own estimates");
    await vi.waitFor(() =>
      expect(el.querySelector(".home-recent tbody")?.textContent).toContain("cccccccc"),
    );
    expect(el.querySelector(".home-live")?.textContent).toContain("Live");
  });

  it("says what failed when the node is unavailable, section by section", async () => {
    for (const path of Object.keys(replies)) {
      replies[path] = () => Response.json({ error: "upstream unavailable" }, { status: 502 });
    }
    const el = mount(() => <HomePage />, "/regtest/");
    await vi.waitFor(() => expect(el.querySelectorAll('[role="alert"]')).toHaveLength(4));
    expect(el.textContent).toContain("Could not load the latest blocks");
    expect(el.textContent).toContain("Upstream unavailable.");
    expect(el.textContent).toContain("Could not load the mempool");
    expect(el.textContent).toContain("Could not load fee estimates");
  });

  it("shows an empty mempool as empty, not as missing", async () => {
    replies["/mempool"] = () =>
      Response.json({ count: 0, vsize: 0, total_fee: 0, fee_histogram: [] });
    replies["/fee-estimates"] = () => Response.json({});
    replies["/mempool/recent"] = () => Response.json([]);
    const { el } = await open();
    await vi.waitFor(() => expect(el.textContent).toContain("Nothing is waiting"));
    expect(el.textContent).toContain("The node has no fee estimates yet");
    expect(el.textContent).toContain("No transactions are waiting");
    expect(el.querySelector(".home-stats")?.textContent).toContain("empty");
  });

  it("says when the live connection is down, and keeps the strip moving over HTTP", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const { el, ws } = await open();
      ws.drop(1006);
      await vi.waitFor(() =>
        expect(el.querySelector(".home-live")?.textContent).toContain("Reconnecting"),
      );
      expect(el.querySelector(".home-live")?.textContent).toContain("Blocks refresh every 15 s");
      blocksServed = chain(2, 11);
      const before = blocksFetches();
      vi.advanceTimersByTime(15_000);
      await vi.waitFor(() => expect(tileHeights(el)[0]).toBe(11));
      expect(blocksFetches()).toBeGreaterThan(before);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("the strip, kept live", () => {
  it("adds each block of a burst as it arrives, without asking HTTP", async () => {
    const { el, ws } = await open();
    const fetched = blocksFetches();
    for (let h = 11; h <= 14; h++)
      ws.receive({ topic: "blocks", type: "block", block: fakeBlock(h) });
    expect(tileHeights(el)).toEqual(chain(5, 14).map((b) => b.height));
    expect(blocksFetches()).toBe(fetched);
  });

  it("refetches over HTTP on a gap", async () => {
    const { el, ws } = await open();
    blocksServed = chain(4, 13);
    ws.receive({ topic: "blocks", type: "block", block: fakeBlock(13) });
    await vi.waitFor(() => expect(tileHeights(el)[0]).toBe(13));
    expect(tileHeights(el)).toEqual(chain(4, 13).map((b) => b.height));
  });

  it("refetches on a resync whose tip differs, and not on one that matches", async () => {
    const { el, ws } = await open();
    const fetched = blocksFetches();
    ws.receive({ topic: "blocks", type: "resync", tip: { height: 10, hash: fakeBlock(10).id } });
    ws.receive({ topic: "blocks", type: "resync", tip: null });
    expect(blocksFetches()).toBe(fetched);
    blocksServed = chain(2, 11);
    ws.receive({ topic: "blocks", type: "resync", tip: { height: 11, hash: fakeBlock(11).id } });
    await vi.waitFor(() => expect(tileHeights(el)[0]).toBe(11));
  });

  it("keeps asking after a resync until HTTP has the announced block", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const { el, ws } = await open();
      // The server's cache still holds the list from before block 11, twice.
      let stale = 2;
      replies["/blocks"] = () => Response.json(stale-- > 0 ? chain(1, 10) : chain(2, 11));
      ws.receive({ topic: "blocks", type: "resync", tip: { height: 11, hash: fakeBlock(11).id } });
      await vi.waitFor(() => expect(blocksFetches()).toBeGreaterThanOrEqual(2));
      expect(tileHeights(el)[0]).toBe(10);
      vi.advanceTimersByTime(1_500);
      await vi.waitFor(() => expect(blocksFetches()).toBeGreaterThanOrEqual(3));
      vi.advanceTimersByTime(1_500);
      await vi.waitFor(() => expect(tileHeights(el)[0]).toBe(11));
      // Caught up: no more asking.
      const settled = blocksFetches();
      vi.advanceTimersByTime(10_000);
      expect(blocksFetches()).toBe(settled);
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps asking after a failed fetch for an announced block, but not forever", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const { el, ws } = await open();
      replies["/blocks"] = () => Response.json({ error: "upstream unavailable" }, { status: 502 });
      ws.receive({ topic: "blocks", type: "resync", tip: { height: 11, hash: fakeBlock(11).id } });
      // The first load, the resync's fetch, then eight retries.
      for (let i = 0; i < 12; i++) await vi.advanceTimersByTimeAsync(1_500);
      expect(blocksFetches()).toBe(10);
      // Under the socket's 30 s ping, which this fake server would leave unanswered.
      await vi.advanceTimersByTimeAsync(6_000);
      expect(blocksFetches()).toBe(10);

      // The next announcement starts afresh, even though the node still serves the old list.
      replies["/blocks"] = () => Response.json(chain(1, 10));
      ws.receive({ topic: "blocks", type: "resync", tip: { height: 12, hash: fakeBlock(12).id } });
      await vi.advanceTimersByTimeAsync(1_500);
      expect(blocksFetches()).toBe(12);
      replies["/blocks"] = () => Response.json(chain(3, 12));
      await vi.advanceTimersByTimeAsync(1_500);
      await vi.waitFor(() => expect(tileHeights(el)[0]).toBe(12));
    } finally {
      vi.useRealTimers();
    }
  });

  it("refetches after a reconnect", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const { el, ws } = await open();
      ws.drop(1001);
      blocksServed = chain(3, 12);
      vi.advanceTimersByTime(1000);
      FakeSocket.last().hello();
      await vi.waitFor(() => expect(tileHeights(el)[0]).toBe(12));
    } finally {
      vi.useRealTimers();
    }
  });

  it("follows a reorg: drops the replaced blocks, then takes the new branch", async () => {
    const { el, ws } = await open();
    ws.receive({ topic: "blocks", type: "reorg", fork_height: 8 });
    expect(tileHeights(el)).toEqual([8, 7, 6, 5, 4, 3, 2, 1]);
    ws.receive({ topic: "blocks", type: "block", block: fakeBlock(9, "b", "a") });
    ws.receive({ topic: "blocks", type: "block", block: fakeBlock(10, "b") });
    ws.receive({ topic: "blocks", type: "block", block: fakeBlock(11, "b") });
    const ids = [...el.querySelectorAll<HTMLElement>(".home-strip__lane > li[data-id]")].map(
      (li) => li.dataset.id,
    );
    expect(ids.slice(0, 3)).toEqual([
      fakeBlock(11, "b").id,
      fakeBlock(10, "b").id,
      fakeBlock(9, "b").id,
    ]);
    expect(tileHeights(el)).toEqual([11, 10, 9, 8, 7, 6, 5, 4, 3, 2]);
  });

  it("applies a block that arrives while the first list is loading, once it lands", async () => {
    let release: (() => void) | undefined;
    replies["/blocks"] = () => Response.json(chain(1, 10));
    vi.stubGlobal("fetch", async (url: string) => {
      const path = url.replace(/^\/regtest\/api/, "");
      requests.push(path);
      if (path === "/blocks") await new Promise<void>((r) => (release = r));
      return (replies[path] ?? (() => Response.json({}, { status: 404 })))();
    });
    const el = mount(() => <HomePage />, "/regtest/");
    const ws = FakeSocket.last();
    ws.hello();
    ws.receive({ topic: "blocks", type: "block", block: fakeBlock(11) });
    await vi.waitFor(() => expect(release).toBeDefined());
    release?.();
    await vi.waitFor(() => expect(tileHeights(el)[0]).toBe(11));
    expect(tileHeights(el)).toHaveLength(10);
  });

  it("announces a new block politely", async () => {
    const { el, ws } = await open();
    ws.receive({ topic: "blocks", type: "block", block: fakeBlock(11) });
    expect(el.querySelector('[aria-live="polite"]')?.textContent).toBe(
      "Block 11 mined: 1 transaction.",
    );
  });
});

describe("the live indicator", () => {
  it("says the node is not answering when the socket is up but the panels fail", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const { el } = await open();
      await vi.waitFor(() => expect(el.querySelector(".home-live")?.textContent).toBe("Live"));
      replies["/mempool/recent"] = () =>
        Response.json({ error: "upstream unavailable" }, { status: 502 });
      vi.advanceTimersByTime(5_000);
      await vi.waitFor(() =>
        expect(el.querySelector(".home-live")?.textContent).toContain("Node not answering"),
      );
      expect(el.querySelector(".home-live")?.textContent).toContain("Figures may be out of date.");
      replies["/mempool/recent"] = () => Response.json(RECENT);
      vi.advanceTimersByTime(5_000);
      await vi.waitFor(() => expect(el.querySelector(".home-live")?.textContent).toBe("Live"));
    } finally {
      vi.useRealTimers();
    }
  });
});
