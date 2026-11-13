// Mines on the run's node, so it runs after every other test (the `mines` project).
import { FOUND_WAIT_MS } from "../src/pages/projected/state";
import { expect, PROJECTION_LAG_MS, test } from "./fixtures";

test("the next block's page says when the block is found, offers it, and carries on", async ({
  page,
  mine,
}) => {
  await page.goto("/regtest/block/next");
  await expect(page.getByText("Live", { exact: true })).toBeVisible();
  const lede = page.getByText(/^What block \d+ would hold if it were mined now/);
  await expect(lede).toBeVisible();
  const before = Number(/block (\d+)/.exec((await lede.textContent()) ?? "")?.[1]);

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
  await expect(page.getByText(`What block ${before + 1} would hold`)).toBeVisible();
  await page.getByRole("link", { name: `Open block ${before}` }).click();
  await expect(page).toHaveURL(`/regtest/block/${hash}`);
});
