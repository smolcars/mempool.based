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
}

export async function stubLarge(page: Page, stub: LargeStub = {}): Promise<void> {
  const size = stub.size === undefined ? { creates: 3_665_660, cap: 10_000 } : stub.size;
  const mempool = stub.mempool ?? (() => [chain.txs.payment, chain.txs.consolidation]);
  await page.route(new RegExp(`/regtest/api/address/${LARGE}$`), (route) =>
    route.fulfill({
      status: 503,
      headers: { "retry-after": "1" },
      json: { error: "address history too large for this server" },
    }),
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
