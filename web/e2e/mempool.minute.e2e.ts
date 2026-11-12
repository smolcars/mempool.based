import { expect, test } from "./fixtures";

// The server records its first minute at the second turn of the clock after it starts (the first
// is never whole; docs/STORE.md "mempool_history"): 60 to 120 s. This test runs in its own
// project from the start of the run, beside every other, so only the part of that wait the other
// tests do not already take adds to the run.
const FIRST_MINUTE_MS = 130_000;

test("a page left open shows the first recorded minute without a reload", async ({ page }) => {
  test.setTimeout(FIRST_MINUTE_MS + 30_000);
  await page.goto("/regtest/mempool?interval=2h");
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Mempool history");
  await page.evaluate(() => {
    document.documentElement.dataset["e2eOpened"] = "yes";
  });
  const chart = page.getByRole("slider", { name: /The mempool's size by fee rate/ });
  await expect(chart).toBeVisible({ timeout: FIRST_MINUTE_MS });
  await expect(chart).toHaveAttribute("aria-valuetext", /UTC: Total [\d.]+/);
  await expect(page.getByText(/This server has recorded the mempool since/)).toBeVisible();
  // The same document: the page asked again by itself when the minute was due.
  expect(await page.evaluate(() => document.documentElement.dataset["e2eOpened"])).toBe("yes");
});
