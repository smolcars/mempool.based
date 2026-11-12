// Mines on the run's node, so it runs after every other test (the `mines` project).
import { FOUND_WAIT_MS } from "../src/pages/projected/state";
import { expect, PROJECTION_LAG_MS, test } from "./fixtures";

test("the next block's page says when the block is found, offers it, and carries on", async ({
  page,
  request,
  mine,
}) => {
  // Earlier tests in this project mined, and the projection trails the node. Start from a page that
  // opened on the caught-up projection, so their blocks are not told as news with this one.
  const before = Number(await (await request.get("/regtest/api/blocks/tip/height")).text()) + 1;
  const lede = page.getByText(`If it were mined now, block ${before}`);
  await page.goto("/regtest/block/next");
  await expect(lede).toBeVisible({ timeout: PROJECTION_LAG_MS });
  await page.reload();
  await expect(page.getByText("Live", { exact: true })).toBeVisible();
  await expect(lede).toBeVisible();

  const [hash] = await mine(1);
  // The page hears of the block with the projection, then holds the notice until the block's new
  // picture arrives, at most FOUND_WAIT_MS. The title; the live region's sentence ends "found.".
  await expect(page.getByText(new RegExp(`^Block ${before} was found\\s`))).toBeVisible({
    timeout: PROJECTION_LAG_MS + FOUND_WAIT_MS,
  });
  await expect(page.getByRole("link", { name: `Open block ${before}` })).toHaveAttribute(
    "href",
    `/regtest/block/${hash}`,
  );
  await expect(
    page.getByRole("status").filter({ hasText: `Block ${before} was found.` }),
  ).toBeAttached();
  // Still the next block, now on top of the one just found.
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Next block");
  await expect(page.getByText(`If it were mined now, block ${before + 1}`)).toBeVisible();
  await page.getByRole("link", { name: `Open block ${before}` }).click();
  await expect(page).toHaveURL(`/regtest/block/${hash}`);
});
