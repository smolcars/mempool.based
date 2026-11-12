// Mines on the run's node, so it runs after every other test (the `mines` project).
import type { Page } from "@playwright/test";
import { expect, test } from "./fixtures";

const heights = (page: Page) =>
  page.$$eval(".home-strip__lane > li[data-height]:not(.home-tile--leaving)", (els) =>
    els.map((e) => Number((e as HTMLElement).dataset["height"])),
  );

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
