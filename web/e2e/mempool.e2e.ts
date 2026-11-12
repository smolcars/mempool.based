import { expect, test } from "./fixtures";

test("the home page links to the mempool's history, which switches interval in the address", async ({
  page,
}) => {
  await page.goto("/regtest/");
  await page.getByRole("link", { name: "Mempool history" }).click();
  await expect(page).toHaveURL(/\/regtest\/mempool$/);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Mempool history");
  const periods = page.getByRole("navigation", { name: "Period" });
  await expect(periods.getByRole("link", { name: /the last 24 hours/ })).toHaveAttribute(
    "aria-current",
    "page",
  );
  // A young server has nothing yet or its first minutes: either is an answer, never an error.
  const answered = page
    .getByText("Nothing recorded yet")
    .or(page.getByRole("slider", { name: /The mempool's size by fee rate/ }));
  await expect(answered).toBeVisible();
  await periods.getByRole("link", { name: /the last 2 hours/ }).click();
  await expect(page).toHaveURL(/\/regtest\/mempool\?interval=2h$/);
  await expect(periods.getByRole("link", { name: /the last 2 hours/ })).toHaveAttribute(
    "aria-current",
    "page",
  );
  await expect(answered).toBeVisible();
});

// e2e.sh stores 20 minutes an hour old before the server starts, through the sampler's own writer
// (crates/engine/examples/history_seed.rs): minute i holds 901 + i transactions, and 8 to 12 are
// missing. The sampler's own timing is the engine's tests' to prove (docs/STORE.md). Newest first,
// as the figures table lists them.
const STORED = Array.from({ length: 20 }, (_, i) => 19 - i)
  .filter((i) => i < 8 || i > 12)
  .map((i) => String(901 + i));

test("the page shows the minutes the server stored, with the gap where none was", async ({
  page,
}) => {
  await page.goto("/regtest/mempool?interval=2h");
  const chart = page.getByRole("slider", { name: /The mempool's size by fee rate/ });
  await expect(chart).toHaveAttribute("aria-valuetext", /UTC: Total [\d.]+/);
  await expect(page.getByText(/This server has recorded the mempool since/)).toBeVisible();
  const figures = page.getByRole("region", { name: /The mempool's size by fee rate.*, figures/ });
  await chart.locator("xpath=ancestor::figure").getByText("Show the figures").click();
  // The table renders after the click: wait for it before reading it once.
  await expect(
    figures.getByRole("columnheader", { name: "Transactions", exact: true }),
  ).toBeVisible();
  const column = (await figures.locator("th").allInnerTexts()).indexOf("Transactions");
  expect(column).toBeGreaterThan(0);
  // A minute the server records live while the test runs is not in the 900s.
  const counts = await figures.locator(`tbody td:nth-child(${column + 1})`).allInnerTexts();
  expect(counts.filter((c) => /^9\d\d$/.test(c))).toEqual(STORED);
});
