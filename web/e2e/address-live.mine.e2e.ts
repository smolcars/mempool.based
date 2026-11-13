// Mines on the run's node, so it runs after every other test (the `mines` project).
// The orderings a live address page meets, each built on purpose against the real node and server
// (docs/FRONTEND.md "Live data"). Each test watches an address of its own, and confirms its own
// payments in a block that holds nothing else: later tests count on the scenario's mempool.
import { createHash, randomBytes } from "node:crypto";
import type { Page } from "@playwright/test";
import { chain, expect, test } from "./fixtures";

/** P2WSH of OP_TRUE: the harness's miner script (docs/REGTEST.md), spent with the witness [OP_TRUE]. */
const MINER_SCRIPT = "00204ae81572f06e1b88fd5ced7a1a000945432e83e1551e6f721ee9c00b8cc33260";
type Rpc = <T>(method: string, params?: unknown[]) => Promise<T>;

const B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
const sha256 = (b: Buffer) => createHash("sha256").update(b).digest();

/** A regtest P2PKH address of random bytes, which nobody has used. */
function freshAddress(): string {
  const body = Buffer.concat([Buffer.from([0x6f]), randomBytes(20)]);
  let n = BigInt(`0x${Buffer.concat([body, sha256(sha256(body)).subarray(0, 4)]).toString("hex")}`);
  let s = "";
  for (; n > 0n; n /= 58n) s = `${B58[Number(n % 58n)]}${s}`;
  return s;
}

const le = (n: bigint, bytes: number) =>
  Array.from({ length: bytes }, (_, i) =>
    ((n >> BigInt(8 * i)) & 0xffn).toString(16).padStart(2, "0"),
  ).join("");

/** Coinbases this file has spent, so two payments in a row never take the same one. */
const spent = new Set<string>();

/** Pays `sats` to `address` from a mature coinbase of the miner script, as the harness does. */
async function pay(rpc: Rpc, address: string, sats: bigint): Promise<string> {
  const { scriptPubKey } = await rpc<{ scriptPubKey: string }>("validateaddress", [address]);
  for (let h = (await rpc<number>("getblockcount")) - 100; h > 0; h--) {
    const id = (await rpc<{ tx: string[] }>("getblock", [await rpc("getblockhash", [h]), 1])).tx[0];
    if (id === undefined || spent.has(id)) continue;
    // An unspent output's details, or null (which the helper reports as a failure) when spent.
    const out = await rpc<{ value: number; scriptPubKey: { hex: string } }>("gettxout", [id, 0])
      .then((o) => (o.scriptPubKey.hex === MINER_SCRIPT ? o : null))
      .catch(() => null);
    if (out === null) continue;
    spent.add(id);
    const change = BigInt(Math.round(out.value * 1e8)) - sats - 10_000n;
    const prev = Array.from({ length: 32 }, (_, i) => id.slice(62 - 2 * i, 64 - 2 * i)).join("");
    const script = (hex: string) => `${(hex.length / 2).toString(16).padStart(2, "0")}${hex}`;
    const raw =
      `02000000` +
      `0001` +
      `01${prev}00000000` +
      `00fdffffff` +
      `02${le(sats, 8)}${script(scriptPubKey)}${le(change, 8)}${script(MINER_SCRIPT)}` +
      `010151` +
      `00000000`;
    return rpc<string>("sendrawtransaction", [raw]);
  }
  throw new Error("a mature coinbase is unspent");
}

/** A block whose coinbase pays `address`, holding `txids` and nothing else from the mempool. */
const block = (rpc: Rpc, address: string, txids: string[] = []) =>
  rpc("generateblock", [address, txids]);

const rows = (page: Page) => page.getByRole("table").locator("tbody tr");
const unconfirmed = (page: Page) => rows(page).filter({ hasText: "Unconfirmed" });
const live = (page: Page) => page.getByText("Live", { exact: true });

/** A gate a socket route waits at until the test opens it. */
function gate() {
  let open: () => void = () => undefined;
  const shut = new Promise<void>((resolve) => {
    open = resolve;
  });
  return { open, shut };
}

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
  await page.waitForTimeout(1000 + 20 - (Date.now() % 1000));
  await page.evaluate(
    (url) => fetch(url, { headers: { accept: "application/json" } }).then((r) => r.text()),
    `/regtest/api/address/${address}/txs`,
  );
  await block(rpc, address);
  await expect(rows(page)).toHaveCount(2);
});
