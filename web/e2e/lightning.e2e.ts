import { readFileSync } from "node:fs";
import type { Page } from "@playwright/test";
import { chain, expect, test } from "./fixtures";
import { tablesFit, tapRow } from "./rows";

// Regtest runs without Lightning, so the filled pages are drawn from answers a mainnet server
// gave on 2026-11-12 (fixtures/lightning/app), served in place of this server's own.
const recorded = (name: string): unknown =>
  JSON.parse(
    readFileSync(new URL(`../../fixtures/lightning/app/${name}.json`, import.meta.url), "utf8"),
  );

const ACINQ = "03864ef025fde8fb587d989186ce6a4a186895ee44a926bfc370e2c366597a3f8f";
/** FUNK-BTCPay: one channel, 10 ppm out and 300 ppm in. */
const ONE = "03874dc955fba789b08db50beb6839d41379071f4f4747ae8546329a14176e339a";

/** Answers `/regtest/app/lightning/…` from `answers`, keyed by the path after it and its query. */
async function serve(page: Page, answers: Record<string, unknown>) {
  await page.route(/\/regtest\/app\/lightning\//, async (route) => {
    const url = new URL(route.request().url());
    const key = url.pathname.replace(/^\/regtest\/app\/lightning\//, "") + url.search;
    const json = answers[key];
    await (json === undefined
      ? route.fulfill({ status: 500, json: { error: `no recorded answer for ${key}` } })
      : route.fulfill({ json }));
  });
}

const mainnet = () => ({
  status: recorded("status"),
  overview: recorded("overview"),
  "statistics/1w": recorded("statistics-1w"),
  [`nodes/${ACINQ}`]: recorded("node-acinq"),
  [`nodes/${ACINQ}/history`]: recorded("node-acinq-history"),
  [`nodes/${ONE}`]: recorded("node-one"),
  [`nodes/${ONE}/history`]: recorded("node-one-history"),
  "channels/811984x2037x0": recorded("channel-811984x2037x0"),
  "search?q=acinq": recorded("search-acinq"),
  "rankings/capacity?limit=100": recorded("rankings-capacity"),
});

test("the header leads to Lightning, which says it is switched off on regtest", async ({
  page,
}) => {
  await page.goto("/regtest/");
  await page.getByRole("navigation").getByRole("link", { name: "Lightning" }).click();
  await expect(page).toHaveURL(/\/regtest\/lightning$/);
  await expect(page.getByText("Lightning is switched off on Regtest")).toBeVisible();
  await expect(page.getByRole("slider")).toHaveCount(0);

  for (const path of [`/regtest/lightning/node/${ACINQ}`, "/regtest/lightning/channel/1x2x3"]) {
    await page.goto(path);
    await expect(page.getByText("Lightning is switched off on Regtest")).toBeVisible();
  }
});

test("a transaction page on a network without Lightning asks only its status", async ({ page }) => {
  const asked: string[] = [];
  page.on("request", (r) => {
    if (r.url().includes("/app/lightning/")) asked.push(new URL(r.url()).pathname);
  });
  await page.goto(`/regtest/tx/${chain.txs.script_types}`);
  await expect(page.getByText("Confirmed", { exact: true }).first()).toBeVisible();
  await expect.poll(() => asked).toStrictEqual(["/regtest/app/lightning/status"]);
  await expect(page.getByRole("heading", { name: "Lightning" })).toHaveCount(0);
});

// "← Nodes by open channels" once pushed "History" onto a line of its own on a phone.
test("a node reached from a ranking keeps its section's links on one line on a phone", async ({
  page,
}) => {
  // The ranking by channels in the recorded ranking's shape.
  await serve(page, { ...mainnet(), "rankings/channels?limit=100": recorded("rankings-capacity") });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/regtest/lightning/rankings?by=channels");
  await page.getByRole("main").getByRole("link", { name: "ACINQ" }).first().click();
  await expect(page.locator(".ln-nav__back")).toHaveText("← Nodes by open channels");
  for (const width of [390, 320]) {
    await page.setViewportSize({ width, height: 844 });
    const tops = await page
      .locator(".ln-nav__item")
      .evaluateAll((all) => all.map((a) => Math.round(a.getBoundingClientRect().top)));
    expect(new Set(tops).size, `${width} px: ${tops.join(", ")}`).toBe(1);
  }
});

test("the overview, a node and a channel, from mainnet's answers", async ({ page }) => {
  await serve(page, mainnet());
  await page.goto("/regtest/lightning");
  await expect(page.getByRole("heading", { name: "The network now" })).toBeVisible();
  await expect(page.locator("dl").getByText(/^36\s228$/)).toBeVisible();
  await expect(page.getByRole("heading", { name: "Newest channels" })).toBeVisible();

  const connected = page.getByRole("table", { name: "The five nodes with the most channels" });
  await connected.getByRole("link", { name: "ACINQ" }).click();
  await expect(page).toHaveURL(`/regtest/lightning/node/${ACINQ}`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("ACINQ");
  await expect(page.getByText("Page 1 of 73")).toBeVisible();

  await page.getByRole("link", { name: "811984x2037x0" }).click();
  await expect(page.getByRole("heading", { level: 1 })).toContainText("Channel 811984x2037x0");
  // Back to where the reader came from: the node, by its name.
  await expect(page.locator(".ln-nav__back")).toHaveText("← ACINQ");
  await expect(page.getByRole("heading", { name: "Its two ends" })).toBeVisible();
  await expect(page.getByRole("link", { name: /^811\s984$/ })).toHaveAttribute(
    "href",
    /^\/regtest\/block\/0{18}/,
  );
});

test("the section's search finds a node by name", async ({ page }) => {
  await serve(page, mainnet());
  await page.goto("/regtest/lightning");
  await page.getByLabel("Find a node or channel").fill("acinq");
  await page.getByRole("button", { name: "Search" }).click();
  await expect(page).toHaveURL("/regtest/lightning/search?q=acinq");
  await expect(page.getByRole("link", { name: "ACINQ" })).toHaveAttribute(
    "href",
    `/regtest/lightning/node/${ACINQ}`,
  );
});

test("hostile names stay inert text and never widen the page", async ({ page }) => {
  const hostile = ['<img src=x onerror="pwned=1">', "‮QNICA", "😀".repeat(5000), "W".repeat(10_000)];
  const node = (alias: string, i: number) => ({
    id: `03${String(i).repeat(64)}`,
    alias,
    capacity: 100_000_000,
    channels: 3,
  });
  await serve(page, {
    status: recorded("status"),
    "search?q=acinq": { nodes: hostile.map((a, i) => node(a, i)), channels: [] },
    [`nodes/${ACINQ}`]: {
      ...(recorded("node-acinq") as object),
      alias: hostile[3],
      color: "red;x",
    },
    [`nodes/${ACINQ}/history`]: recorded("node-acinq-history"),
  });
  await page.goto("/regtest/lightning/search?q=acinq");
  await expect(page.getByText("�QNICA")).toBeVisible();
  await expect(page.getByText(hostile[0] ?? "")).toBeVisible();
  expect(await page.evaluate(() => "pwned" in window)).toBe(false);
  await expect(page.locator("main img")).toHaveCount(0);
  const wide = () =>
    page.evaluate(
      () => document.documentElement.scrollWidth > document.documentElement.clientWidth,
    );
  expect(await wide()).toBe(false);

  await page.goto(`/regtest/lightning/node/${ACINQ}`);
  await expect(page.getByRole("button", { name: "Show the whole name" })).toBeVisible();
  expect(await wide()).toBe(false);
});

test("a transaction page links the channel its transaction opened", async ({ page }) => {
  const overview = recorded("overview") as { opened: { id: string }[] };
  await serve(page, {
    status: recorded("status"),
    [`tx/${chain.txs.script_types}`]: { opened: overview.opened.slice(0, 1), closed: [] },
  });
  await page.goto(`/regtest/tx/${chain.txs.script_types}`);
  await expect(page.getByText("This transaction opened Lightning channel")).toBeVisible();
  await expect(
    page.getByRole("link", { name: overview.opened[0]?.id ?? "missing" }),
  ).toHaveAttribute("href", `/regtest/lightning/channel/${overview.opened[0]?.id ?? ""}`);
});

// The narrowest phone, strictly: 390 and 720 px are in "Lightning's lists" below.
for (const width of [320]) {
  test(`every Lightning table fits at ${width} px`, async ({ page }) => {
    await serve(page, mainnet());
    await page.setViewportSize({ width, height: 800 });
    for (const path of [
      "/regtest/lightning",
      `/regtest/lightning/node/${ACINQ}`,
      "/regtest/lightning/search?q=acinq",
      "/regtest/lightning/rankings",
      "/regtest/lightning/channel/811984x2037x0",
    ]) {
      await page.goto(path);
      await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
      // Every page here links a node once its answer is drawn.
      await expect(
        page.locator('main a[href*="/lightning/node/"]:not(.table__rowlink)').first(),
      ).toBeVisible();
      for (const t of await page.getByRole("table").all()) {
        const box = await t.evaluate((el) => ({
          table: el.scrollWidth,
          room: (el.parentElement as HTMLElement).clientWidth,
        }));
        expect(box.table, `${path}: table width`).toBeLessThanOrEqual(box.room);
      }
      // What sticks out past the viewport, named, so a failure on another platform says why.
      const over = await page.evaluate((w) => {
        const out: string[] = [];
        for (const el of document.querySelectorAll("main *")) {
          const r = el.getBoundingClientRect();
          if (r.right > w + 0.5 && !el.closest(".table-scroll")) {
            out.push(
              `${el.tagName.toLowerCase()}.${el.getAttribute("class") ?? ""} ${Math.round(r.right)}`,
            );
          }
        }
        return out.slice(0, 5);
      }, width);
      const scrolled = await page.evaluate(() => document.documentElement.scrollWidth);
      expect(scrolled, `${path}: page width; past it: ${over.join(", ")}`).toBeLessThanOrEqual(
        width,
      );
    }
  });
}

// Right-aligned figures once ran into the next column's text ("RankNode", "0.17000000Active"):
// at every width, the text of neighbouring cells keeps a gutter between them.
for (const width of [360, 768, 1440]) {
  test(`every Lightning table keeps a gutter between columns at ${width} px`, async ({ page }) => {
    await serve(page, mainnet());
    await page.setViewportSize({ width, height: 900 });
    for (const path of [`/regtest/lightning/node/${ACINQ}`, "/regtest/lightning/rankings"]) {
      await page.goto(path);
      await expect(
        page.locator('main a[href*="/lightning/node/"]:not(.table__rowlink)').first(),
      ).toBeVisible();
      const tight = await page.evaluate(() => {
        const out: string[] = [];
        for (const row of document.querySelectorAll("main table tr")) {
          const boxes = [...row.querySelectorAll("th, td")]
            .filter((c) => (c as HTMLElement).offsetParent !== null)
            .map((c) => {
              // The visible text only: a unit hidden for screen readers takes no room.
              let left = Number.POSITIVE_INFINITY;
              let right = Number.NEGATIVE_INFINITY;
              const walk = document.createTreeWalker(c, NodeFilter.SHOW_TEXT);
              for (let n = walk.nextNode(); n; n = walk.nextNode()) {
                if (n.parentElement?.closest(".visually-hidden, .table__long")) continue;
                const range = document.createRange();
                range.selectNodeContents(n);
                for (const r of range.getClientRects()) {
                  if (r.width === 0) continue;
                  left = Math.min(left, r.left);
                  right = Math.max(right, r.right);
                }
              }
              return {
                left,
                right,
                extent: right > left ? right - left : 0,
                text: c.textContent ?? "",
              };
            })
            .filter((b) => b.extent > 0);
          for (let i = 1; i < boxes.length; i++) {
            const a = boxes[i - 1];
            const b = boxes[i];
            if (a && b && b.left - a.right < 8) out.push(`${a.text}|${b.text}`);
          }
        }
        return out.slice(0, 5);
      });
      expect(tight, `${path} at ${width} px`).toStrictEqual([]);
    }
  });
}

test("History in the section's links lands on the overview's chart from any Lightning page", async ({
  page,
}) => {
  await serve(page, mainnet());
  for (const width of [1440, 390]) {
    await page.setViewportSize({ width, height: 844 });
    for (const from of [
      `/regtest/lightning/node/${ACINQ}`,
      "/regtest/lightning/channel/811984x2037x0",
      "/regtest/lightning/rankings",
    ]) {
      await page.goto(from);
      const nav = page.getByRole("navigation", { name: "Lightning" });
      await expect(nav.getByRole("link", { name: "Overview" })).toBeVisible();
      await expect(nav.getByRole("link", { name: "Rankings" })).toBeVisible();
      await nav.getByRole("link", { name: "History" }).click();
      await expect(page).toHaveURL(/\/regtest\/lightning#history$/);
      // One current destination: the chart, now the reader is on it.
      await expect(nav.locator('[aria-current="location"]')).toHaveText("History");
      await expect(nav.locator('[aria-current="page"]')).toHaveCount(0);
      const heading = page.getByRole("heading", { name: "Over time" });
      await expect(heading).toBeInViewport();
      // Clear of the sticky header, not under it.
      const header = await page.locator(".site-header").boundingBox();
      const top = (await heading.boundingBox())?.y ?? 0;
      expect(top, `${from} at ${width} px`).toBeGreaterThanOrEqual(
        (header?.y ?? 0) + (header?.height ?? 0),
      );
    }
  }
});

test.describe("Lightning's lists", () => {
  // A tap opens a node or channel with no recorded answer, which the stand-in refuses with a 500.
  test.use({ consoleErrorsAllowed: [/status of 500/] });
  const pages = [
    "/regtest/lightning",
    "/regtest/lightning/rankings",
    `/regtest/lightning/node/${ACINQ}`,
    "/regtest/lightning/search?q=acinq",
  ];

  test("every row opens its item from anywhere on it, at a phone's width", async ({ page }) => {
    await serve(page, mainnet());
    await page.setViewportSize({ width: 390, height: 844 });
    for (const path of pages) {
      await page.goto(path);
      const tables = page.locator("table.table--rows");
      await expect(tables.first()).toBeVisible();
      const count = await tables.count();
      for (let i = 0; i < count; i++) {
        if (i > 0) await page.goto(path);
        await tapRow(page, page.locator("table.table--rows").nth(i), `${path}, table ${i + 1}`);
      }
    }
  });

  test("every table fits its panel or shows that it scrolls, at 320, 390 and 720 px", async ({
    page,
  }) => {
    await serve(page, mainnet());
    for (const width of [320, 390, 720]) {
      await page.setViewportSize({ width, height: 900 });
      for (const path of pages) {
        await page.goto(path);
        await expect(page.locator("table.table--rows tbody tr").first()).toBeVisible();
        await tablesFit(page, `${path} at ${width} px`);
      }
    }
  });
});

test("the overview's own links mark one current place at a time", async ({ page }) => {
  await serve(page, mainnet());
  await page.goto("/regtest/lightning");
  const nav = page.getByRole("navigation", { name: "Lightning" });
  await expect(nav.locator(":is([aria-current='page'], [aria-current='location'])")).toHaveText(
    "Overview",
  );
});

/** The node page's section headed `name`. */
const panel = (page: Page, name: string) =>
  page.locator("section", { has: page.getByRole("heading", { name, exact: true }) });

test("a node's past and fee rates sit side by side, and a phone reaches its channels soon after", async ({
  page,
}) => {
  await serve(page, mainnet());
  await page.goto(`/regtest/lightning/node/${ACINQ}`);
  const chart = page.getByRole("slider", { name: /This node's channels and capacity/ });
  await expect(chart).toHaveAttribute("aria-valuetext", /Channels 1\s793/);
  const fees = panel(page, "Fee rates");
  await expect(fees.getByText(/^It charges a median of 200–499 ppm to forward/)).toBeVisible();
  // Two rows of bars on one scale: the longest is the 1 469 channels it prices at 200–499 ppm.
  const widths = await fees
    .locator(".ln-fees__fill")
    .evaluateAll((bars) => bars.map((b) => b.getBoundingClientRect().width));
  expect(Math.max(...widths)).toBeGreaterThan(10 * Math.min(...widths));

  await page.setViewportSize({ width: 1440, height: 900 });
  const top = async (name: string) => (await panel(page, name).boundingBox())?.y ?? 0;
  expect(await top("Over time")).toBe(await top("Fee rates"));

  await page.setViewportSize({ width: 390, height: 844 });
  // In the wide faces a Linux machine falls back to, as the mempool's chart is checked.
  await page.evaluate(() => {
    const root = document.documentElement.style;
    root.setProperty("--font-sans", '"DejaVu Sans", Verdana, sans-serif');
    root.setProperty("--font-mono", '"DejaVu Sans Mono", "Courier New", monospace');
  });
  const header = (await page.locator(".site-header").boundingBox())?.height ?? 0;
  // The plot and its axis are on the first screen of its panel, under the sticky header.
  const fold = await panel(page, "Over time").evaluate((section) => {
    const plot = section.querySelector("[role=slider]");
    const bottom = Math.max(
      ...[...(plot?.querySelectorAll("svg, svg text") ?? [])].map(
        (e) => e.getBoundingClientRect().bottom,
      ),
    );
    return bottom - section.getBoundingClientRect().top;
  });
  expect(fold).toBeLessThanOrEqual(844 - header);
  // A tap reads a point out and never moves the plot.
  const svg = chart.locator("svg").first();
  await svg.scrollIntoViewIfNeeded();
  const before = await svg.boundingBox();
  await svg.click({ position: { x: 20, y: 60 } });
  await expect(page.locator(".chart__readout").first()).toContainText("13 Nov 2026");
  expect(await svg.boundingBox()).toStrictEqual(before);
  // Both charts take at most a screen and a half before what follows them.
  const charts = (await top("What it announces")) - (await top("Over time"));
  expect(charts).toBeLessThanOrEqual(1.5 * 844);
  // The figures: a finger's target, and a table that fits the narrowest phone.
  const figures = fees.locator(".chart__figures > summary");
  expect((await figures.boundingBox())?.height ?? 0).toBeGreaterThanOrEqual(44);
  await figures.click();
  await page.setViewportSize({ width: 320, height: 844 });
  await expect(fees.getByRole("table")).toBeVisible();
  await tablesFit(page, "a node's fee figures at 320 px");
  // Every column, the capacities included: the table scrolls rather than drop one.
  await expect(fees.locator("thead th:visible")).toHaveCount(5);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(320);
});

test.describe("a node's past and fees at the edges", () => {
  test.use({ consoleErrorsAllowed: [/status of 500/] });

  test("one channel, no past yet, and a refused history", async ({ page }) => {
    await serve(page, mainnet());
    await page.goto(`/regtest/lightning/node/${ONE}`);
    const fees = panel(page, "Fee rates");
    await expect(fees.locator(".ln-fees__answer")).toHaveText(
      "It charges 10–49 ppm to forward; its peer charges 200–499 ppm towards it.",
    );
    // From its cheapest rate to its dearest, nothing past them.
    await expect(fees.locator(".ln-fees__label")).toHaveText([
      "10–49",
      "50–99",
      "100–199",
      "200–499",
    ]);
    await expect(
      page.getByRole("slider", { name: /This node's channels and capacity/ }),
    ).toHaveAttribute("aria-valuetext", /Channels 1\b/);

    const latest = { time: 1_794_594_636, channels: 1793, capacity: 28_498_822_087 };
    await serve(page, {
      ...mainnet(),
      [`nodes/${ACINQ}/history`]: { from: null, points: [latest] },
    });
    await page.goto(`/regtest/lightning/node/${ACINQ}`);
    const past = panel(page, "Over time");
    await expect(past.getByText("Nothing to draw yet")).toBeVisible();
    await expect(past.getByRole("slider")).toHaveCount(0);

    const answers: Record<string, unknown> = mainnet();
    delete answers[`nodes/${ACINQ}/history`];
    await serve(page, answers);
    await page.goto(`/regtest/lightning/node/${ACINQ}`);
    await expect(past.getByText("Could not load its history")).toBeVisible();
    await expect(past.getByRole("button", { name: "Try again" })).toBeVisible();
    // The node's own figures stay.
    await expect(panel(page, "Fee rates").locator(".ln-fees__answer")).toBeVisible();
  });
});
