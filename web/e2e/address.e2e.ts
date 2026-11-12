import type { Page } from "@playwright/test";
import { chain, expect, test } from "./fixtures";

const stat = (page: Page, label: string) =>
  page
    .locator(".address-stat", { has: page.getByText(label, { exact: true }) })
    .locator("dd")
    .first();

test("a taproot address shows its totals, history and unspent outputs from the chain", async ({
  page,
}) => {
  await page.goto(`/regtest/address/${chain.addresses.p2tr}`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Address");
  await expect(page.getByText("P2TR", { exact: true })).toBeVisible();
  // Block 106 paid it 1 BTC and block 107 0.49 BTC; the RBF replacement in the mempool pays it 0.25.
  await expect(stat(page, "Balance")).toHaveText("1.49000000 BTC");
  await expect(stat(page, "Received")).toHaveText("1.49000000 BTC");
  await expect(stat(page, "Sent")).toHaveText("0.00000000 BTC");
  await expect(stat(page, "Transactions")).toHaveText("3");
  await expect(stat(page, "Unconfirmed")).toHaveText("+0.25000000 BTC");

  const rows = page.getByRole("table").locator("tbody tr");
  await expect(rows).toHaveCount(3);
  await expect(rows.nth(0)).toContainText("Unconfirmed");
  await expect(rows.nth(0)).toContainText("+0.25000000");
  await expect(rows.nth(1)).toContainText("+0.49000000");
  await expect(rows.nth(2)).toContainText("+1.00000000");

  // Its balance over time: two confirmed steps; the unconfirmed payment stays in the figures above.
  const chart = page.getByRole("slider", { name: "Confirmed balance after each block" });
  await expect(chart).toHaveAttribute("aria-valuemax", "1");
  await expect(chart).toHaveAttribute(
    "aria-valuetext",
    /UTC: Balance 1\.49000000 BTC, Block 107, Change \+0\.49000000 BTC$/,
  );
  await chart.press("Home");
  await expect(chart).toHaveAttribute("aria-valuetext", /Balance 1\.00000000 BTC, Block 106,/);
  await rows.nth(1).getByRole("link").click();
  await expect(page).toHaveURL(`/regtest/tx/${chain.txs.spend_p2sh_p2a}`);
  await page.goBack();

  await page.getByRole("tab", { name: "Unspent outputs" }).click();
  await expect(page).toHaveURL(/\?view=unspent$/);
  const outputs = page.getByRole("table").locator("tbody tr");
  await expect(outputs).toHaveCount(3);
  // Tip 110: the output from block 107 has 4 confirmations, the one from 106 has 5.
  await expect(outputs.nth(0)).toContainText("0.25000000");
  await expect(outputs.nth(0)).toContainText("Unconfirmed");
  await expect(outputs.nth(1)).toContainText("0.49000000");
  await expect(outputs.nth(1).locator("td").nth(2)).toHaveText("4");
  await expect(outputs.nth(2)).toContainText("1.00000000");
  await expect(outputs.nth(2).locator("td").nth(2)).toHaveText("5");

  // A reload keeps the view.
  await page.reload();
  await expect(page.getByRole("tab", { name: "Unspent outputs" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
});

test("a spend and a receive net out per transaction", async ({ page }) => {
  await page.goto(`/regtest/address/${chain.addresses.p2sh}`);
  await expect(stat(page, "Balance")).toHaveText("0.00000000 BTC");
  await expect(stat(page, "Sent")).toHaveText("0.50000000 BTC");
  const rows = page.getByRole("table").locator("tbody tr");
  await expect(rows).toHaveCount(2);
  await expect(rows.nth(0)).toContainText("−0.50000000");
  await expect(rows.nth(1)).toContainText("+0.50000000");
  await page.getByRole("tab", { name: "Unspent outputs" }).click();
  await expect(page.getByText("No unspent outputs")).toBeVisible();
});

test("unconfirmed payments are marked and agree with the node's own figures", async ({
  page,
  request,
}) => {
  const address = chain.addresses.p2wpkh;
  const stats = (await (await request.get(`/regtest/api/address/${address}`)).json()) as {
    mempool_stats: { funded_txo_sum: number; spent_txo_sum: number; tx_count: number };
  };
  const summary = (await (
    await request.get(`/regtest/api/address/${address}/txs/summary`)
  ).json()) as { txid: string; value: number }[];
  await page.goto(`/regtest/address/${address}`);
  await expect(stat(page, "Balance")).toHaveText("91.00000000 BTC");
  const pending = stats.mempool_stats.funded_txo_sum - stats.mempool_stats.spent_txo_sum;
  await expect(stat(page, "Unconfirmed")).toHaveText(`+${(pending / 1e8).toFixed(8)} BTC`);

  // The node's summary lists confirmed transactions with this address's net value in each.
  const rows = page.getByRole("table").locator("tbody tr");
  await expect(rows).toHaveCount(summary.length + stats.mempool_stats.tx_count);
  await expect(rows.filter({ hasText: "Unconfirmed" })).toHaveCount(stats.mempool_stats.tx_count);
  for (const s of summary) {
    const row = rows.filter({ has: page.locator(`a[href$="${s.txid}"]`) });
    // The unit is hidden from sight, in the header, but read out with the number.
    await expect(row.locator("td").nth(2)).toHaveText(`+${(s.value / 1e8).toFixed(8)} BTC`);
  }
});

test("history pages 25 confirmed transactions at a time", async ({ page }) => {
  // The miner script has 113 confirmed transactions: the scenario's coinbases and change.
  await page.goto(`/regtest/address/${chain.addresses.miner}`);
  const confirmed = page
    .getByRole("table")
    .locator("tbody tr")
    .filter({ hasNotText: "Unconfirmed" });
  await expect(confirmed).toHaveCount(25);
  const more = page.getByRole("button", { name: "Show 25 more" });
  for (const n of [50, 75, 100, 113]) {
    await more.click();
    await expect(confirmed).toHaveCount(n);
  }
  await expect(more).toHaveCount(0);
  await expect(confirmed.last()).toContainText("+50.00000000");
});

test("an address of another network and a malformed one are named as such", async ({ page }) => {
  await page.goto("/regtest/address/bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
  await expect(page.getByText("This is a Mainnet address")).toBeVisible();
  await expect(page.getByText("This server does not serve Mainnet.")).toBeVisible();

  await page.goto("/regtest/address/bcrt1qnotanaddress");
  await expect(page.getByText("This is not a valid address")).toBeVisible();
});

test("the live line never sits on the address", async ({ page }) => {
  for (const width of [390, 1440]) {
    await page.setViewportSize({ width, height: 900 });
    await page.goto(`/regtest/address/${chain.addresses.p2wpkh}`);
    const live = page.locator(".address-live");
    await expect(live).toBeVisible();
    const a = await page.locator(".address-head .hash--full").boundingBox();
    const b = await live.boundingBox();
    if (!a || !b) throw new Error("missing address or live line");
    expect(b.y, `at ${width} px`).toBeGreaterThanOrEqual(a.y + a.height);
  }
});
