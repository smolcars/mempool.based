import { flush } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mount } from "../../components/test-utils";
import { clearApiCache } from "../../lib/api";
import MempoolPage from "./MempoolPage";
import { history, point, T0 } from "./testdata";

type Reply = () => Response;
let replies: Record<string, Reply>;
let requests: string[];

beforeEach(() => {
  clearApiCache();
  requests = [];
  replies = {};
  vi.stubGlobal("fetch", async (url: string) => {
    const path = url.replace(/^\/(regtest|signet|testnet4)/, "");
    requests.push(path);
    const reply = replies[path];
    return reply ? reply() : Response.json({ error: "not found" }, { status: 404 });
  });
  // happy-dom lays nothing out: give the chart a width to draw in.
  vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(800);
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const text = (el: HTMLElement) => el.textContent ?? "";
const answer = (h: ReturnType<typeof history>) => () => Response.json(h);
/** The size chart: the first slider on the page. */
const sizeChart = (el: HTMLElement) =>
  el.querySelector<HTMLElement>("[role=slider][aria-label^='The mempool']");
const drawn = async (el: HTMLElement) => {
  await vi.waitFor(() => expect(sizeChart(el)?.querySelector("svg")).toBeInstanceOf(SVGSVGElement));
  return sizeChart(el) as HTMLElement;
};
const subpaths = (d: string | null | undefined) => (d ?? "").match(/M/g)?.length ?? 0;

describe("mempool history page", () => {
  it("takes the interval from the address, asks for it and marks it current", async () => {
    replies["/app/mempool/history/1w"] = answer(history({ interval: "1w", step: 600 }));
    const el = mount(() => <MempoolPage />, "/mempool?interval=1w");
    await vi.waitFor(() => expect(requests).toContain("/app/mempool/history/1w"));
    expect(el.querySelector("[aria-current=page]")?.textContent).toBe("1 w: the last week");
    expect(el.querySelector("a[href='/mempool?interval=2h']")).not.toBeNull();
    expect(el.querySelector("a[href='/mempool?interval=all']")).not.toBeNull();
  });

  it("asks for the last 24 hours when the address names no interval, on its own network", async () => {
    mount(() => <MempoolPage />, "/signet/mempool?interval=nonsense");
    await vi.waitFor(() => expect(requests).toContain("/app/mempool/history/24h"));
  });

  it("bridges a few missing minutes, and breaks the line where recording paused longer", async () => {
    // Five missing minutes (T0+120 to T0+360), the most bridged, are drawn across; ten (T0+540 to
    // T0+1080) break it.
    replies["/app/mempool/history/2h"] = answer(
      history({
        points: [point(T0), point(T0 + 60), point(T0 + 420), point(T0 + 480), point(T0 + 1140)],
      }),
    );
    const el = mount(() => <MempoolPage />, "/mempool?interval=2h");
    const chart = await drawn(el);
    const total = chart.querySelector(".chart__line")?.getAttribute("d");
    expect(subpaths(total)).toBe(2);
    // Every fee band breaks at the same place; the empty ones draw nothing worth seeing.
    const layers = [...chart.querySelectorAll(".chart__layer")];
    expect(layers).toHaveLength(10);
    expect(subpaths(layers[4]?.getAttribute("d"))).toBe(2);
    expect(layers[4]?.getAttribute("style")).toContain("--fill: var(--fee-5)");
  });

  it("shows a period with a single point", async () => {
    replies["/app/mempool/history/24h"] = answer(
      history({
        interval: "24h",
        complete: false,
        sampled: 1,
        minutes: 1440,
        first: T0 + 3600,
        points: [point(T0 + 3600)],
      }),
    );
    // A period the reader chose keeps its whole width, the time before the records hatched.
    const el = mount(() => <MempoolPage />, "/mempool?interval=24h");
    const chart = await drawn(el);
    // A lone point is a short tick on the line and a sliver in its layers, not nothing.
    expect(chart.querySelector(".chart__line")?.getAttribute("d")).toContain("h0.1");
    expect(chart.querySelectorAll(".chart__layer")[4]?.getAttribute("d")).toMatch(/^M.*Z$/);
    expect(text(el)).toContain(
      "This server has recorded the mempool since 12 Nov 2026, 15:00 UTC, so this view holds 1 minute of the 1 440 minutes in the last 24 hours (under 1%).",
    );
    expect(text(el)).toContain("No data stored");
  });

  it("opens on what is recorded when the reader chose no period, and says so", async () => {
    replies["/app/mempool/history/24h"] = answer(
      history({
        interval: "24h",
        complete: false,
        sampled: 1,
        minutes: 1440,
        first: T0 + 3600,
        points: [point(T0 + 3600)],
      }),
    );
    const el = mount(() => <MempoolPage />, "/mempool");
    await drawn(el);
    expect(text(el)).toContain("Everything recorded, since 12 Nov 2026, 15:00 UTC.");
    expect(text(el)).not.toContain("No data stored");
    // The choice marked is "Recorded", not "24 h", which means the whole day.
    expect(el.querySelector(".mempool-intervals [aria-current=page]")?.textContent).toBe(
      "Recorded: since this server began recording",
    );
  });

  it("says when nothing has been recorded at all", async () => {
    replies["/app/mempool/history/2h"] = answer(
      history({ first: null, points: [], sampled: 0, complete: false }),
    );
    const el = mount(() => <MempoolPage />, "/mempool?interval=2h");
    await vi.waitFor(() => expect(text(el)).toContain("Nothing recorded yet"));
    expect(el.querySelector("[role=slider]")).toBeNull();
  });

  it("says when the period holds nothing but older records exist, and offers them", async () => {
    replies["/app/mempool/history/2h"] = answer(
      history({ first: T0 - 86_400, points: [], sampled: 0, complete: false }),
    );
    const el = mount(() => <MempoolPage />, "/mempool?interval=2h");
    await vi.waitFor(() => expect(text(el)).toContain("Nothing recorded in this period"));
    expect(text(el)).toContain("Its records begin 11 Nov 2026, 14:00 UTC.");
    expect(el.querySelector(".state a[href='/mempool?interval=all']")?.textContent).toBe(
      "See all of them",
    );
  });

  it("explains a server without a data directory, without retrying it", async () => {
    replies["/app/mempool/history/24h"] = () =>
      Response.json(
        { error: "mempool history needs a data directory on this server" },
        { status: 503, headers: { "retry-after": "1" } },
      );
    const el = mount(() => <MempoolPage />, "/mempool");
    await vi.waitFor(() => expect(text(el)).toContain("This server keeps no mempool history"));
    expect(requests.filter((r) => r.includes("/history/"))).toHaveLength(1);
  });

  it("reads a moment's figures by keyboard", async () => {
    replies["/app/mempool/history/2h"] = answer(
      history({ points: [point(T0), point(T0 + 60, { count: 2345, vsize: 610_000 })] }),
    );
    const el = mount(() => <MempoolPage />, "/mempool?interval=2h");
    const chart = await drawn(el);
    // The latest point by default, with the size, count, floor and every band.
    const now = chart.getAttribute("aria-valuetext") ?? "";
    expect(now).toMatch(/^12 Nov 2026, 14:01 UTC: Total 610\.00 kvB, Transactions 2 345/);
    expect(now).toContain("Lowest accepted 0.10 sat/vB");
    expect(now).toContain("under 0.2 sat/vB 100.00 kvB");
    expect(now).toContain("10+ sat/vB 0 vB");
    chart.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true }));
    flush();
    expect(chart.getAttribute("aria-valuetext")).toMatch(/^12 Nov 2026, 14:00 UTC: Total 550\.00/);
    const readout = el.querySelector(".chart__readout");
    expect(readout?.textContent).toContain("14:00 UTC");
    chart.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    flush();
    expect(chart.getAttribute("aria-valuetext")).toContain("14:01 UTC");
  });

  it("reads a moment's figures under the pointer or a finger", async () => {
    replies["/app/mempool/history/2h"] = answer(
      history({ points: [point(T0), point(T0 + 3600, { count: 9 })] }),
    );
    const el = mount(() => <MempoolPage />, "/mempool?interval=2h");
    const chart = await drawn(el);
    const svg = chart.querySelector("svg") as SVGSVGElement;
    // The plot starts 66 px in, and two hours span the rest: the first point is at the left edge.
    svg.dispatchEvent(
      new PointerEvent("pointerdown", {
        clientX: 70,
        clientY: 100,
        bubbles: true,
        pointerType: "touch",
      }),
    );
    flush();
    expect(chart.getAttribute("aria-valuetext")).toMatch(/^12 Nov 2026, 14:00 UTC/);
    // A finger lifting keeps the reading, as a tap shows it.
    svg.dispatchEvent(new PointerEvent("pointerleave", { bubbles: true, pointerType: "touch" }));
    flush();
    expect(chart.getAttribute("aria-valuetext")).toMatch(/^12 Nov 2026, 14:00 UTC/);
  });

  it("asks again just after each new minute is recorded", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "Date"] });
    vi.setSystemTime(T0 * 1000 + 30_000);
    replies["/app/mempool/history/2h"] = answer(history());
    mount(() => <MempoolPage />, "/mempool?interval=2h");
    await vi.waitFor(() => expect(requests).toHaveLength(1));
    await vi.advanceTimersByTimeAsync(32_000);
    expect(requests).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(2000);
    expect(requests).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(requests).toHaveLength(3);
  });

  it("lets an ask still in flight finish rather than starting it again each minute", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "Date"] });
    vi.setSystemTime(T0 * 1000);
    let signal: AbortSignal | undefined;
    vi.stubGlobal("fetch", (url: string, init: RequestInit) => {
      requests.push(url);
      signal = init.signal ?? undefined;
      // A server that never answers.
      return new Promise<Response>(() => {
        signal?.throwIfAborted();
      });
    });
    mount(() => <MempoolPage />, "/mempool?interval=2h");
    await vi.waitFor(() => expect(requests).toHaveLength(1));
    await vi.advanceTimersByTimeAsync(130_000);
    expect(requests).toHaveLength(1);
    expect(signal?.aborted).toBe(false);
  });
});
