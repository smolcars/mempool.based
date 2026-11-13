// Amounts at their longest, at a phone's width: never broken inside, never parted from their unit,
// never run into a neighbour. Real pages, their answers given the largest values they can hold.
import { readFileSync } from "node:fs";
import type { Page, Route } from "@playwright/test";
import { chain, expect, stubRetargets, test } from "./fixtures";
import { amountsIntact, exactsInside } from "./rows";

/** 21 million BTC to the satoshi, a 13-digit sat amount, the largest msat limit, fine rates. */
const ALL_BTC = 2_099_999_997_690_000;
const SATS_13 = 1_234_567_890_123;
const MSAT_MAX = 2_100_000_000_000_000_000;
const ACINQ = "03864ef025fde8fb587d989186ce6a4a186895ee44a926bfc370e2c366597a3f8f";

const recorded = (name: string): Record<string, unknown> =>
  JSON.parse(
    readFileSync(new URL(`../../fixtures/lightning/app/${name}.json`, import.meta.url), "utf8"),
  ) as Record<string, unknown>;

/** Answers a route with its real reply, changed. */
async function swell(page: Page, url: RegExp, change: (json: Record<string, unknown>) => void) {
  await page.route(url, async (route: Route) => {
    const reply = await route.fetch();
    const json = (await reply.json()) as Record<string, unknown>;
    change(json);
    await route.fulfill({ response: reply, json });
  });
}

async function lightning(page: Page) {
  const policy = {
    base_fee_msat: 1_234_567_890,
    cltv_delta: 2016,
    enabled: true,
    fee_rate: 999_999,
    last_update: 1_794_456_478,
    max_htlc_msat: MSAT_MAX,
    min_htlc_msat: 1,
  };
  const channel = recorded("channel-811984x2037x0");
  channel["capacity"] = ALL_BTC;
  for (const end of ["node_one", "node_two"]) {
    (channel[end] as Record<string, unknown>)["policy"] = policy;
  }
  const node = recorded("node-acinq");
  node["capacity"] = ALL_BTC;
  const answers: Record<string, unknown> = {
    status: recorded("status"),
    [`nodes/${ACINQ}`]: node,
    [`nodes/${ACINQ}/history`]: recorded("node-acinq-history"),
    "channels/811984x2037x0": channel,
  };
  await page.route(/\/regtest\/app\/lightning\//, async (route) => {
    const url = new URL(route.request().url());
    const key = url.pathname.replace(/^\/regtest\/app\/lightning\//, "");
    // Lightning on only for the Lightning pages: elsewhere the server says it is off, as it is.
    const fromLightning = page.url().includes("/lightning/");
    const json = key === "status" && !fromLightning ? undefined : answers[key];
    // Anything not recorded is the server's own answer (regtest runs without Lightning).
    await (json === undefined ? route.continue() : route.fulfill({ json }));
  });
}

async function esplora(page: Page) {
  await swell(page, /\/regtest\/api\/tx\/[0-9a-f]{64}$/, (tx) => {
    // A large fee a real transaction can pay: 13 digits of fee on 141 vB would be a rate no
    // transaction has. The 13-digit figures are on the address and block pages.
    tx["fee"] = 12_345_678;
    for (const out of tx["vout"] as Record<string, unknown>[]) out["value"] = ALL_BTC;
    for (const input of tx["vin"] as { prevout?: Record<string, unknown> | null }[]) {
      if (input.prevout) input.prevout["value"] = ALL_BTC;
    }
  });
  await swell(page, /\/regtest\/api\/address\/[^/]+$/, (a) => {
    a["chain_stats"] = {
      ...(a["chain_stats"] as object),
      funded_txo_sum: ALL_BTC + SATS_13,
      spent_txo_sum: SATS_13,
    };
  });
  await swell(page, /\/regtest\/app\/block\/[0-9a-f]{64}\/summary$/, (s) => {
    s["fees"] = SATS_13;
    s["fee_rate"] = {
      min: 0.101,
      p10: 1.234,
      p25: 12.345,
      median: 123.456,
      p75: 1234.567,
      p90: 12_345.678,
      max: 123_456.789,
    };
  });
  await swell(page, /\/regtest\/api\/v1\/mining\/pool\/unknown$/, (p) => {
    p["totalReward"] = String(ALL_BTC);
  });
}

const PAGES = [
  "/regtest/lightning/channel/811984x2037x0",
  `/regtest/lightning/node/${ACINQ}`,
  `/regtest/tx/${chain.txs.script_types}`,
  `/regtest/address/${chain.addresses.p2tr}`,
  `/regtest/block/${chain.blocks[106]}`,
  "/regtest/mining/pool/unknown",
  // Its past adjustments, mainnet's: words, arrows and difficulties in one row.
  "/regtest/mining",
];

/** Opens a page with every figure in, in the wide faces a Linux machine falls back to. */
async function load(page: Page, path: string) {
  await page.goto(path);
  await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
  // Every figure in: nothing still loading.
  await expect(page.locator("main [aria-busy='true'], main .skel")).toHaveCount(0, {
    timeout: 15_000,
  });
  // CI's runner once broke a line a Mac's fonts kept whole.
  await page.evaluate(() => {
    const root = document.documentElement.style;
    root.setProperty("--font-sans", '"DejaVu Sans", Verdana, sans-serif');
    root.setProperty("--font-mono", '"DejaVu Sans Mono", "Courier New", monospace');
  });
}

// A test to a width and to a kind of check: four that run side by side, not one long one that
// held the browser job for 16 s on CI's runner.
for (const width of [320, 390]) {
  // Both schemes on each load: the figures' layout is the same, and each load costs a page.
  test(`amounts at their longest stay whole at ${width} px, in both schemes`, async ({ page }) => {
    await lightning(page);
    await esplora(page);
    await stubRetargets(page);
    await page.setViewportSize({ width, height: 900 });
    for (const path of PAGES) {
      await page.emulateMedia({ colorScheme: "light" });
      await load(page, path);
      for (const colorScheme of ["light", "dark"] as const) {
        await page.emulateMedia({ colorScheme });
        await page.evaluate(
          () =>
            new Promise<void>((resolve) => {
              requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
            }),
        );
        await amountsIntact(page, `${path} at ${width} px, ${colorScheme}`);
      }
    }
  });

  // Each exact amount opens whole and on screen, whatever column it is in, moving nothing.
  test(`every exact amount opens whole and on screen at ${width} px`, async ({ page }) => {
    await lightning(page);
    await esplora(page);
    // In the dark scheme, as when these checks followed the amounts' light and dark passes.
    await page.emulateMedia({ colorScheme: "dark" });
    await page.setViewportSize({ width, height: 900 });
    // The pages that have them: a channel's terms and an address's totals (a node page has none).
    for (const path of PAGES.filter((p) => /channel|address/.test(p))) {
      await load(page, path);
      await exactsInside(page, `${path} at ${width} px`);
    }
  });
}

// One exact amount open at a time, and a tap elsewhere or a scroll closes it (Safari never
// focused the button, so closing on blur left them open, two at once).
test("an exact amount closes on a tap elsewhere, on scrolling, and when another opens", async ({
  page,
}) => {
  await lightning(page);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/regtest/lightning/channel/811984x2037x0");
  const controls = page.locator("main button:has(+ .exact)");
  await expect(controls.first()).toBeVisible();
  const open = page.locator(".exact:not([hidden])");
  await controls.nth(1).click();
  await expect(open).toHaveCount(1);
  await controls.nth(0).click();
  await expect(open).toHaveCount(1);
  await expect(controls.nth(0)).toHaveAttribute("aria-expanded", "true");
  await expect(controls.nth(1)).toHaveAttribute("aria-expanded", "false");
  await page.getByRole("heading", { level: 1 }).click();
  await expect(open).toHaveCount(0);
  await controls.nth(0).click();
  await expect(open).toHaveCount(1);
  // The scroll a finger makes fires the same event; WebKit's test driver does not wheel the page.
  await page.evaluate(() => {
    window.scrollBy(0, 120);
  });
  await expect(open).toHaveCount(0);
});
