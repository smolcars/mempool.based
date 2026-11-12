import type { Page } from "@playwright/test";
import { chain, expect, test } from "./fixtures";

// A regtest block hash has no leading zeros, so search asks for a transaction first and gets a 404.
test.use({ consoleErrorsAllowed: [/status of 404/] });

const search = (page: Page) => page.getByRole("search").getByRole("searchbox", { name: "Search" });

const cases = [
  { kind: "a height", query: "#106", url: "/regtest/block/106" },
  {
    kind: "a block hash",
    query: chain.blocks[106].toUpperCase(),
    url: `/regtest/block/${chain.blocks[106]}`,
  },
  { kind: "an outpoint", query: `${chain.txs.payment}:0`, url: `/regtest/tx/${chain.txs.payment}` },
  {
    kind: "an address",
    query: ` ${chain.addresses.p2tr} `,
    url: `/regtest/address/${chain.addresses.p2tr}`,
  },
  {
    kind: "a bitcoin: URI",
    query: `bitcoin:${chain.addresses.p2pkh}?amount=1`,
    url: `/regtest/address/${chain.addresses.p2pkh}`,
  },
];

for (const c of cases) {
  test(`${c.kind} lands on its page from the "/" shortcut`, async ({ page }) => {
    await page.goto("/regtest/");
    const field = search(page);
    await page.keyboard.press("/");
    await expect(field).toBeFocused();
    await field.fill(c.query);
    await field.press("Enter");
    await expect(page).toHaveURL(c.url);
    await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
    await expect(field).toHaveValue("");
  });
}

test("a pasted link to another explorer opens the page here", async ({ page }) => {
  await page.goto("/regtest/blocks");
  const field = search(page);
  await field.fill(`https://mempool.space/testnet4/tx/${chain.txs.consolidation}#vin=0`);
  await expect(page.getByRole("search")).toContainText("Transaction");
  await field.press("Enter");
  await expect(page).toHaveURL(`/regtest/tx/${chain.txs.consolidation}`);
});

test("nothing found says what was searched and what search finds", async ({ page }) => {
  await page.goto("/regtest/");
  const field = search(page);
  const missing = "ab".repeat(32);
  await field.fill(missing);
  await field.press("Enter");
  const box = page.getByRole("search");
  await expect(box.getByText("Nothing matches", { exact: true })).toBeVisible();
  await expect(box.getByTitle(missing)).toBeVisible();
  await expect(box.getByText("the whole address")).toBeVisible();
  await expect(page).toHaveURL("/regtest/");
  await expect(field).toHaveValue(missing);
  await field.press("Escape");
  await expect(box.getByText("Nothing matches", { exact: true })).toBeHidden();
});
