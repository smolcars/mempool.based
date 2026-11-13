// Every table fits a phone's width with nothing cut off, and no page scrolls sideways (WCAG 1.4.10).
import type { Locator, Page } from "@playwright/test";
import { chain, expect, test } from "./fixtures";
import { AUDIT } from "../src/components/audit/fakes";
import { tablesFit, tapRow } from "./rows";

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
  // Each engine reports the refused font in its own words. WebKit also calls a summary fetch that
  // the next page's load cancels an "access control" failure: the app is same-origin.
  test.use({
    consoleErrorsAllowed: [
      /status of (429|503)|font|Failed to load resource|ERR_FAILED|due to access control checks/i,
    ],
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

/** Lists on the regtest chain, each with what its rows open. */
const LISTS: { path: string; table: string | RegExp }[] = [
  { path: "/regtest/", table: /newest transactions/ },
  { path: "/regtest/blocks", table: "Recent blocks, newest first" },
  { path: `/regtest/block/${chain.blocks[106]}`, table: "Transactions in this block" },
  { path: "/regtest/block/next", table: /^Transactions in the next block/ },
  {
    path: `/regtest/address/${chain.addresses.p2wpkh}`,
    table: "Transactions of this address, newest first",
  },
  {
    path: `/regtest/address/${chain.addresses.p2wpkh}?view=unspent`,
    table: "Unspent outputs of this address, newest first",
  },
  { path: "/regtest/mining?period=all", table: /^Pools ranked by blocks found/ },
  { path: "/regtest/mining/pool/unknown", table: /^Blocks found by/ },
];

// Only a row's small link text was a target once, and on four lists the whole row did nothing.
test.describe("a tap anywhere on any list's row", () => {
  // Each tap leaves a page whose fetches may still run: WebKit calls the ones the next load
  // cancels "access control" failures, though the app is same-origin and catches them.
  test.use({ consoleErrorsAllowed: [/status of (429|503)|due to access control checks/] });
  test("opens its item, at a phone's width", async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    for (const { path, table } of LISTS) {
      await page.goto(path);
      await tapRow(page, page.getByRole("table", { name: table }), path);
    }
  });
});

// A test to a width: each is its own client, so no reader's allowance is spent on 24 page loads.
for (const width of [320, 390, 720]) {
  test(`every table fits its panel or shows that it scrolls, at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    for (const { path, table } of LISTS) {
      await page.goto(path);
      await expect(
        page.getByRole("table", { name: table }).locator("tbody tr").first(),
      ).toBeVisible({ timeout: 30_000 });
      await tablesFit(page, `${path} at ${width} px`);
    }
  });
}

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

// Fitting once ran only when the panel changed width: figures that arrived later and widened the
// table left it scrolling, droppable columns and all.
test("a table that grows after it is drawn drops another column to fit", async ({ page }) => {
  await page.setViewportSize({ width: 720, height: 900 });
  await page.goto("/regtest/blocks");
  const table = page.getByRole("table", { name: "Recent blocks, newest first" });
  await expect(table.locator("tbody tr").first()).toBeVisible();
  await expect(table.getByRole("link", { name: "Unknown" }).first()).toBeVisible();
  const gone = () => table.locator("thead th.table__gone").count();
  const before = await gone();
  // A wide name lands in a row, as a long pool name might.
  await table
    .locator("tbody tr")
    .first()
    .locator("td")
    .filter({ visible: true })
    .nth(1)
    .evaluate((td) => {
      const wide = document.createElement("span");
      wide.style.display = "inline-block";
      wide.style.width = "260px";
      td.append(wide);
    });
  await expect.poll(gone).toBeGreaterThan(before);
  await tablesFit(page, "/regtest/blocks after a wider row");
});

// Tables that appear only on a tap were outside the rule once: the audit's list on a phone put the
// fee rate it is sorted by off-screen. Every table shown on interaction is held to it too.
test("tables that open on a tap fit their panel or show that they scroll, at 320, 390 and 720 px", async ({
  page,
}) => {
  const block = chain.blocks[106];
  const txid = (n: number) => n.toString(16).padStart(64, "a");
  // A block the server audited: its totals in the summary, and the list behind the button.
  await page.route(/\/regtest\/app\/blocks\/summary\?/, async (route) => {
    const reply = await route.fetch();
    const all = (await reply.json()) as Record<string, unknown>[];
    for (const one of all) if (one["id"] === block) one["audit"] = AUDIT;
    await route.fulfill({ response: reply, json: all });
  });
  await page.route(new RegExp(`/regtest/app/block/${block}/audit$`), (route) =>
    route.fulfill({
      json: {
        ...AUDIT,
        id: block,
        height: 106,
        time: 1_767_225_600,
        seen_at: 1_767_225_600,
        complete: true,
        txs: Array.from({ length: 30 }, (_, i) => ({
          txid: txid(i + 1),
          kind: "missing",
          vsize: 1_234_567,
          rate: 12_345_678,
          first_seen: 1_767_225_000,
        })),
      },
    }),
  );
  for (const width of [320, 390, 720]) {
    await page.setViewportSize({ width, height: 900 });
    await page.goto(`/regtest/block/${block}`);
    await page.getByRole("button", { name: "Show the transactions that differ" }).click();
    await expect(page.locator(".audit-list tbody tr").first()).toBeVisible();
    await tablesFit(page, `the audit's list at ${width} px`);
    // The column it is sorted by is in sight, not behind a scroll.
    const inSight = await page.locator(".audit-list .table-scroll").evaluate((scroller) => {
      const header = [...scroller.querySelectorAll("th")].find((th) =>
        (th.textContent ?? "").includes("sat/vB"),
      );
      if (!header) return false;
      return header.getBoundingClientRect().right <= scroller.getBoundingClientRect().right + 1;
    });
    expect(inSight, `the audit's fee rate at ${width} px`).toBe(true);
    // The figures behind each chart.
    for (const path of [
      "/regtest/mempool?interval=2h",
      "/regtest/mining?period=all",
      `/regtest/address/${chain.addresses.p2tr}`,
    ]) {
      await page.goto(path);
      const figures = page.locator(".chart__figures > summary");
      await expect(figures.first()).toBeVisible({ timeout: 30_000 });
      for (const f of await figures.all()) if (await f.isVisible()) await f.click();
      await expect(page.locator(".chart__figures table").first()).toBeVisible();
      await tablesFit(page, `${path}'s figures at ${width} px`);
    }
  }
});
