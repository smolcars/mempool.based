// Replacements made on the run's own node, and one mined. It broadcasts and mines, so it runs with
// the other `*.mine.e2e.ts` files, after every other test: new mempool transactions would push the
// scenario's out of the home page's latest transactions, which home.e2e.ts reads.
import type { APIRequestContext } from "@playwright/test";
import { chain, expect, PROJECTION_LAG_MS, test } from "./fixtures";
import { tapRow } from "./rows";

/** P2WSH of OP_TRUE: the harness's miner script (docs/REGTEST.md), spent with the witness [OP_TRUE]. */
const MINER_SCRIPT = "00204ae81572f06e1b88fd5ced7a1a000945432e83e1551e6f721ee9c00b8cc33260";
const SIGNALS = 0xfffffffd;
const FINAL = 0xffffffff;

const le = (n: bigint | number, bytes: number) =>
  Array.from({ length: bytes }, (_, i) =>
    ((BigInt(n) >> BigInt(8 * i)) & 0xffn).toString(16).padStart(2, "0"),
  ).join("");
const reversed = (txid: string) =>
  Array.from({ length: 32 }, (_, i) => txid.slice(62 - 2 * i, 64 - 2 * i)).join("");

/** A transaction spending one miner-script output to miner-script outputs, built by hand like the harness's own. */
function spend(txid: string, vout: number, sequence: number, outputs: bigint[]): string {
  return (
    `02000000` +
    `0001` +
    `01${reversed(txid)}${le(vout, 4)}00${le(sequence, 4)}` +
    `${le(outputs.length, 1)}${outputs.map((v) => `${le(v, 8)}22${MINER_SCRIPT}`).join("")}` +
    `010151` +
    `00000000`
  );
}

/** Waits until the server's mempool engine holds `txid`, so a replacement is seen replacing it. */
async function seen(request: APIRequestContext, txid: string) {
  await expect
    .poll(
      async () =>
        (
          (await (await request.get(`/regtest/api/v1/transaction-times?txId[]=${txid}`)).json()) as
            | number[]
            | undefined
        )?.[0] ?? 0,
      { timeout: PROJECTION_LAG_MS },
    )
    .toBeGreaterThan(0);
}

test("replacements made now appear without a reload, filter, open, and confirm", async ({
  page,
  request,
  rpc,
  mine,
}) => {
  // Coins of our own: the scenario's payment returns its change to the miner script (output 1).
  const change = await rpc<{ value: number; scriptPubKey: { hex: string } }>("gettxout", [
    chain.txs.payment,
    1,
    true,
  ]);
  expect(change.scriptPubKey.hex).toBe(MINER_SCRIPT);
  const total = BigInt(Math.round(change.value * 1e8));
  const coin = 500_000_000n;
  const fanout = await rpc<string>("sendrawtransaction", [
    spend(chain.txs.payment, 1, SIGNALS, [coin, coin, total - 2n * coin - 1_000n]),
  ]);
  // One input, one output: 95.25 vB as the server counts it, so these fees are 1.05, 2.10 and
  // 4.20 sat/vB.
  const version = (vout: number, sequence: number, fee: bigint) =>
    rpc<string>("sendrawtransaction", [spend(fanout, vout, sequence, [coin - fee])]);

  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto("/regtest/rbf");
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Replacements");

  // A chain of two replacements, each made once the server had seen the version before it.
  const a = await version(0, SIGNALS, 100n);
  await seen(request, a);
  const b = await version(0, SIGNALS, 200n);
  await seen(request, b);
  const c = await version(0, SIGNALS, 400n);
  const chainStory = page.locator(`[data-story="${c}"]`);
  await expect(chainStory).toBeVisible({ timeout: PROJECTION_LAG_MS });
  const versions = chainStory.locator(".rbf-v");
  await expect(versions).toHaveCount(3);
  for (const [i, txid] of [a, b, c].entries()) {
    await expect(versions.nth(i)).toContainText(txid.slice(0, 8));
  }
  const chainRow = page.locator("tbody tr", { has: chainStory });
  await expect(chainRow).toContainText("+3.15");
  await expect(chainRow).toContainText("Not mined");
  await expect(chainRow).not.toContainText("Full RBF");

  // A replacement of a transaction that did not signal: full RBF.
  const x = await version(1, FINAL, 100n);
  await seen(request, x);
  const y = await version(1, FINAL, 300n);
  const fullStory = page.locator(`[data-story="${y}"]`);
  await expect(fullStory).toBeVisible({ timeout: PROJECTION_LAG_MS });
  await expect(page.locator("tbody tr", { has: fullStory })).toContainText("Full RBF");
  await expect(fullStory).toContainText("did not signal");

  await page.getByRole("link", { name: "Full RBF only" }).click();
  await expect(page.locator(`[data-story="${y}"]`)).toBeVisible();
  await expect(page.locator(`[data-story="${c}"]`)).toHaveCount(0);
  await page.getByRole("link", { name: "All", exact: true }).click();
  await expect(page.locator(`[data-story="${c}"]`)).toBeVisible();

  // The home page lists the newest few; a tap on a row opens the replacement that stands.
  await page.goto("/regtest/");
  const panel = page.getByRole("table", { name: "The newest replacements, newest first" });
  await expect(panel.locator(`a[href="/regtest/tx/${y}"]`).first()).toBeAttached();
  await tapRow(page, panel, "the home page's replacements");
  await expect(page).toHaveURL(`/regtest/tx/${y}`);
  await page.goto("/regtest/");
  await page.getByRole("link", { name: "All replacements" }).click();
  await expect(page).toHaveURL("/regtest/rbf");

  // Mined while the page watches: the story says so without a reload.
  await expect(chainStory).toBeVisible();
  await mine(1);
  await expect(page.locator("tbody tr", { has: chainStory })).toContainText("Mined", {
    timeout: PROJECTION_LAG_MS,
  });
  await expect(page.locator("tbody tr", { has: chainStory })).not.toContainText("Not mined");
  await tapRow(page, page.getByRole("table", { name: "Replacements, newest first" }), "/rbf");
});
