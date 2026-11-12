import { chain, expect, PROJECTION_LAG_MS, test } from "./fixtures";

test("a confirmed transaction names every output type and links where the money went", async ({
  page,
}) => {
  await page.goto(`/regtest/tx/${chain.txs.script_types}`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Transaction");
  await expect(page.getByText("Confirmed", { exact: true }).first()).toBeVisible();
  await expect(page.getByRole("link", { name: "block 106" })).toHaveAttribute(
    "href",
    `/regtest/block/${chain.blocks[106]}`,
  );

  const outputs = page.getByRole("list", { name: "Outputs" });
  for (const type of ["P2PKH", "P2SH", "P2WPKH", "P2WSH", "P2TR", "Pay to anchor"]) {
    await expect(outputs.getByText(type, { exact: true }).first()).toBeVisible();
  }
  await expect(outputs).toContainText("OP_RETURN “mempool.based regtest”Unspendable");
  // Amounts keep their unit for screen readers, though the cards show bare numbers.
  await expect(outputs.getByText("1.00000000 BTC", { exact: true }).first()).toBeAttached();
  // spend_p2sh_p2a spent the P2SH and P2A outputs.
  await expect(
    outputs.getByRole("link", { name: `Spent in transaction ${chain.txs.spend_p2sh_p2a}` }),
  ).toHaveCount(2);

  await page.getByRole("button", { name: "Details of output 0" }).click();
  await expect(page.getByRole("heading", { name: "Output 0" })).toBeFocused();
  await expect(
    page.getByText(/^OP_DUP OP_HASH160 [0-9a-f]{40} OP_EQUALVERIFY OP_CHECKSIG$/),
  ).toBeVisible();

  await page.reload();
  await expect(page.getByRole("list", { name: "Outputs" })).toContainText("Pay to anchor");
});

test("a coinbase shows as new coins and pays no fee", async ({ page, request }) => {
  const block = await request.get(`/regtest/api/block/${chain.blocks[1]}/txid/0`);
  await page.goto(`/regtest/tx/${await block.text()}`);
  await expect(page.getByRole("list", { name: "Inputs" })).toContainText("Coinbase");
  await expect(page.getByText("None: a coinbase pays no fee")).toBeVisible();
  await expect(page.getByText("Spendable")).toBeVisible();
});

test("the genesis coinbase is never called spendable or unspent", async ({ page, request }) => {
  const genesis = await request.get("/regtest/api/block-height/0");
  const txid = await (
    await request.get(`/regtest/api/block/${await genesis.text()}/txid/0`)
  ).text();
  await page.goto(`/regtest/tx/${txid}`);
  await expect(page.getByText("Never spendable")).toBeVisible();
  await expect(page.getByRole("list", { name: "Outputs" })).toContainText("Unspendable");
  await expect(page.getByRole("list", { name: "Outputs" })).not.toContainText("Unspent");
});

// Two versions of one payment, built by hand like the harness's own: block 9's coinbase (the
// miner script, anyone can spend) back to the miner script at 1 and then 5 sat/vB.
const original =
  "02000000000101c3849571e9ec71a5723146c5b7162c2e915caec6e7beab3de671b2b9b78e35e40000000000fdffffff" +
  "01a0f1052a010000002200204ae81572f06e1b88fd5ced7a1a000945432e83e1551e6f721ee9c00b8cc33260010151" +
  "00000000";
const replacement =
  "02000000000101c3849571e9ec71a5723146c5b7162c2e915caec6e7beab3de671b2b9b78e35e40000000000fdffffff" +
  "0120f0052a010000002200204ae81572f06e1b88fd5ced7a1a000945432e83e1551e6f721ee9c00b8cc33260010151" +
  "00000000";
const originalTxid = "7a2656b2d1c4d3f88f49db20d27a2428d9171705d49691d9d413c72721e61687";

test.describe("transactions the node no longer has", () => {
  // A 404 from the API is the answer these pages expect, not an error.
  test.use({ consoleErrorsAllowed: [/status of 404/] });

  test("a transaction that was replaced is not found, with the reason it may be missing", async ({
    page,
  }) => {
    await page.goto(`/regtest/tx/${chain.txs.rbf_original}`);
    await expect(page.getByText("Transaction not found")).toBeVisible();
  });

  test("an unconfirmed transaction that is replaced names its replacement, without a reload", async ({
    page,
    request,
  }) => {
    // Only regtest allows writes (AGENTS.md); this runs against the test's own node.
    expect(await (await request.post("/regtest/api/tx", { data: original })).text()).toBe(
      originalTxid,
    );
    await page.goto(`/regtest/tx/${originalTxid}`);
    await expect(page.getByText("Unconfirmed", { exact: true })).toBeVisible();
    await expect(page.getByText("Yes, until it confirms")).toBeVisible();
    // The server takes it in at its next cycle; the page then says where it waits.
    await expect(page.getByRole("link", { name: "In the next block" }).first()).toBeVisible({
      timeout: PROJECTION_LAG_MS,
    });

    const replacementTxid = await (
      await request.post("/regtest/api/tx", { data: replacement })
    ).text();
    await expect(page.getByText("Replaced", { exact: true }).first()).toBeVisible({
      timeout: PROJECTION_LAG_MS,
    });
    const notice = page.getByText("This version was replaced").locator("..");
    await expect(notice.getByRole("link", { name: /…/ }).first()).toHaveAttribute(
      "href",
      `/regtest/tx/${replacementTxid}`,
    );
    await expect(notice).toContainText("paying 5.00 sat/vB where this one paid 1.00 sat/vB");
    await expect(page.getByText("No: it was replaced")).toBeVisible();
    await expect(page.getByRole("list", { name: "Outputs" })).toBeVisible();

    // Opened later, the node no longer has it, and the server still says what happened.
    await page.reload();
    await expect(page.getByText("This version was replaced")).toBeVisible();
  });
});

test("an unconfirmed transaction shows where it waits, and leaving lets go of it", async ({
  page,
}) => {
  // Each socket's frames sent, and whether it closed.
  const sockets: { sent: string[]; closed: boolean }[] = [];
  page.on("websocket", (ws) => {
    const s = { sent: [] as string[], closed: false };
    sockets.push(s);
    ws.on("framesent", (f) => s.sent.push(String(f.payload)));
    ws.on("close", () => {
      s.closed = true;
    });
  });
  await page.goto(`/regtest/tx/${chain.txs.payment}`);
  const where = page.getByRole("region", { name: "Where it waits" });
  await expect(where.getByRole("link", { name: "In the next block" })).toHaveAttribute(
    "href",
    "/regtest/block/next",
  );
  await expect(where).toContainText("Regtest has no block schedule");
  await expect(page.getByText("Live", { exact: true })).toBeVisible();
  const topic = `tx:${chain.txs.payment}`;
  const following = sockets.find((s) => s.sent.some((f) => f.includes(topic)));
  expect(following, "a socket that follows the transaction").toBeDefined();
  await page.getByRole("link", { name: "Blocks", exact: true }).click();
  await expect(page).toHaveURL("/regtest/blocks");
  // Released: unsubscribed, or the socket closed with nothing else on it.
  await expect(() => {
    const released =
      following?.closed === true ||
      following?.sent.some((f) => f.includes('"op":"unsubscribe"') && f.includes(topic));
    expect(released, "the transaction's topic released").toBe(true);
  }).toPass();
});

test("a parent shows the rate its child lifts it to, and links the child", async ({ page }) => {
  await page.goto(`/regtest/tx/${chain.txs.cpfp_parent}`);
  await expect(page.getByText("Effective fee rate")).toBeVisible();
  const group = page.getByRole("region", { name: "Mined with" });
  await expect(group).toContainText("but its child pays enough to lift the group to");
  await expect(group.getByRole("link")).toHaveAttribute(
    "href",
    `/regtest/tx/${chain.txs.cpfp_child}`,
  );
});
