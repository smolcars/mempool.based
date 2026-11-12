import { chain, expect, test } from "./fixtures";

test("the API passes the scenario's chain through from the node", async ({ request }) => {
  const height = await request.get("/regtest/api/blocks/tip/height");
  expect(height.ok()).toBe(true);
  expect(await height.text()).toBe(String(chain.tip.height));

  const block = await request.get(`/regtest/api/block/${chain.blocks[106]}`);
  expect(block.ok()).toBe(true);
  expect(await block.json()).toMatchObject({ id: chain.blocks[106], height: 106 });

  const tx = await request.get(`/regtest/api/tx/${chain.txs.script_types}`);
  expect(tx.ok()).toBe(true);
  expect(await tx.json()).toMatchObject({
    txid: chain.txs.script_types,
    status: { confirmed: true, block_height: 106 },
  });
});

test("an unknown API path is a JSON 404", async ({ request }) => {
  const res = await request.get("/regtest/api/no/such/route");
  expect(res.status()).toBe(404);
  expect(res.headers()["content-type"]).toContain("application/json");
  expect(await res.json()).toEqual({ error: "not found" });
});

// Spends the coinbase of block 10 (50 BTC to the miner script, anyone can spend) back to the
// miner script, paying 1,000 sats. Built by hand like the harness's own transactions.
const coinbase10 = "547d53d4e4290dac860915c0c88f8212b3aeb440c400a0b31bc92ccb77dfa527";
const spendHex =
  "0200000000010127a5df77cb2cc91bb3a000c440b4aeb312828fc8c0150986ac0d29e4d4537d540000000000fdffffff" +
  "0118ee052a010000002200204ae81572f06e1b88fd5ced7a1a000945432e83e1551e6f721ee9c00b8cc33260010151" +
  "00000000";
const spendTxid = "fc40c92d9763e584976a4c55b50b231c214f2a5ae0514dadc8e1bf0958d68fa6";

test("a broadcast reaches the node's mempool", async ({ request }) => {
  // Only regtest allows writes (AGENTS.md); this runs against the test's own node.
  const outspend = await request.get(`/regtest/api/tx/${coinbase10}/outspend/0`);
  expect(await outspend.json()).toEqual({ spent: false });

  const res = await request.post("/regtest/api/tx", { data: spendHex });
  expect(await res.text()).toBe(spendTxid);
  expect(res.status()).toBe(200);

  // Asked by txid: the mempool lists are held for a second and may predate the broadcast.
  const tx = await request.get(`/regtest/api/tx/${spendTxid}`);
  expect(await tx.json()).toMatchObject({ txid: spendTxid, status: { confirmed: false } });
});
