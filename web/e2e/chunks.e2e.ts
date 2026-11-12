// A page's code and styles load on demand. These fail those downloads in the browser, as a dropped
// connection or a deploy that replaced the files would.
import type { Page, Route } from "@playwright/test";
import { chain, expect, test } from "./fixtures";

// Each refused download is logged by the browser.
test.use({ consoleErrorsAllowed: [/status of (404|503)/] });

const BLOCK = `/regtest/block/${chain.blocks[106]}`;

/** Refuses the block page's stylesheet and script `times` times each, then lets them through. */
async function refuse(page: Page, times: number, status = 503) {
  const seen = { css: 0, js: 0 };
  const handler = (kind: "css" | "js") => (route: Route) =>
    seen[kind]++ < times ? route.fulfill({ status, body: "" }) : route.continue();
  await page.route(/\/assets\/BlockPage-[^/]+\.css$/, handler("css"));
  await page.route(/\/assets\/BlockPage-[^/]+\.js$/, handler("js"));
  return seen;
}

/** Whether the block page's own stylesheet applies: its pager is a flex row. */
const styled = (page: Page) =>
  page
    .getByRole("navigation", { name: "Neighbouring blocks" })
    .evaluate((nav) => getComputedStyle(nav).display);

test("a page whose stylesheet and code fail once loads by itself, styled", async ({ page }) => {
  const seen = await refuse(page, 1);
  await page.goto(BLOCK);
  await expect(page.getByRole("status").filter({ hasText: "Loading this page" })).toBeAttached();
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Block 106", {
    timeout: 15_000,
  });
  expect(await styled(page)).toBe("flex");
  // The script: its failed preload, the request that proves it downloads now, then the page's own
  // import (WebKit's comes from a reloaded page, as it holds the failure; docs/FRONTEND.md).
  expect(seen).toStrictEqual({ css: 2, js: 3 });
});

test("when retries run out, Try again asks for everything again", async ({ page }) => {
  const seen = await refuse(page, 4);
  await page.goto(BLOCK);
  const again = page.getByRole("button", { name: "Try again" });
  await expect(again).toBeVisible({ timeout: 15_000 });
  await expect(page.getByText("Part of it could not be downloaded")).toBeVisible();
  await page.unrouteAll();
  await again.click();
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Block 106");
  expect(await styled(page)).toBe("flex");
  // One stylesheet request per automatic attempt, plus the first; the script was never reached.
  expect(seen.css).toBe(4);
});

test("a tab opened before a deploy is told to reload, and the reload works", async ({ page }) => {
  await page.goto("/regtest/blocks");
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Blocks");
  // The deploy: the old files are gone and the server reports a new version.
  await refuse(page, Number.POSITIVE_INFINITY, 404);
  await page.route("**/app/config", async (route) => {
    const res = await route.fetch();
    const body = (await res.json()) as { version: string };
    await route.fulfill({ response: res, json: { ...body, version: `${body.version}-next` } });
  });
  await page.getByRole("link", { name: "106", exact: true }).click();
  const reload = page.getByRole("button", { name: "Reload" });
  await expect(reload).toBeVisible({ timeout: 10_000 });
  await expect(page.getByText("This site has been updated")).toBeVisible();
  await page.unrouteAll();
  await reload.click();
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Block 106");
  await expect(page).toHaveURL(BLOCK);
});
