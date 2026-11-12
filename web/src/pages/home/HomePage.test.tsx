import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mount } from "../../components/test-utils";
import { clearApiCache } from "../../lib/api";
import type { Block } from "../../lib/esplora";
import { FakeSocket, fakeBlock } from "../../lib/fakes";
import { type BlockSummary, clearSummaries } from "../../lib/summary";
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
/** The batch summary route; unset, it is a 404 and tiles keep their neutral gauge. */
let summaries: ((hashes: string[]) => Response | Promise<Response>) | undefined;
let blocksServed: Block[];
let requests: string[];

function serve() {
  vi.stubGlobal("fetch", async (url: string) => {
    const path = url.replace(/^\/(regtest|signet)\/api/, "");
    requests.push(path);
    const batch = /\/app\/blocks\/summary\?hashes=(.*)$/.exec(path);
    if (batch && summaries) return summaries(batch[1]?.split(",") ?? []);
    const reply = replies[path];
    if (reply) return reply();
    return Response.json({ error: "not found" }, { status: 404 });
  });
}

beforeEach(() => {
  clearApiCache();
  clearSummaries();
  summaries = undefined;
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
    Number(li.dataset["height"]),
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
    expect(tileHeights(el)).toStrictEqual(chain(1, 10).map((b) => b.height));
    expect(el.querySelector(".home-tile--mined a")?.getAttribute("href")).toBe(
      `/regtest/block/${fakeBlock(10).id}`,
    );
    await vi.waitFor(() => expect(el.querySelector(".home-stats")?.textContent).toContain("3"));
    expect(el.querySelector(".home-stats")?.textContent).toContain("1\u202F500 vB");
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
    expect(tileHeights(el)).toStrictEqual(chain(5, 14).map((b) => b.height));
    expect(blocksFetches()).toBe(fetched);
  });

  it("refetches over HTTP on a gap", async () => {
    const { el, ws } = await open();
    blocksServed = chain(4, 13);
    ws.receive({ topic: "blocks", type: "block", block: fakeBlock(13) });
    await vi.waitFor(() => expect(tileHeights(el)[0]).toBe(13));
    expect(tileHeights(el)).toStrictEqual(chain(4, 13).map((b) => b.height));
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
    expect(tileHeights(el)).toStrictEqual([8, 7, 6, 5, 4, 3, 2, 1]);
    ws.receive({ topic: "blocks", type: "block", block: fakeBlock(9, "b", "a") });
    ws.receive({ topic: "blocks", type: "block", block: fakeBlock(10, "b") });
    ws.receive({ topic: "blocks", type: "block", block: fakeBlock(11, "b") });
    const ids = [...el.querySelectorAll<HTMLElement>(".home-strip__lane > li[data-id]")].map(
      (li) => li.dataset["id"],
    );
    expect(ids.slice(0, 3)).toStrictEqual([
      fakeBlock(11, "b").id,
      fakeBlock(10, "b").id,
      fakeBlock(9, "b").id,
    ]);
    expect(tileHeights(el)).toStrictEqual([11, 10, 9, 8, 7, 6, 5, 4, 3, 2]);
  });

  it("applies a block that arrives while the first list is loading, once it lands", async () => {
    let release: (() => void) | undefined;
    replies["/blocks"] = () => Response.json(chain(1, 10));
    vi.stubGlobal("fetch", async (url: string) => {
      const path = url.replace(/^\/regtest\/api/, "");
      requests.push(path);
      if (path === "/blocks") {
        await new Promise<void>((resolve) => {
          release = resolve;
        });
      }
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

const facts = (id: string, over: Partial<BlockSummary> = {}): BlockSummary => ({
  id,
  height: 0,
  tx_count: 3,
  fees: 900,
  vsize: 400,
  weight: 1600,
  fee_rate: { min: 0.1, p10: 0.1, p25: 0.15, median: 1.234, p75: 3, p90: 4, max: 12.5 },
  fee_bands: [100, 0, 0, 0, 200, 0, 0, 0, 0, 100],
  ...over,
});
const tile = (el: HTMLElement, height: number) =>
  el.querySelector<HTMLElement>(`.home-strip__lane > li[data-height="${height}"]`);
const summaryRequests = () => requests.filter((p) => p.includes("/app/blocks/summary"));

describe("the tiles' fee facts", () => {
  it("loads every tile's facts in one request on first paint, and settles their layers in", async () => {
    summaries = (hashes) => Response.json(hashes.map((h) => facts(h)));
    const { el } = await open();
    await vi.waitFor(() =>
      expect(tile(el, 10)?.querySelectorAll(".strata__layer")).toHaveLength(3),
    );
    expect(summaryRequests()).toHaveLength(1);
    expect(summaryRequests()[0]?.split(",")).toHaveLength(10);
    const t = tile(el, 10) as HTMLElement;
    expect(t.querySelector(".home-tile__fee")?.textContent).toBe("1.23 sat/vB");
    expect(t.querySelector(".home-tile__range")?.textContent).toBe("0.10 – 12.5");
    expect(t.querySelector(".strata--settle")).not.toBeNull();
    // The gauge the layers settle over clears away.
    expect(t.querySelector(".home-tile__fill--gone")).not.toBeNull();
    expect(t.querySelector("a")?.getAttribute("aria-label")).toContain(
      "median fee 1.23 sat/vB, fee range 0.10 to 12.5 sat/vB",
    );
    // The median's swatch is the color of a band the strata draw.
    const swatch = t.querySelector<HTMLElement>(".fee__swatch")?.style.getPropertyValue("--swatch");
    const layers = [...t.querySelectorAll<HTMLElement>(".strata__layer")];
    expect(layers.map((l) => l.style.getPropertyValue("--c"))).toContain(swatch);
  });

  it("shows the neutral gauge and loading lines while the facts are on their way", async () => {
    summaries = () => new Promise<Response>(() => {});
    const { el } = await open();
    const t = tile(el, 10) as HTMLElement;
    expect(t.querySelector(".home-tile__fill")).not.toBeNull();
    expect(t.querySelectorAll(".home-tile__bone--line")).toHaveLength(2);
    expect(t.querySelector(".strata")).toBeNull();
    expect(t.querySelector("a")?.getAttribute("aria-label")).not.toContain("fee");
  });

  it("says plainly when a block holds only its coinbase", async () => {
    summaries = (hashes) =>
      Response.json(
        hashes.map((h) =>
          facts(h, { fee_rate: null, fee_bands: Array.from({ length: 10 }, () => 0) }),
        ),
      );
    const { el } = await open();
    const t = tile(el, 10) as HTMLElement;
    await vi.waitFor(() => expect(t.querySelector(".home-tile__fee")?.textContent).toBe("No fees"));
    expect(t.querySelector(".home-tile__range")?.textContent).toBe("Coinbase only");
    expect(t.querySelector(".strata")).toBeNull();
    expect(t.querySelector(".home-tile__fill:not(.home-tile__fill--gone)")).not.toBeNull();
    expect(t.querySelector("a")?.getAttribute("aria-label")).toContain(
      "only the coinbase, no fees",
    );
  });

  it("keeps the gauge and the block's size when the facts cannot be had", async () => {
    const { el } = await open();
    const t = tile(el, 10) as HTMLElement;
    await vi.waitFor(() => expect(t.querySelector(".home-tile__fee")?.textContent).toBe("250 vB"));
    expect(t.querySelector(".home-tile__range")?.textContent).toBe("under 1% full");
  });

  it("fills a new block's tile from the WebSocket, without asking HTTP", async () => {
    summaries = (hashes) => Response.json(hashes.map((h) => facts(h)));
    const { el, ws } = await open();
    await vi.waitFor(() => expect(summaryRequests()).toHaveLength(1));
    ws.receive({ topic: "blocks", type: "block", block: fakeBlock(11) });
    const t = tile(el, 11) as HTMLElement;
    expect(t.querySelectorAll(".home-tile__bone--line")).toHaveLength(2);
    ws.receive({
      topic: "blocks",
      type: "summary",
      summary: facts(fakeBlock(11).id, {
        fee_rate: { min: 0.1, p10: 0.1, p25: 0.2, median: 0.5, p75: 1, p90: 2, max: 3 },
      }),
    });
    expect(t.querySelector(".home-tile__fee")?.textContent).toBe("0.50 sat/vB");
    expect(t.querySelector(".strata--settle")).not.toBeNull();
    // A malformed push is ignored, and a push for a block not shown is kept for later.
    ws.receive({ topic: "blocks", type: "summary", summary: { id: fakeBlock(10).id } });
    expect(summaryRequests()).toHaveLength(1);
  });

  it("asks HTTP for a new block's facts when the push does not come", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      summaries = (hashes) => Response.json(hashes.map((h) => facts(h)));
      const { el, ws } = await open();
      await vi.waitFor(() => expect(summaryRequests()).toHaveLength(1));
      ws.receive({ topic: "blocks", type: "block", block: fakeBlock(11) });
      vi.advanceTimersByTime(3_000);
      expect(summaryRequests()).toHaveLength(1);
      vi.advanceTimersByTime(1_500);
      await vi.waitFor(() => expect(summaryRequests()).toHaveLength(2));
      expect(summaryRequests()[1]).toContain(fakeBlock(11).id);
      await vi.waitFor(() =>
        expect(tile(el, 11)?.querySelector(".home-tile__fee")?.textContent).toBe("1.23 sat/vB"),
      );
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("the tiles' fee facts, racing", () => {
  it("keeps a pushed summary when the HTTP fallback for it fails afterwards", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      let failFallback: (r: Response) => void = () => {};
      summaries = (hashes) =>
        hashes.includes(fakeBlock(11).id)
          ? new Promise<Response>((resolve) => {
              failFallback = resolve;
            })
          : Response.json(hashes.map((h) => facts(h)));
      const { el, ws } = await open();
      await vi.waitFor(() => expect(summaryRequests()).toHaveLength(1));
      ws.receive({ topic: "blocks", type: "block", block: fakeBlock(11) });
      vi.advanceTimersByTime(4_500);
      await vi.waitFor(() => expect(summaryRequests()).toHaveLength(2));
      ws.receive({ topic: "blocks", type: "summary", summary: facts(fakeBlock(11).id) });
      failFallback(Response.json({ error: "upstream failed" }, { status: 502 }));
      const t = tile(el, 11) as HTMLElement;
      await vi.waitFor(() =>
        expect(t.querySelector(".home-tile__fee")?.textContent).toBe("1.23 sat/vB"),
      );
      await vi.advanceTimersByTimeAsync(100);
      expect(t.querySelector(".home-tile__fee")?.textContent).toBe("1.23 sat/vB");
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("the difficulty adjustment", () => {
  const ADJUSTMENT = {
    progressPercent: 89.0377,
    difficultyChange: -7.046,
    estimatedRetargetDate: Date.now() + 39 * 3_600_000 + 30 * 60_000 + 30_000,
    remainingBlocks: 221,
    remainingTime: 1,
    previousRetarget: 5.539,
    previousTime: 1,
    nextRetargetHeight: 975_744,
    timeAvg: 1,
    adjustedTimeAvg: 1,
    timeOffset: 0,
    expectedBlocks: 1932.47,
  };

  it("says regtest never adjusts, rather than show a change of zero", async () => {
    const { el } = await open();
    expect(el.querySelector(".home-difficulty")?.textContent).toContain(
      "Regtest never adjusts its difficulty",
    );
    expect(requests.some((p) => p.includes("difficulty"))).toBe(false);
  });

  it("shows the period, the expected change, when, and the previous change", async () => {
    replies["/v1/difficulty-adjustment"] = () => Response.json(ADJUSTMENT);
    const el = mount(() => <HomePage />, "/signet/");
    const panel = () => el.querySelector(".home-difficulty")?.textContent ?? "";
    await vi.waitFor(() => expect(panel()).toContain("Down 7.05%"));
    expect(panel()).toContain("1\u202F795 of 2\u202F016 blocks");
    expect(panel()).toContain("137 blocks behind");
    expect(panel()).toContain("at block 975\u202F744");
    expect(panel()).toContain("in 39 h 30 min");
    expect(panel()).toContain("Up 5.54%");
  });

  it("asks again while its answer is as of an older tip than the strip shows", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const stamp = (hash: string) => ({
        "x-bitcoin-chain-tip": hash,
        "x-bitcoin-chain-tip-height": "9",
      });
      let n = 0;
      replies["/v1/difficulty-adjustment"] = () =>
        Response.json(ADJUSTMENT, {
          headers: stamp(n++ === 0 ? "ab".repeat(32) : fakeBlock(10).id),
        });
      mount(() => <HomePage />, "/signet/");
      await vi.waitFor(() => expect(n).toBe(1));
      vi.advanceTimersByTime(1_600);
      await vi.waitFor(() => expect(n).toBe(2));
      // Caught up with the strip's tip: no more asking.
      vi.advanceTimersByTime(10_000);
      expect(n).toBe(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it("waits out the server's 503 before its first answer without showing an error", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      let n = 0;
      replies["/v1/difficulty-adjustment"] = () =>
        n++ === 0
          ? Response.json(
              { error: "not ready yet; retry" },
              { status: 503, headers: { "retry-after": "1" } },
            )
          : Response.json(ADJUSTMENT);
      const el = mount(() => <HomePage />, "/signet/");
      const panel = () => el.querySelector(".home-difficulty");
      await vi.waitFor(() => expect(n).toBe(1));
      expect(panel()?.querySelector('[role="alert"]')).toBeNull();
      expect(panel()?.querySelector(".home-difficulty__stats")?.getAttribute("aria-busy")).toBe(
        "true",
      );
      vi.advanceTimersByTime(1_300);
      await vi.waitFor(() => expect(panel()?.textContent).toContain("Down 7.05%"));
    } finally {
      vi.useRealTimers();
    }
  });
});
