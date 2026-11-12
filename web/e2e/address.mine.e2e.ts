// Mines on the run's node, so it runs after every other test (the `mines` project).
import { chain, expect, test } from "./fixtures";

test("an address page shows a new payment without a reload", async ({ page, rpc, mine }) => {
  await page.goto(`/regtest/address/${chain.addresses.p2wpkh}`);
  await expect(page.getByText("Live", { exact: true })).toBeVisible();
  const rows = page.getByRole("table").locator("tbody tr");
  await expect(rows.first()).toContainText("Unconfirmed");
  const before = await rows.count();
  // `mine` waits for the server to catch up; the block itself is one whose coinbase pays the
  // address, holding nothing from the mempool, so the payment is the only change.
  await mine(0);
  await rpc("generateblock", [chain.addresses.p2wpkh, []]);
  await expect(rows).toHaveCount(before + 1);
  await expect(page.locator("tbody tr.address-row--new")).toHaveCount(1);
});
