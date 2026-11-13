// Mines on the run's node, so it runs after every other test (the `mines` project).
// The orderings a live address page meets, each built on purpose against the real node and server
// (docs/FRONTEND.md "Live data"). Each test watches an address of its own, and confirms its own
// payments in a block that holds nothing else: later tests count on the scenario's mempool.
import type { Page } from "@playwright/test";
import { chain, expect, test } from "./fixtures";
import { block, freshAddress, gate, keep, pay } from "./regtest";

const rows = (page: Page) => page.getByRole("table").locator("tbody tr");
const unconfirmed = (page: Page) => rows(page).filter({ hasText: "Unconfirmed" });
const live = (page: Page) => page.getByText("Live", { exact: true });

let address = "";
test.beforeEach(async ({ rpc, mine }) => {
  // The fixture waits for the server to sit on the node's tip.
  await mine(0);
  address = freshAddress();
  await block(rpc, address);
});

test("a payment made before the page's socket opens shows once it does", async ({ page, rpc }) => {
  const g = gate();
  await page.routeWebSocket(/\/app\/ws$/, async (ws) => {
    await g.shut;
    ws.connectToServer();
  });
  await page.goto(`/regtest/address/${address}`);
  // Asked after a second without a status, so the payment is made after the page asked.
  await expect(rows(page)).toHaveCount(1);
  const txid = await pay(rpc, address, 50_000n);
  g.open();
  await expect(live(page)).toBeVisible();
  await expect(unconfirmed(page)).toHaveCount(1);
  await expect(rows(page)).toHaveCount(2);
  await block(rpc, chain.addresses.miner, [txid]);
});

test("a payment made between the subscribe and its answer shows", async ({ page, rpc }) => {
  const g = gate();
  let held = false;
  await page.routeWebSocket(/\/app\/ws$/, (ws) => {
    const server = ws.connectToServer();
    ws.onMessage(async (m) => {
      if (typeof m === "string" && m.includes(`"address:`)) {
        held = true;
        await g.shut;
      }
      server.send(m);
    });
  });
  await page.goto(`/regtest/address/${address}`);
  await expect.poll(() => held).toBe(true);
  const txid = await pay(rpc, address, 50_000n);
  g.open();
  await expect(live(page)).toBeVisible();
  await expect(rows(page)).toHaveCount(2);
  await block(rpc, chain.addresses.miner, [txid]);
});

test("a payment made while the socket reconnects shows once it is back", async ({ page, rpc }) => {
  const g = gate();
  let sockets = 0;
  let dropFirst: () => Promise<void> = () => Promise.resolve();
  await page.routeWebSocket(/\/app\/ws$/, async (ws) => {
    sockets += 1;
    if (sockets === 1) dropFirst = () => ws.close({ code: 1012 });
    else await g.shut;
    ws.connectToServer();
  });
  await page.goto(`/regtest/address/${address}`);
  await expect(live(page)).toBeVisible();
  await expect(rows(page)).toHaveCount(1);
  await dropFirst();
  await expect(page.getByText("The live connection dropped")).toBeVisible();
  const txid = await pay(rpc, address, 50_000n);
  g.open();
  await expect(live(page)).toBeVisible();
  await expect(rows(page)).toHaveCount(2);
  await block(rpc, chain.addresses.miner, [txid]);
});

test("two payments in one moment both show, then both confirm", async ({ page, rpc }) => {
  await page.goto(`/regtest/address/${address}`);
  await expect(live(page)).toBeVisible();
  await expect(rows(page)).toHaveCount(1);
  const txids = [await pay(rpc, address, 50_000n), await pay(rpc, address, 60_000n)];
  await expect(unconfirmed(page)).toHaveCount(2);
  await block(rpc, chain.addresses.miner, txids);
  await expect(unconfirmed(page)).toHaveCount(0);
  await expect(rows(page)).toHaveCount(3);
});

test("a block and a payment together both show", async ({ page, rpc }) => {
  await page.goto(`/regtest/address/${address}`);
  await expect(live(page)).toBeVisible();
  await expect(rows(page)).toHaveCount(1);
  const txid = await pay(rpc, address, 50_000n);
  await block(rpc, address);
  await expect(rows(page)).toHaveCount(3);
  await expect(unconfirmed(page)).toHaveCount(1);
  await block(rpc, chain.addresses.miner, [txid]);
  await expect(unconfirmed(page)).toHaveCount(0);
});

// What failed on CI: the totals and the history are separate answers, each kept by the browser
// for a second (`max-age=1`). A history answer the browser kept from a newer second than the
// totals' let a refresh bring new totals and the old history, after which the page stopped asking.
test("a payment shows when the browser holds a history answer from before it", async ({
  page,
  rpc,
}) => {
  await page.goto(`/regtest/address/${address}`);
  await expect(live(page)).toBeVisible();
  await expect(rows(page)).toHaveCount(1);
  // At the start of a later second than the totals' answer, the browser keeps a history answer.
  await keep(page, `/regtest/api/address/${address}/txs`);
  await block(rpc, address);
  await expect(rows(page)).toHaveCount(2);
});
