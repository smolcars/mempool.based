import { chain, expect, test } from "./fixtures";

test("a block by height shows its facts and transactions, and steps to its neighbours", async ({
  page,
}) => {
  await page.goto("/regtest/block/106");
  const title = page.getByRole("heading", { level: 1 });
  await expect(title).toHaveText("Block 106");
  await expect(page.getByText(chain.blocks[106].slice(0, 8))).toBeVisible();
  // Regtest halves every 150 blocks, so block 106 still earns 50 BTC.
  await expect(page.getByText("50.00000000 BTC subsidy")).toBeVisible();
  const rows = page.getByRole("table", { name: "Transactions in this block" }).getByRole("row");
  await expect(rows).toHaveCount(4); // header, coinbase, script_types, consolidation
  await expect(page.getByRole("cell", { name: "Coinbase" })).toBeVisible();

  await page.getByRole("link", { name: /^Next: 107/ }).click();
  await expect(title).toHaveText("Block 107");
  await page.getByRole("link", { name: /^← Previous: 106/ }).click();
  await expect(title).toHaveText("Block 106");
  await expect(page).toHaveURL(`/regtest/block/${chain.blocks[106]}`);
});

test("the genesis and newest blocks say so", async ({ page }) => {
  await page.goto("/regtest/block/0");
  await expect(page.getByText("Genesis block")).toBeVisible();
  await page.goto(`/regtest/block/${chain.tip.hash}`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText(`Block ${chain.tip.height}`);
  await expect(page.getByText("The newest block")).toBeVisible();
});

test.describe("missing blocks", () => {
  test.use({ consoleErrorsAllowed: [/status of 404/] });

  test("an unknown hash and a height above the tip are explained", async ({ page }) => {
    await page.goto(`/regtest/block/${"ab".repeat(32)}`);
    await expect(page.getByRole("heading", { level: 1 })).toHaveText("Block not found");
    await page.goto("/regtest/block/5000");
    await expect(page.getByRole("heading", { level: 1 })).toHaveText("No block 5 000 yet");
  });
});

test("the blocks list pages back and forward", async ({ page }) => {
  await page.goto("/regtest/blocks");
  const firstHeight = page.getByRole("table").getByRole("row").nth(1).getByRole("link").first();
  await expect(firstHeight).toHaveText(String(chain.tip.height));
  await page.getByRole("link", { name: /Older/ }).click();
  await expect(page).toHaveURL(`/regtest/blocks/${chain.tip.height - 10}`);
  await expect(firstHeight).toHaveText(String(chain.tip.height - 10));
  await page.getByRole("link", { name: /Newer/ }).click();
  await expect(page).toHaveURL("/regtest/blocks");
  await firstHeight.click();
  await expect(page.getByRole("heading", { level: 1 })).toHaveText(`Block ${chain.tip.height}`);
});
