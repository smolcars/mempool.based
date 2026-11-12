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
  const next = projected.getByRole("link").first();
  // Four from the scenario, and maybe the broadcast test's.
  await expect(next).toHaveAccessibleName(/^Next block, projected: \d+ transactions, median fee/);
  const label = (await next.getAttribute("aria-label")) ?? "";
  expect(Number(/projected: (\d+)/.exec(label)?.[1])).toBeGreaterThanOrEqual(4);
  // A phone hides the note to give the tiles room (HomePage.css).
  const note = page.getByText("Everything waiting fits in the next block.");
  await ((page.viewportSize()?.width ?? 0) > 640
    ? expect(note).toBeVisible()
    : expect(note).toBeAttached());
  // It opens that block's page.
  await next.click();
  await expect(page).toHaveURL(/\/regtest\/block\/next$/);
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
  expect(counts.has("/api/mempool")).toBe(false);
  expect(counts.has("/api/mempool/recent")).toBe(false);
  expect(counts.has("/api/v1/fees/precise")).toBe(false);
});

test("the fee panel shows what the server says now, and no number when it has none", async ({
  page,
  request,
}) => {
  await page.goto("/regtest/");
  const panel = page.getByRole("region", { name: "Fee to pay now" });
  // Regtest's node makes no estimates, and the server's own wait five minutes after it starts, so
  // this is usually no answer; compare with the route at the same moment either way.
  await expect(async () => {
    const res = await request.get("/regtest/api/v1/fees/precise");
    if (res.status() === 503) {
      await expect(panel).toContainText("No recommendation right now", { timeout: 100 });
    } else {
      const fees = (await res.json()) as { fastestFee: number };
      await expect(panel).toContainText(fees.fastestFee.toFixed(3), { timeout: 100 });
    }
  }).toPass({ timeout: 15_000 });
});

test("the fee panel follows the fees topic: an update, the node's fallback, then none", async ({
  page,
}) => {
  // The server's own answer needs five minutes of the mempool, longer than this suite: the topic's
  // messages are played into the real socket, which carries everything else as it is.
  let send: ((m: object) => void) | undefined;
  await page.routeWebSocket(/\/app\/ws$/, (ws) => {
    const server = ws.connectToServer();
    ws.onMessage((m) => server.send(m));
    server.onMessage((m) => {
      // The server's own `fees` messages are held back, so only the played ones are seen.
      if (typeof m === "string" && m.includes('"topic":"fees"')) return;
      ws.send(m);
    });
    send = (m) => ws.send(JSON.stringify(m));
  });
  await page.goto("/regtest/");
  await expect(page.getByRole("status").filter({ hasText: "Live" })).toBeVisible();
  const panel = page.getByRole("region", { name: "Fee to pay now" });
  const levels = (rates: number[], source: string) => ({
    topic: "fees",
    type: "fees",
    source,
    tip: null,
    levels: ["high", "medium", "low", "no_rush"].map((name, i) => ({
      name,
      blocks: [1, 3, 6, 144][i],
      minutes: [10, 30, 60, 1440][i],
      rate: rates[i],
    })),
    min_fee_rate: 100,
  });
  const high = panel.getByRole("definition").last();

  send?.(levels([1031, 500, 200, 100], "projected"));
  await expect(high).toHaveText("1.031 sat/vB");
  await expect(panel).toContainText("read from the projected blocks");
  // The mempool filled up: the next message is the new answer.
  send?.(levels([2501, 1031, 200, 100], "projected"));
  await expect(high).toHaveText("2.501 sat/vB");
  send?.(levels([3000, 1000, 500, 120], "node"));
  await expect(high).toHaveText("3.000 sat/vB");
  await expect(panel).toContainText("The node's own estimates");
  send?.({ topic: "fees", type: "unavailable" });
  await expect(panel).toContainText("No recommendation right now");
  await expect(panel).not.toContainText("3.000");
});
