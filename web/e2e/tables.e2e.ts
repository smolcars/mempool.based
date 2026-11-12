// Every table fits a phone's width with nothing cut off, and no page scrolls sideways (WCAG 1.4.10).
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
});
