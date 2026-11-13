// Regtest runs without Lightning, so the filled pages are drawn from answers a mainnet server
// gave on 2026-11-12 (fixtures/lightning/app), served in place of this server's own. Shared by
// lightning.e2e.ts and the table rules in tables.e2e.ts.
import { readFileSync } from "node:fs";
import type { Page } from "@playwright/test";

export const recorded = (name: string): unknown =>
  JSON.parse(
    readFileSync(new URL(`../../fixtures/lightning/app/${name}.json`, import.meta.url), "utf8"),
  );

export const ACINQ = "03864ef025fde8fb587d989186ce6a4a186895ee44a926bfc370e2c366597a3f8f";
/** FUNK-BTCPay: one channel, 10 ppm out and 300 ppm in. */
export const ONE = "03874dc955fba789b08db50beb6839d41379071f4f4747ae8546329a14176e339a";

/** Answers `/regtest/app/lightning/…` from `answers`, keyed by the path after it and its query. */
export async function serve(page: Page, answers: Record<string, unknown>) {
  await page.route(/\/regtest\/app\/lightning\//, async (route) => {
    const url = new URL(route.request().url());
    const key = url.pathname.replace(/^\/regtest\/app\/lightning\//, "") + url.search;
    const json = answers[key];
    await (json === undefined
      ? route.fulfill({ status: 500, json: { error: `no recorded answer for ${key}` } })
      : route.fulfill({ json }));
  });
}

export const mainnet = (): Record<string, unknown> => ({
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
