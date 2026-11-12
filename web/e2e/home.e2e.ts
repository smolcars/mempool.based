import { chain, expect, test } from "./fixtures";

test("the home page shows the scenario's blocks, live, with the mempool and fees", async ({
  page,
  request,
}) => {
  await page.goto("/regtest/");
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Blocks");
  await expect(page.getByRole("status").filter({ hasText: "Live" })).toBeVisible();

  const tiles = page
    .getByRole("list", { name: "Mined blocks, newest first" })
    .getByRole("listitem");
  await expect(tiles).toHaveCount(10);
  const newest = tiles.first().getByRole("link");
  await expect(newest).toContainText("110");
  await expect(newest).toHaveAttribute("href", `/regtest/block/${chain.tip.hash}`);
  await expect(page.getByText("Next block", { exact: true }).last()).toBeVisible();

  // The figures are the node's: another test may broadcast meanwhile, so compare with a fresh answer.
  await expect(async () => {
    const mempool = (await (await request.get("/regtest/api/mempool")).json()) as { count: number };
    await expect(page.locator(".home-stat").first()).toContainText(String(mempool.count), {
      timeout: 100,
    });
  }).toPass({ timeout: 15_000 });
  await expect(page.getByRole("heading", { name: "Fee to pay now" })).toBeVisible();
  await expect(
    page.getByRole("table", { name: "The newest transactions waiting in the mempool" }),
  ).toContainText(chain.txs.cpfp_child.slice(0, 8));
});

test("the home page fits a phone without scrolling sideways", async ({ page }) => {
  await page.setViewportSize({ width: 320, height: 700 });
  await page.goto("/regtest/");
  await expect(
    page.getByRole("list", { name: "Mined blocks, newest first" }).getByRole("listitem"),
  ).toHaveCount(10);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(320);
});

test("each tile shows its block's strata and fee figures, and a coinbase-only block says so", async ({
  page,
}) => {
  await page.goto("/regtest/");
  const lane = page.getByRole("list", { name: "Mined blocks, newest first" });
  // Blocks 106 and 107 hold the scenario's transactions; the rest only a coinbase.
  const paying = lane.getByRole("link", { name: /^Block 106,/ });
  await expect(paying).toHaveAccessibleName(/median fee \d+\.\d\d sat\/vB, fee range/);
  await expect(paying.locator(".strata__layer").first()).toBeVisible();
  await expect(paying).toContainText("sat/vB");
  const one = lane.getByRole("link", { name: /^Block 107,/ });
  await expect(one.locator(".strata__layer")).toHaveCount(1);
  const empty = lane.getByRole("link", { name: /^Block 105,/ });
  await expect(empty).toContainText("No fees");
  await expect(empty).toContainText("Coinbase only");
  await expect(empty.locator(".strata__layer")).toHaveCount(0);
});

test("regtest says it never adjusts its difficulty", async ({ page }) => {
  await page.goto("/regtest/");
  await expect(
    page.getByRole("region", { name: "Difficulty adjustment" }).getByText(/never adjusts/),
  ).toBeVisible();
});

test("the unconfirmed scenario transactions wait in the projected next block", async ({ page }) => {
  await page.goto("/regtest/");
  const projected = page.getByRole("list", { name: "Projected blocks, next first" });
  const next = projected.getByRole("listitem").first();
  // Four from the scenario, and maybe the broadcast test's.
  await expect(next).toContainText(/Next block, projected: \d+ transactions, median fee about/);
  const label = (await next.locator(".visually-hidden").textContent()) ?? "";
  expect(Number(/projected: (\d+)/.exec(label)?.[1])).toBeGreaterThanOrEqual(4);
  await expect(next.getByRole("link")).toHaveCount(0);
  await expect(page.getByText("Everything waiting fits in the next block.")).toBeVisible();
});

test("a cold load asks for each thing once, and nothing the mempool topics send", async ({
  page,
}) => {
  const asked: string[] = [];
  page.on("request", (r) => {
    const path = new URL(r.url()).pathname;
    if (path.includes("/api/") || path.includes("/app/")) asked.push(path.replace("/regtest", ""));
  });
  await page.goto("/regtest/");
  await expect(page.getByRole("list", { name: "Projected blocks, next first" })).toBeVisible();
  await expect(page.locator(".home-stats")).not.toHaveAttribute("aria-busy", "true");
  // Long enough for a second round of requests, as a new tip would cause. (The panels' fallback
  // polling, after the topics stay silent for 5 s, is covered by the unit tests.)
  await page.waitForTimeout(1_500);
  const counts = new Map<string, number>();
  for (const p of asked) counts.set(p, (counts.get(p) ?? 0) + 1);
  expect(
    [...counts.values()].every((n) => n === 1),
    JSON.stringify([...counts]),
  ).toBe(true);
  expect(counts.get("/api/blocks")).toBe(1);
  expect(counts.get("/api/fee-estimates")).toBe(1);
  expect(counts.has("/api/mempool")).toBe(false);
  expect(counts.has("/api/mempool/recent")).toBe(false);
});
