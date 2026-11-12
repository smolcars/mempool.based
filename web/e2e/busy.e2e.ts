// The server answers 503 (node busy) or 429 (this client asked for too much) with Retry-After.
// Every page waits as told and shows that it is loading, never an error, while it does.
import type { Page } from "@playwright/test";
import { chain, expect, test } from "./fixtures";

test.use({ consoleErrorsAllowed: [/status of (429|503)/] });

/** Answers the first request to `path` with a busy refusal, then lets requests through. */
async function busyOnce(page: Page, path: string, status: 429 | 503) {
  let refused = 0;
  await page.route(`**/regtest/api${path}`, (route) =>
    refused++ === 0
      ? route.fulfill({
          status,
          headers: { "retry-after": "1" },
          json: { error: status === 503 ? "node busy; retry" : "too many requests" },
        })
      : route.continue(),
  );
  return () => refused;
}

test("a transaction page waits out a busy node and then shows the transaction", async ({
  page,
}) => {
  const asked = await busyOnce(page, `/tx/${chain.txs.script_types}`, 503);
  await page.goto(`/regtest/tx/${chain.txs.script_types}`);
  await expect(page.getByText("Loading transaction")).toBeAttached();
  await expect(page.getByText("Confirmed", { exact: true }).first()).toBeVisible();
  await expect(page.getByRole("alert")).toHaveCount(0);
  expect(asked()).toBe(2);
});

test("an address page waits out a refusal for asking too much", async ({ page }) => {
  const asked = await busyOnce(page, `/address/${chain.addresses.p2wpkh}/txs`, 429);
  await page.goto(`/regtest/address/${chain.addresses.p2wpkh}`);
  await expect(
    page
      .getByRole("table", { name: "Transactions of this address, newest first" })
      .getByRole("link", {
        name: chain.txs.script_types.slice(0, 8),
      }),
  ).toBeVisible();
  await expect(page.getByRole("alert")).toHaveCount(0);
  expect(asked()).toBe(2);
});
