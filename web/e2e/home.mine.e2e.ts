// Mines on the run's node, so it runs after every other test (the `mines` project).
import type { Page } from "@playwright/test";
import { expect, PROJECTION_LAG_MS, test } from "./fixtures";

const heights = (page: Page) =>
  page.$$eval(".home-strip__lane > li[data-height]:not(.home-tile--leaving)", (els) =>
    els.map((e) => Number((e as HTMLElement).dataset["height"])),
  );

/** The strip as drawn in one frame: mined tiles newest first, and the next projected tile's name. */
interface Frame {
  at: number;
  ids: string[];
  heights: number[];
  next: string;
}

/** Starts recording the strip at every animation frame, in the page's clock (epoch ms). */
const record = (page: Page) =>
  page.evaluate(() => {
    const frames: unknown[] = [];
    Object.assign(window, { stripFrames: frames });
    const read = () => {
      const lane = [
        ...document.querySelectorAll<HTMLElement>(
          ".home-strip__lane > li[data-id]:not(.home-tile--leaving)",
        ),
      ];
      const next = document.querySelector(
        ".home-strip__projected > li[data-key]:not(.home-tile--leaving) a",
      );
      frames.push({
        at: performance.timeOrigin + performance.now(),
        ids: lane.map((e) => e.dataset["id"] ?? ""),
        heights: lane.map((e) => Number(e.dataset["height"])),
        next: next?.getAttribute("aria-label") ?? "",
      });
      requestAnimationFrame(read);
    };
    requestAnimationFrame(read);
  });
const recorded = (page: Page) =>
  page.evaluate(() => (window as unknown as { stripFrames: Frame[] }).stripFrames);

/** Every frame shows each block once, newest first, ten of them. */
function eachOnce(frames: readonly Frame[]) {
  for (const f of frames) {
    expect(new Set(f.ids).size, `a block shown twice at ${f.at}`).toBe(f.ids.length);
    const ordered = f.heights.every((h, i) => i === 0 || h < (f.heights[i - 1] ?? 0));
    expect(ordered, `out of order at ${f.at}`).toBe(true);
    expect(f.ids).toHaveLength(10);
  }
}

/** When each `block` and `projected` message reached the page, in epoch ms. */
function listen(page: Page) {
  const heard: { at: number; topic: string; type: string; tip: string }[] = [];
  page.on("websocket", (ws) =>
    ws.on("framereceived", ({ payload }) => {
      if (typeof payload !== "string") return;
      const m = JSON.parse(payload) as {
        topic?: string;
        type?: string;
        block?: { id: string };
        tip?: { hash: string } | null;
      };
      if (m.type === "block" || m.type === "projected") {
        heard.push({
          at: Date.now(),
          topic: m.topic ?? "",
          type: m.type,
          tip: m.block?.id ?? m.tip?.hash ?? "",
        });
      }
    }),
  );
  return heard;
}

test("mining takes the projected next block across the line and empties the projection", async ({
  page,
  request,
  mine,
}) => {
  const heard = listen(page);
  await page.goto("/regtest/");
  const projected = page.getByRole("list", { name: "Projected blocks, next first" });
  const next = projected.getByRole("listitem").first();
  const nextLink = next.getByRole("link");
  // The projection trails the node by a cycle or so: wait until it holds the node's whole mempool.
  let waiting = 0;
  await expect(async () => {
    const node = (await (await request.get("/regtest/api/mempool")).json()) as { count: number };
    const label = (await nextLink.getAttribute("aria-label")) ?? "";
    waiting = Number(/projected: (\d+) transactions/.exec(label)?.[1]);
    expect(waiting).toBe(node.count);
  }).toPass({ timeout: PROJECTION_LAG_MS });
  const key = await next.getAttribute("data-key");
  await record(page);
  // A frame before the block, to compare with: WebKit on Linux can go a while without a frame
  // while nothing on the page moves.
  await expect.poll(async () => (await recorded(page)).length).toBeGreaterThan(0);

  const [hash = ""] = await mine(1);
  const lane = page.getByRole("list", { name: "Mined blocks, newest first" });
  const tile = lane.getByRole("link").first();
  await expect(tile).toHaveAttribute("href", `/regtest/block/${hash}`);
  // The block that was projected next: its height, and everything that waited, plus its coinbase.
  await expect(lane.getByRole("listitem").first()).toHaveAttribute("data-height", key ?? "");
  await expect(tile).toHaveAccessibleName(new RegExp(`, ${waiting + 1} transactions,`));
  // The projection on the block follows its `block` message at once (docs/WS.md).
  await expect(nextLink).toHaveAccessibleName("Next block, projected: nothing waiting");
  // A second more, so a flicker after it would be caught.
  await page.waitForTimeout(1_000);
  const frames = await recorded(page);
  eachOnce(frames);
  // From the frame the block lands in: it stays the newest, and the next tile never again holds
  // what it mined, nor anything at all.
  const landed = frames.findIndex((f) => f.ids[0] === hash);
  expect(landed).toBeGreaterThan(0);
  for (const f of frames.slice(landed)) {
    expect(f.ids[0]).toBe(hash);
    expect(f.next).toBe("Next block, projected: nothing waiting");
  }
  // It lands in the first frame after its `block` message, and the projection is on it by then.
  const blockAt = heard.findIndex((m) => m.type === "block" && m.tip === hash);
  const onItAt = heard.findIndex((m) => m.type === "projected" && m.tip === hash);
  expect(blockAt, "the block message").toBeGreaterThanOrEqual(0);
  expect(onItAt, "a projection on the block, after it").toBeGreaterThan(blockAt);
  const block = heard[blockAt];
  const onIt = heard[onItAt];
  const at = frames[landed]?.at ?? 0;
  test.info().annotations.push({
    type: "settled",
    description: `landed ${Math.round(at - (block?.at ?? 0))} ms after the block message; projection on it ${Math.round((onIt?.at ?? 0) - (block?.at ?? 0))} ms after`,
  });
  expect(at - (block?.at ?? 0)).toBeLessThan(500);
  // The mempool panel says the same once its own topic catches up, so look in the strip. A phone
  // hides the note to give the tiles room (HomePage.css).
  const note = page
    .getByRole("region", { name: "Blocks" })
    .getByText("Nothing is waiting.", { exact: false });
  await ((page.viewportSize()?.width ?? 0) > 640
    ? expect(note).toBeVisible()
    : expect(note).toBeAttached());
});

test("a new block lands in the strip without a reload, and a burst leaves it correct", async ({
  page,
  mine,
}) => {
  await page.goto("/regtest/");
  await expect(page.getByRole("status").filter({ hasText: "Live" })).toBeVisible();
  const lane = page.getByRole("list", { name: "Mined blocks, newest first" });
  await expect(lane.getByRole("listitem")).toHaveCount(10);
  const start = (await heights(page))[0] ?? 0;

  const [hash] = await mine(1);
  await expect(lane.getByRole("link").first()).toHaveAttribute("href", `/regtest/block/${hash}`);
  await expect(page.getByText(`Block ${start + 1} mined`)).toBeAttached();

  // Three at once, then two more straight after: the strip ends on the tip, ten in a row, and no
  // frame on the way shows a block twice or out of order.
  await record(page);
  await mine(3);
  const last = await mine(2);
  await expect(lane.getByRole("link").first()).toHaveAttribute(
    "href",
    `/regtest/block/${last.at(-1)}`,
  );
  await expect
    .poll(() => heights(page))
    .toEqual(Array.from({ length: 10 }, (_, i) => start + 6 - i));
  eachOnce(await recorded(page));
});

test("a new block's tile fills from the WebSocket's summary, not over HTTP", async ({
  page,
  mine,
}) => {
  await page.goto("/regtest/");
  await expect(page.getByRole("status").filter({ hasText: "Live" })).toBeVisible();
  const lane = page.getByRole("list", { name: "Mined blocks, newest first" });
  await expect(lane.getByRole("listitem")).toHaveCount(10);
  const asked: string[] = [];
  page.on("request", (r) => {
    if (r.url().includes("/app/blocks/summary")) asked.push(r.url());
  });
  const [hash = ""] = await mine(1);
  const tile = lane.getByRole("link").first();
  await expect(tile).toHaveAttribute("href", `/regtest/block/${hash}`);
  // Well before the page would fall back to asking over HTTP.
  await expect(tile).toHaveAccessibleName(/median fee|only the coinbase/, { timeout: 3_000 });
  // The push names the miner too: regtest has no pools, so Unknown, under the tile and in its name.
  await expect(tile).toHaveAccessibleName(/^Block \d+, mined by Unknown,/);
  await expect(lane.locator(".home-tile__pool").first()).toHaveText("Unknown");
  expect(asked.filter((u) => u.includes(hash))).toEqual([]);
});
