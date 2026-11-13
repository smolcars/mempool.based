// The projected-block page against the real server: the scenario leaves transactions unconfirmed.
import { chain, expect, test } from "./fixtures";

// The scenario's live mempool; api.e2e.ts may broadcast one more, so the count is not asserted.
const WAITING = [
  chain.txs.payment,
  chain.txs.rbf_replacement,
  chain.txs.cpfp_parent,
  chain.txs.cpfp_child,
];

test("the next block shows the waiting transactions as squares and rows", async ({ page }) => {
  await page.goto("/regtest/block/next");
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Next block");
  const list = page.getByRole("table", {
    name: "Transactions in the next block, highest fee rate first",
  });
  for (const txid of WAITING) {
    await expect(list.locator(`a.hash__text[href="/regtest/tx/${txid}"]`)).toBeVisible();
  }
  // The replaced original is not waiting any more.
  await expect(
    list.locator(`a.hash__text[href="/regtest/tx/${chain.txs.rbf_original}"]`),
  ).toHaveCount(0);
  // One square per row, drawn by the grid from the same records.
  const rows = await list.locator("tbody tr").count();
  const canvas = page.getByRole("img", { name: /^Map of the next block's/ });
  await expect(canvas).toHaveAttribute("data-squares", String(rows));
  // The parent lifted by its child is mined at their chunk's rate, and says what it pays itself.
  const parent = list.locator("tr", {
    has: page.locator(`a.hash__text[href="/regtest/tx/${chain.txs.cpfp_parent}"]`),
  });
  await expect(parent).toContainText("own 1.00");
  await expect(page.getByText("Live", { exact: true })).toBeVisible();
});

test("the second block is empty on regtest, and says so", async ({ page }) => {
  await page.goto("/regtest/block/next/2");
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Second block from now");
  await expect(page.getByText("Nothing waiting here")).toBeVisible();
  await page.getByRole("link", { name: "← Next block" }).click();
  await expect(page).toHaveURL(/\/regtest\/block\/next$/);
});

test("a place that does not exist says so", async ({ page }) => {
  await page.goto("/regtest/block/next/9");
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("No such projected block");
});
