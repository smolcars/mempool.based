// Every page type in a stand-in for a wider system font than a Mac's, across widths and in both
// schemes: nothing runs into anything, nothing is cut, amounts stay whole, tables fit their panels.
import { readFileSync } from "node:fs";
import { stripVTControlCharacters } from "node:util";
import type { Page } from "@playwright/test";
import { type Prose, chain, expect, stubRetargets, test } from "./fixtures";
import { frames, layoutFaults } from "./layout";
import { amountsIntact, tablesFit } from "./rows";

// The wide stand-in unless the run names another (`E2E_PROSE`, checked in fixtures.ts).
test.use({ prose: (process.env["E2E_PROSE"] as Prose | undefined) ?? "wide" });
// Each width is a resize of one loaded page: WebKit calls a fetch the next page's load cancels an
// "access control" failure, though the app is same-origin and catches it.
test.use({ consoleErrorsAllowed: [/status of (429|503)|due to access control checks/] });

const ACINQ = "03864ef025fde8fb587d989186ce6a4a186895ee44a926bfc370e2c366597a3f8f";
const recorded = (name: string): unknown =>
  JSON.parse(
    readFileSync(new URL(`../../fixtures/lightning/app/${name}.json`, import.meta.url), "utf8"),
  );

/** Lightning on regtest from mainnet's recorded answers (as in lightning.e2e.ts). */
async function lightning(page: Page) {
  const answers: Record<string, unknown> = {
    status: recorded("status"),
    overview: recorded("overview"),
    "statistics/1w": recorded("statistics-1w"),
    [`nodes/${ACINQ}`]: recorded("node-acinq"),
    [`nodes/${ACINQ}/history`]: recorded("node-acinq-history"),
    "channels/811984x2037x0": recorded("channel-811984x2037x0"),
    "search?q=acinq": recorded("search-acinq"),
    "rankings/capacity?limit=100": recorded("rankings-capacity"),
  };
  await page.route(/\/regtest\/app\/lightning\//, async (route) => {
    const url = new URL(route.request().url());
    const key = url.pathname.replace(/^\/regtest\/app\/lightning\//, "") + url.search;
    const json = answers[key];
    await (json === undefined ? route.continue() : route.fulfill({ json }));
  });
}

/** Every page type, each with what it needs stubbed. */
const PAGES: { path: string; stub?: (page: Page) => Promise<void> }[] = [
  { path: "/regtest/" },
  { path: "/regtest/blocks" },
  { path: `/regtest/block/${chain.blocks[106]}` },
  { path: "/regtest/block/next" },
  { path: `/regtest/tx/${chain.txs.script_types}` },
  { path: `/regtest/tx/${chain.txs.cpfp_child}` },
  { path: `/regtest/address/${chain.addresses.p2tr}` },
  { path: "/regtest/mining", stub: stubRetargets },
  { path: "/regtest/mining/pool/unknown" },
  { path: "/regtest/mempool" },
  { path: "/regtest/rbf" },
  { path: "/regtest/lightning", stub: lightning },
  { path: "/regtest/lightning/rankings", stub: lightning },
  { path: `/regtest/lightning/node/${ACINQ}`, stub: lightning },
  { path: "/regtest/lightning/channel/811984x2037x0", stub: lightning },
  { path: "/regtest/lightning/search?q=acinq", stub: lightning },
  { path: "/regtest/no/such/page" },
];

/** What a failed check says, on one line: its label and what it found. */
const said = (error: unknown) =>
  stripVTControlCharacters(String(error))
    .split("\n")
    .filter((l) => /^(Error|\+ {3}|Received)/.test(l.trim()) && !l.includes("Error: expect"))
    .map((l) => l.trim())
    .join(" ");

// `E2E_SWEEP=1`: the design's widths, the breakpoints (640, 1040) either side and every 40 px
// between, each in both schemes. Otherwise (pull requests) the widths where the sweep found
// faults, each in one scheme: the layout is the same in both.
const SWEEP = process.env["E2E_SWEEP"] === "1";
const NAMED = new Set([390, 639, 641, 1039, 1041, 1100]);
const WIDTHS = SWEEP
  ? Array.from({ length: 1441 - 320 }, (_, i) => 320 + i).filter(
      (w) => w % 40 === 0 || NAMED.has(w),
    )
  : [320, 400, 641, 1100];

for (const { path, stub } of PAGES) {
  test(`${path} holds in a stand-in font at every width`, async ({ page }) => {
    test.setTimeout(SWEEP ? 240_000 : 30_000);
    await stub?.(page);
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(path);
    await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
    await expect(page.locator("main [aria-busy='true'], main .skel")).toHaveCount(0, {
      timeout: 15_000,
    });
    // Every fault at every width, so one run lists them all.
    const faults: string[] = [];
    for (const [i, width] of WIDTHS.entries()) {
      await page.setViewportSize({ width, height: 900 });
      const schemes: ("light" | "dark")[] = SWEEP
        ? ["light", "dark"]
        : [i % 2 === 0 ? "light" : "dark"];
      for (const colorScheme of schemes) {
        await page.emulateMedia({ colorScheme });
        await frames(page);
        const at = `${width} px, ${colorScheme}`;
        for (const f of await layoutFaults(page)) faults.push(`${at}: ${f}`);
        for (const check of [amountsIntact, tablesFit]) {
          try {
            await check(page, at);
          } catch (error) {
            faults.push(said(error));
          }
        }
      }
    }
    expect(faults).toStrictEqual([]);
  });
}
