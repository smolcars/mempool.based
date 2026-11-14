// An address above the node's `--max-sh-creates`, played by a real regtest address: the node's
// answers that regtest cannot give (its refusal of the totals, the size the server learned from
// it, its unconfirmed transactions) are stubbed, and everything else is the real server's.
import type { Page, Route } from "@playwright/test";
import { chain } from "./fixtures";

export const LARGE = chain.addresses.p2pkh;

/** The node's count and cap as the server passes them on; `null` for a size it does not know. */
export interface LargeStub {
  size?: { creates: number; cap: number } | null;
  /** The unconfirmed transactions at the moment of each ask, as txids on the regtest chain. */
  mempool?: () => readonly string[];
  /** What `/app/address/:a/ledger` says at each ask; `{ state: "none" }` by default. */
  ledger?: () => unknown;
}

/** The totals the server answers from its ledger for a kept address, in the node's shape. */
export const KEPT_STATS = {
  address: LARGE,
  chain_stats: {
    funded_txo_count: 3_700_000,
    funded_txo_sum: 2_000_000_000_000_000,
    spent_txo_count: 3_699_000,
    spent_txo_sum: 1_999_990_000_000_000,
    tx_count: 3_800_000,
  },
  mempool_stats: {
    funded_txo_count: 1,
    funded_txo_sum: 50_000_000,
    spent_txo_count: 0,
    spent_txo_sum: 0,
    tx_count: 1,
  },
};

/** A ledger keeping {@link LARGE}: three newest transactions and a short chart. */
export const KEPT = {
  state: "tracked",
  chain_stats: KEPT_STATS.chain_stats,
  balance: 10_000_000_000,
  at: { height: 110, hash: "00".repeat(32), time: 1_767_291_600 },
  seeded: { height: 105, time: 1_767_288_600 },
  window: 200,
  filling: false,
  txs: [
    { txid: chain.txs.payment, height: 110, position: 2, value: 50_000_000, time: 1_767_291_600 },
    {
      txid: chain.txs.consolidation,
      height: 108,
      position: 1,
      value: -1_234_567_890,
      time: 1_767_290_400,
    },
    { txid: chain.txs.cpfp_child, height: 106, position: 3, value: 2_100, time: 1_767_289_200 },
  ],
  points: [
    { height: 105, time: 1_767_288_600, balance: 11_184_565_790 },
    { height: 106, time: 1_767_289_200, balance: 11_184_567_890 },
    { height: 108, time: 1_767_290_400, balance: 9_950_000_000 },
    { height: 110, time: 1_767_291_600, balance: 10_000_000_000 },
  ],
};

export async function stubLarge(page: Page, stub: LargeStub = {}): Promise<void> {
  const size = stub.size === undefined ? { creates: 3_665_660, cap: 10_000 } : stub.size;
  const mempool = stub.mempool ?? (() => [chain.txs.payment, chain.txs.consolidation]);
  const ledger = stub.ledger ?? (() => ({ state: "none" }));
  // The server answers a kept address's totals from its ledger, and refuses them otherwise.
  await page.route(new RegExp(`/regtest/api/address/${LARGE}$`), (route) =>
    (ledger() as { state?: string }).state === "tracked"
      ? route.fulfill({ json: KEPT_STATS })
      : route.fulfill({
          status: 503,
          headers: { "retry-after": "1" },
          json: { error: "address history too large for this server" },
        }),
  );
  await page.route(new RegExp(`/regtest/app/address/${LARGE}/ledger$`), (route) =>
    route.fulfill({ json: ledger(), headers: { "cache-control": "no-store" } }),
  );
  await page.route(new RegExp(`/regtest/app/address/${LARGE}/size$`), (route) =>
    size === null
      ? route.fulfill({ status: 404, json: { error: "size not known" } })
      : route.fulfill({ json: size }),
  );
  // Real transactions from the chain, shown as unconfirmed and paying this address. Each is
  // fetched once, and again once if the connection drops (seen once in WebKit's run).
  const known = new Map<string, Promise<Unconfirmed>>();
  const read = async (route: Route, txid: string) => {
    const url = new URL(`/regtest/api/tx/${txid}`, route.request().url()).href;
    const fetched = await route.fetch({ url }).catch(async () => route.fetch({ url }));
    const tx = (await fetched.json()) as Unconfirmed;
    tx.status = { confirmed: false };
    const first = tx.vout[0];
    if (first) first.scriptpubkey_address = LARGE;
    return tx;
  };
  await page.route(new RegExp(`/regtest/api/address/${LARGE}/txs/mempool$`), async (route) => {
    for (const txid of mempool()) if (!known.has(txid)) known.set(txid, read(route, txid));
    const txs = await Promise.all(mempool().map(async (txid) => known.get(txid)));
    await route.fulfill({ json: txs, headers: { "cache-control": "public, max-age=1" } });
  });
}

interface Unconfirmed {
  status: object;
  vout: { scriptpubkey_address?: string }[];
}
