import { chain, expect, test } from "./fixtures";

// The scenario's blocks are stamped from 2026-01-01, ten minutes apart, by no pool on the list.

test("regtest's mining page says what is and is not meaningful there", async ({ page }) => {
  await page.goto("/regtest/mining");
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Mining");
  await expect(
    page.getByText(/There are no pools and the hashrate is not meaningful/),
  ).toBeVisible();
  await expect(
    page.getByText("Regtest never adjusts its difficulty", { exact: false }),
  ).toBeVisible();
  // Nothing was mined in the last week: the page says so and offers all of history.
  const pools = page.getByRole("region", { name: "Pools" });
  await expect(pools.getByText("No blocks stored for the last week")).toBeVisible({
    timeout: 30_000,
  });
  await pools.getByRole("link", { name: "See all stored history" }).click();
  await expect(page).toHaveURL(/\/regtest\/mining\?period=all$/);
  await expect(page.getByRole("link", { name: /all time/ })).toHaveAttribute(
    "aria-current",
    "page",
  );
  const table = pools.getByRole("table", { name: /Pools ranked by blocks found/ });
  const unknown = table.getByRole("row").nth(1);
  await expect(unknown).toContainText("Unknown");
  await expect(unknown).toContainText("100.0%");
});

test("a chart draws and its readout shows the route's figure", async ({ page }) => {
  const route = "/regtest/api/v1/mining/blocks/rewards/all";
  const chart = page.getByRole("slider", { name: /Average reward per block/ });
  let reward = "";
  // The server refreshes its answer while the backfill runs: compare until both see the same one.
  await expect(async () => {
    const points = (await (await page.request.get(route)).json()) as { avgRewards: number }[];
    reward = `${((points.at(-1)?.avgRewards ?? 0) / 1e8).toFixed(8)} BTC`;
    await page.goto("/regtest/mining?period=all");
    await page.getByRole("button", { name: "Rewards" }).click();
    await expect(chart).toHaveAttribute(
      "aria-valuetext",
      new RegExp(`Reward per block ${reward}`),
      {
        timeout: 3_000,
      },
    );
  }).toPass({ timeout: 30_000 });
  const section = page.getByRole("region", { name: "Blocks over time" });
  await expect(section.locator("svg path").first()).toBeAttached();
  // The pointer on the chart: the readout shows the point under it.
  const box = await chart.boundingBox();
  if (box) await page.mouse.move(box.x + box.width - 20, box.y + box.height / 2);
  await expect(section.getByText(reward, { exact: true })).toBeVisible();
  // Keyboard: the chart takes focus and moves with the arrow keys.
  await chart.focus();
  await expect(chart).toBeFocused();
  await page.keyboard.press("Home");
  await expect(chart).toHaveAttribute("aria-valuenow", "0");
  // The figures behind the chart are a table a screen reader can reach.
  await section.getByText("Show the figures").click();
  await expect(section.getByRole("cell", { name: reward })).toBeVisible();
});

test("the Unknown pool's page lists the chain's blocks, linked", async ({ page }) => {
  await page.goto("/regtest/mining/pool/unknown");
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Unknown", {
    timeout: 30_000,
  });
  const blocks = page.getByRole("table", { name: /Blocks found by Unknown/ });
  await expect(blocks.getByRole("link", { name: String(chain.tip.height) })).toHaveAttribute(
    "href",
    `/regtest/block/${chain.tip.hash}`,
  );
});
