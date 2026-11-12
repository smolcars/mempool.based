// Mines on the run's node, so it runs after every other test (the `mines` project).
import { expect, test } from "./fixtures";

test("the next block's page says when the block is found, offers it, and carries on", async ({
  page,
  mine,
}) => {
  await page.goto("/regtest/block/next");
  await expect(page.getByText("Live", { exact: true })).toBeVisible();
  const lede = page.getByText(/^If it were mined now, block \d+/);
  await expect(lede).toBeVisible();
  const before = Number(/block (\d+)/.exec((await lede.textContent()) ?? "")?.[1]);

  const [hash] = await mine(1);
  // The notice's title; the live region's sentence ends "found." instead.
  await expect(page.getByText(new RegExp(`^Block ${before} was found\\s`))).toBeVisible();
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
