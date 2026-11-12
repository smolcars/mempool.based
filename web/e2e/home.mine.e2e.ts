// Mines on the run's node, so it runs after every other test (the `mines` project).
import type { Page } from "@playwright/test";
import { expect, PROJECTION_LAG_MS, test } from "./fixtures";

const heights = (page: Page) =>
  page.$$eval(".home-strip__lane > li[data-height]:not(.home-tile--leaving)", (els) =>
    els.map((e) => Number((e as HTMLElement).dataset["height"])),
  );

test("mining takes the projected next block across the line and empties the projection", async ({
  page,
  request,
  mine,
}) => {
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

  const [hash] = await mine(1);
  const lane = page.getByRole("list", { name: "Mined blocks, newest first" });
  const tile = lane.getByRole("link").first();
  await expect(tile).toHaveAttribute("href", `/regtest/block/${hash}`);
  // The block that was projected next: its height, and everything that waited, plus its coinbase.
  await expect(lane.getByRole("listitem").first()).toHaveAttribute("data-height", key ?? "");
  await expect(tile).toHaveAccessibleName(new RegExp(`, ${waiting + 1} transactions,`));
  // The strip moves with the block; the projection empties when it catches up.
  await expect(nextLink).toHaveAccessibleName("Next block, projected: nothing waiting", {
    timeout: PROJECTION_LAG_MS,
  });
  // The mempool panel says the same once its own topic catches up, so look in the strip.
  await expect(
    page.getByRole("region", { name: "Blocks" }).getByText("Nothing is waiting.", { exact: false }),
  ).toBeVisible();
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

  // Three at once, then two more straight after: the strip ends on the tip, ten in a row.
  await mine(3);
  const last = await mine(2);
  await expect(lane.getByRole("link").first()).toHaveAttribute(
    "href",
    `/regtest/block/${last.at(-1)}`,
  );
  await expect
    .poll(() => heights(page))
    .toEqual(Array.from({ length: 10 }, (_, i) => start + 6 - i));
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
