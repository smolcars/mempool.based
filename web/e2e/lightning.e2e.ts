import { readFileSync } from "node:fs";
import type { Page } from "@playwright/test";
import { chain, expect, test } from "./fixtures";

// Regtest runs without Lightning, so the filled pages are drawn from answers a mainnet server
// gave on 2026-11-12 (fixtures/lightning/app), served in place of this server's own.
const recorded = (name: string): unknown =>
  JSON.parse(
    readFileSync(new URL(`../../fixtures/lightning/app/${name}.json`, import.meta.url), "utf8"),
  );

const ACINQ = "03864ef025fde8fb587d989186ce6a4a186895ee44a926bfc370e2c366597a3f8f";

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

for (const width of [320, 360]) {
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
