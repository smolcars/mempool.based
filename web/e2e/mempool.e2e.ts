import { expect, test } from "./fixtures";

test("the home page links to the mempool's history, which switches interval in the address", async ({
  page,
}) => {
  await page.goto("/regtest/");
  await page.getByRole("link", { name: "Mempool history" }).click();
  await expect(page).toHaveURL(/\/regtest\/mempool$/);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Mempool history");
  const periods = page.getByRole("navigation", { name: "Period" });
  // A young server has nothing yet or its first minutes: either is an answer, never an error.
  const answered = page
    .getByText("Nothing recorded yet")
    .or(page.getByRole("slider", { name: /The mempool's size by fee rate/ }));
  await expect(answered).toBeVisible();
  // Its records begin inside the day, so the view starts where they do, and says so in the
  // choice it marks: "24 h" is kept for the whole day.
  const recorded = periods.getByRole("link", { name: /^Recorded/ });
  await expect(recorded).toHaveAttribute("aria-current", "page");
  await expect(periods.getByRole("link", { name: /the last 24 hours/ })).not.toHaveAttribute(
    "aria-current",
    "page",
  );
  await periods.getByRole("link", { name: /the last 24 hours/ }).click();
  await expect(page).toHaveURL(/\/regtest\/mempool\?interval=24h$/);
  await expect(periods.getByRole("link", { name: /the last 24 hours/ })).toHaveAttribute(
    "aria-current",
    "page",
  );
  await expect(recorded).toHaveCount(0);
  await expect(page.getByText("No data stored").first()).toBeVisible();
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
  await expect(page.getByText(/^(Under 1|\d+)% recorded, from .* UTC\.$/)).toBeVisible();
  const figures = page.getByRole("region", { name: /The mempool's size by fee rate.*, figures/ });
  await chart.locator("xpath=ancestor::figure").getByText("Show the figures").click();
  // The table renders after the click: wait for it before reading it once.
  await expect(
    figures.getByRole("columnheader", { name: "Transactions", exact: true }),
  ).toBeVisible();
  const column = (await figures.locator("th").allInnerTexts()).indexOf("Transactions");
  expect(column).toBeGreaterThan(0);
  // Twelve bands: the lowest split three ways, as the server records it.
  for (const band of ["under 0.12 sat/vB", "0.12–0.15 sat/vB", "0.15–0.2 sat/vB", "10+ sat/vB"]) {
    await expect(figures.getByRole("columnheader", { name: band, exact: true })).toBeAttached();
  }
  // The stored minutes are the oldest rows, exactly, so nothing fills the gap; a minute the server
  // records live while the test runs is newer than all of them.
  const counts = await figures.locator(`tbody td:nth-child(${column + 1})`).allInnerTexts();
  expect(counts.slice(-STORED.length)).toEqual(STORED);
});

// The plot once started 650 to 700 px down a phone, its time axis off the first screen.
test("a phone sees the whole size chart, time axis and all, on its first screen", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  for (const path of ["/regtest/mempool", "/regtest/mempool?interval=24h"]) {
    await page.goto(path);
    const chart = page.getByRole("slider", { name: /The mempool's size by fee rate/ });
    await expect(chart).toHaveAttribute("aria-valuetext", /UTC: Total [\d.]+/);
    // In the wide faces a Linux machine falls back to, as well as these: CI's runner once put the
    // axis 15 px below the fold that a Mac's fonts kept above it.
    await page.evaluate(() => {
      const root = document.documentElement.style;
      root.setProperty("--font-sans", '"DejaVu Sans", Verdana, sans-serif');
      root.setProperty("--font-mono", '"DejaVu Sans Mono", "Courier New", monospace');
    });
    await page.evaluate(
      () =>
        new Promise<void>((resolve) => {
          requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
        }),
    );
    // The plot and every label on it, the dates under it included, end above the fold.
    const bottom = await chart.evaluate((el) =>
      Math.max(
        ...[el, ...el.querySelectorAll("svg, svg text")].map(
          (e) => e.getBoundingClientRect().bottom,
        ),
      ),
    );
    expect(bottom, path).toBeLessThanOrEqual(844);
    // Above it, one row of periods and one line of coverage.
    const periods = page.getByRole("navigation", { name: "Period" }).locator("ul");
    const rows = await periods.evaluate(
      (ul) =>
        new Set([...ul.children].map((li) => Math.round(li.getBoundingClientRect().top))).size,
    );
    expect(rows, path).toBe(1);
    const coverage = page.locator(".mempool-coverage");
    if ((await coverage.count()) > 0) {
      const lines = await coverage.evaluate((el) => {
        const range = document.createRange();
        range.selectNodeContents(el);
        // Tops more than half a line apart: the hatch swatch beside the words sits a pixel off.
        const tops = [...range.getClientRects()].map((r) => r.top);
        tops.sort((x, y) => x - y);
        return tops.filter((t, i) => i === 0 || t - (tops[i - 1] ?? t) > 8).length;
      });
      expect(lines, `${path}: coverage lines`).toBe(1);
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(
      390,
    );
    // The readout holds the time, total, transactions and lowest rate; no fee band until a finger
    // is on the chart.
    const readout = page.locator(".chart__readout").first();
    await expect(readout.getByText("Total")).toBeVisible();
    await expect(readout.getByText("Transactions")).toBeVisible();
    await expect(readout.getByText("Lowest accepted")).toBeVisible();
    for (const band of await readout.locator(".chart__value--band").all()) {
      await expect(band).toBeHidden();
    }
  }
});

test("a phone's chart reads out only the layers that hold anything", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/regtest/mempool?interval=2h");
  const chart = page.getByRole("slider", { name: /The mempool's size by fee rate/ });
  await expect(chart).toHaveAttribute("aria-valuetext", /UTC: Total [\d.]+/);
  // The seeded minutes sit in one or two fee bands, so most of the ten are empty.
  const empty = page.locator(".chart__value--empty");
  expect(await empty.count()).toBeGreaterThan(0);
  for (const value of await empty.all()) await expect(value).toBeHidden();
  await expect(page.locator(".chart__value:not(.chart__value--empty)").first()).toBeVisible();
});
