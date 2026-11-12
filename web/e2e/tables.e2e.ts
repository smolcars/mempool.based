// Every table fits a phone's width with nothing cut off, and no page scrolls sideways (WCAG 1.4.10).
import type { Locator, Page } from "@playwright/test";
import { chain, expect, test } from "./fixtures";

test.use({ consoleErrorsAllowed: [/status of (429|503)/] });

const PAGES = [
  { path: `/regtest/block/${chain.blocks[106]}`, table: "Transactions in this block" },
  { path: "/regtest/blocks", table: "Recent blocks, newest first" },
  {
    path: `/regtest/address/${chain.addresses.p2wpkh}`,
    table: "Transactions of this address, newest first",
  },
  {
    path: `/regtest/address/${chain.addresses.p2wpkh}?view=unspent`,
    table: "Unspent outputs of this address, newest first",
  },
  { path: `/regtest/tx/${chain.txs.script_types}`, table: null },
];

for (const width of [320, 360]) {
  test(`every table fits at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 800 });
    for (const { path, table } of PAGES) {
      await page.goto(path);
      await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
      if (table) {
        const t = page.getByRole("table", { name: table });
        await expect(t.getByRole("link").first()).toBeVisible();
        const box = await t.evaluate((el) => {
          const scroller = el.parentElement as HTMLElement;
          return { table: el.scrollWidth, room: scroller.clientWidth };
        });
        expect(box.table, `${path}: table width`).toBeLessThanOrEqual(box.room);
      } else {
        await expect(page.getByRole("list", { name: "Outputs" })).toBeVisible();
      }
      const scrolled = await page.evaluate(() => document.documentElement.scrollWidth);
      expect(scrolled, `${path}: page width`).toBeLessThanOrEqual(width);
    }
  });
}

// Without the web font every engine draws figures in its own monospace, which can be wider: the
// blocks list must still fit a 320 px phone.
test.describe("without the web font", () => {
  // Each engine reports the refused font in its own words.
  test.use({
    consoleErrorsAllowed: [/status of (429|503)|font|Failed to load resource|ERR_FAILED/i],
  });

  test("the blocks list fits 320 px when the web font never arrives", async ({ page }) => {
    await page.route(/\.woff2$/, (route) => route.abort());
    await page.setViewportSize({ width: 320, height: 700 });
    await page.goto("/regtest/blocks");
    const t = page.getByRole("table", { name: "Recent blocks, newest first" });
    await expect(t.getByRole("link").first()).toBeVisible();
    const box = await t.evaluate((el) => ({
      table: el.scrollWidth,
      room: (el.parentElement as HTMLElement).clientWidth,
    }));
    expect(box.table).toBeLessThanOrEqual(box.room);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(
      320,
    );
  });

  // Linux's fallback faces are wider than a Mac's: a 320 px page once scrolled sideways on CI's
  // runner alone. Wide faces are forced here, so any engine on any system shows the same.
  test("no page scrolls sideways at 320 px in wide fallback faces", async ({ page }) => {
    await page.route(/\.woff2$/, (route) => route.abort());
    await page.setViewportSize({ width: 320, height: 700 });
    // Each page with what shows it has loaded: a page is measured whole, never as its skeleton.
    const pages: [string, (p: Page) => Locator][] = [
      ["/regtest/", (p) => p.getByRole("table", { name: /newest transactions/ }).getByRole("link")],
      ["/regtest/blocks", (p) => p.getByRole("table").getByRole("link")],
      [`/regtest/block/${chain.blocks[106]}`, (p) => p.getByRole("table").getByRole("link")],
      ["/regtest/block/next", (p) => p.locator("main table a")],
      [`/regtest/tx/${chain.txs.script_types}`, (p) => p.locator(".tx-summary .amount")],
      [`/regtest/address/${chain.addresses.p2tr}`, (p) => p.getByRole("table").getByRole("link")],
      // Regtest's chain is months old: all of history has its pools table.
      ["/regtest/mining?period=all", (p) => p.locator("main table a")],
      ["/regtest/mempool", (p) => p.getByRole("slider")],
      ["/regtest/lightning", (p) => p.getByText("Lightning is switched off on Regtest")],
      ["/regtest/no/such/page", (p) => p.getByRole("main").getByRole("searchbox")],
    ];
    for (const [path, ready] of pages) {
      await page.goto(path);
      await expect(ready(page).first()).toBeVisible();
      await page.evaluate(() => {
        const root = document.documentElement.style;
        root.setProperty("--font-sans", '"DejaVu Sans", Verdana, sans-serif');
        root.setProperty("--font-mono", '"DejaVu Sans Mono", "Courier New", monospace');
      });
      // One frame in the new faces before measuring.
      await page.evaluate(
        () =>
          new Promise<void>((resolve) => {
            requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
          }),
      );
      const over = await page.evaluate(() =>
        [...document.querySelectorAll("main *")]
          .filter((el) => el.getBoundingClientRect().right > 320.5 && !el.closest(".table-scroll"))
          .slice(0, 4)
          .map((el) => `${el.tagName.toLowerCase()}.${el.getAttribute("class") ?? ""}`),
      );
      const scrolled = await page.evaluate(() => document.documentElement.scrollWidth);
      expect(scrolled, `${path}: past the edge: ${over.join(", ")}`).toBeLessThanOrEqual(320);
    }
  });
});

// Only a row's small link text was a target once: on a phone the whole row opens its item.
test("a tap anywhere on a list's row opens its item, and the keyboard meets it once", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/regtest/blocks");
  const table = page.getByRole("table", { name: "Recent blocks, newest first" });
  const row = table.getByRole("row").nth(1);
  const height = await row.getByRole("link").first().textContent();
  // Its transaction count, far from any link: pressed where it is, as a thumb would.
  const cell = await row.getByRole("cell").nth(2).boundingBox();
  if (!cell) throw new Error("no cell");
  await page.mouse.click(cell.x + cell.width / 2, cell.y + cell.height / 2);
  await expect(page).toHaveURL(/\/regtest\/block\/[0-9a-f]{64}$/);
  await expect(page.getByRole("heading", { level: 1 })).toContainText(height ?? "-");
  // The row's own link is hidden from the keyboard and from screen readers: one stop a row.
  await page.goBack();
  await expect(table.locator(".table__rowlink").first()).toHaveAttribute("tabindex", "-1");
  await expect(table.locator(".table__rowlink").first()).toHaveAttribute("aria-hidden", "true");
});
