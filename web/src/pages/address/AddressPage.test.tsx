import { createMemoryHistory, MemoryRouter, Route } from "@solidjs/router";
import { render } from "solid-js/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import { clearApiCache } from "../../lib/api";
import type { AddressStats, Transaction, Utxo } from "../../lib/esplora";
import { NNBSP } from "../../lib/format";
import AddressPage from "./AddressPage";

const ME = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4";
const OTHER = "bc1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3qccfmv3";
const txid = (n: number) => n.toString(16).padStart(64, "0");

type Reply = unknown | ((url: string) => Response | Promise<Response>);
let calls: string[] = [];

afterEach(() => {
  vi.unstubAllGlobals();
  clearApiCache();
  calls = [];
  document.body.replaceChildren();
});

/** Answers each API path (after the network's `/api`) from `replies`; anything else is a 404. */
function serve(replies: Record<string, Reply>) {
  vi.stubGlobal("fetch", async (input: string) => {
    const url = String(input);
    calls.push(url);
    const path = url.replace(/^(\/\w+)?\/api/, "");
    const reply = url === "/app/config" ? replies["/app/config"] : replies[path];
    if (reply === undefined) return Response.json({ error: "not found" }, { status: 404 });
    if (typeof reply === "function") return reply(url);
    return Response.json(reply);
  });
}

const refused = (status: number, error: string) => () => Response.json({ error }, { status });
const never = () => new Promise<Response>(() => {});

function open(url: string) {
  const history = createMemoryHistory();
  history.set({ value: url });
  const el = document.createElement("div");
  document.body.append(el);
  render(
    () => (
      <MemoryRouter history={history}>
        <Route path={["/address/:address", "/:net/address/:address"]} component={AddressPage} />
      </MemoryRouter>
    ),
    el,
  );
  return el;
}

const text = (el: Element) => el.textContent ?? "";
const until = (fn: () => void) => vi.waitFor(fn, { timeout: 2000 });

const part = (funded: number, spent: number, txs: number) => ({
  funded_txo_count: funded ? 1 : 0,
  funded_txo_sum: funded,
  spent_txo_count: spent ? 1 : 0,
  spent_txo_sum: spent,
  tx_count: txs,
});
const stats = (chain = part(150_000_000, 50_000_000, 3), pool = part(0, 0, 0)): AddressStats => ({
  address: ME,
  chain_stats: chain,
  mempool_stats: pool,
});
const tx = (n: number, vin: [string, number][], vout: [string, number][], confirmed = true) =>
  ({
    txid: txid(n),
    version: 2,
    locktime: 0,
    fee: 210,
    weight: 840,
    vin: vin.map(([address, value]) => ({
      txid: txid(1000 + n),
      vout: 0,
      is_coinbase: false,
      prevout: {
        scriptpubkey: "",
        scriptpubkey_asm: "",
        scriptpubkey_type: "v0_p2wpkh",
        scriptpubkey_address: address,
        value,
      },
    })),
    vout: vout.map(([address, value]) => ({
      scriptpubkey: "",
      scriptpubkey_asm: "",
      scriptpubkey_type: "v0_p2wpkh",
      scriptpubkey_address: address,
      value,
    })),
    status: confirmed
      ? { confirmed: true, block_height: 900_000 - n, block_time: 1_700_000_000 }
      : { confirmed: false },
  }) satisfies Transaction;

describe("address page", () => {
  it("shows the address, its type and skeletons shaped like the totals while loading", async () => {
    serve({ [`/address/${ME}`]: never, [`/address/${ME}/txs`]: never });
    const el = open(`/address/${ME}`);
    expect(el.querySelector("h1")?.textContent).toBe("Address");
    expect(text(el.querySelector(".address-type") as Element)).toBe("P2WPKH");
    expect(text(el.querySelector(".hash--full") as Element)).toContain("bc1qw508");
    const labels = [...el.querySelectorAll(".address-stat__label")].map(text);
    expect(labels).toEqual(["Balance", "Received", "Sent", "Transactions", "Unconfirmed"]);
    expect(el.querySelectorAll(".address-stat .skeleton")).toHaveLength(10);
    expect(el.querySelectorAll("tbody tr .skeleton").length).toBeGreaterThan(0);
    expect(text(el)).toContain("Loading transactions");
  });

  it("shows totals and each transaction's net change for the address", async () => {
    const page = [
      tx(
        1,
        [[OTHER, 60_000]],
        [
          [ME, 50_000],
          [OTHER, 9_790],
        ],
        false,
      ),
      tx(
        2,
        [[ME, 100_000]],
        [
          [OTHER, 40_000],
          [ME, 59_790],
        ],
      ),
      tx(
        3,
        [
          [ME, 1000],
          [ME, 2000],
        ],
        [[ME, 2790]],
      ),
    ];
    serve({
      [`/address/${ME}`]: stats(part(150_000_000, 50_000_000, 3), part(50_000, 0, 1)),
      [`/address/${ME}/txs`]: page,
    });
    const el = open(`/address/${ME}`);
    await until(() => expect(el.querySelectorAll("tbody tr .hash")).toHaveLength(3));
    const values = [...el.querySelectorAll(".address-stat__value")].map(text);
    expect(values).toEqual([
      "1.00000000 BTC",
      "1.50000000 BTC",
      "0.50000000 BTC",
      "4",
      "+0.00050000 BTC",
    ]);
    const rows = [...el.querySelectorAll("tbody tr")];
    const change = rows.map((r) => text(r.querySelectorAll("td")[2] as Element));
    expect(change).toEqual(["+0.00050000", "−0.00040210", "−0.00000210"]);
    expect(text(rows[0] as Element)).toContain("Unconfirmed");
    expect(rows[1]?.querySelector("a")?.getAttribute("href")).toBe(`/tx/${txid(2)}`);
    expect(text(el.querySelector(".section__meta") as Element)).toBe("3 of 4");
  });

  it("loads the next page after the last txid seen and appends it", async () => {
    const first = Array.from({ length: 25 }, (_, i) => tx(i + 1, [[OTHER, 2000]], [[ME, 1000]]));
    serve({
      [`/address/${ME}`]: stats(part(26_000, 0, 26)),
      [`/address/${ME}/txs`]: first,
      [`/address/${ME}/txs?after_txid=${txid(25)}`]: [tx(26, [[ME, 5000]], [[OTHER, 4000]])],
    });
    const el = open(`/address/${ME}`);
    await until(() => expect(el.querySelectorAll("tbody tr")).toHaveLength(25));
    const more = [...el.querySelectorAll("button")].find((b) => text(b) === "Show 25 more");
    more?.click();
    await until(() => expect(el.querySelectorAll("tbody tr")).toHaveLength(26));
    expect(text(el.querySelectorAll("tbody tr")[25] as Element)).toContain("−0.00005000");
    expect([...el.querySelectorAll("button")].some((b) => text(b) === "Show 25 more")).toBe(false);
  });

  it("says plainly when an address has never been used", async () => {
    serve({ [`/address/${ME}`]: stats(part(0, 0, 0)), [`/address/${ME}/txs`]: [] });
    const el = open(`/address/${ME}`);
    await until(() => expect(text(el)).toContain("This address has never been used"));
    expect(el.querySelector('[role="alert"]')).toBeNull();
    expect(text(el.querySelector(".address-stat__value") as Element)).toBe("0.00000000 BTC");
  });

  it("answers a malformed address without asking the server, as text", async () => {
    serve({});
    const el = open(`/address/${encodeURIComponent('<img src=x onerror="alert(1)">')}`);
    expect(text(el)).toContain("This is not a valid address");
    expect(text(el)).toContain("Mainnet addresses start with bc1, 1 or 3");
    expect(text(el.querySelector(".address-head__raw") as Element)).toBe(
      '<img src=x onerror="alert(1)">',
    );
    expect(el.querySelector("img")).toBeNull();
    expect(calls).toEqual([]);
  });

  it("treats an address the server rejects as malformed", async () => {
    serve({
      [`/address/${ME}`]: refused(400, "invalid address"),
      [`/address/${ME}/txs`]: refused(400, "invalid address"),
    });
    const el = open(`/address/${ME}`);
    await until(() => expect(text(el)).toContain("This is not a valid address"));
  });

  it("points a test-network address on mainnet to the network that serves it", async () => {
    serve({ "/app/config": { networks: ["mainnet", "signet"], version: "0" } });
    const tb = "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx";
    const el = open(`/address/${tb}`);
    expect(text(el)).toContain("This is a Signet or Testnet4 address");
    await until(() => expect(el.querySelector(".address-elsewhere a")).not.toBeNull());
    const links = [...el.querySelectorAll(".address-elsewhere a")];
    expect(links.map((a) => [text(a), a.getAttribute("href")])).toEqual([
      ["Open it on Signet", `/signet/address/${tb}`],
    ]);
    expect(calls).toEqual(["/app/config"]);
  });

  it("says when no enabled network serves the address", async () => {
    serve({ "/app/config": { networks: ["signet"], version: "0" } });
    const el = open(`/signet/address/${ME}`);
    await until(() => expect(text(el)).toContain("This server does not serve Mainnet."));
  });

  it("shows the history of an address too large to total, and says why", async () => {
    serve({
      [`/address/${ME}`]: refused(
        503,
        "address history too large for this route; use the paged /txs routes",
      ),
      [`/address/${ME}/txs`]: [tx(1, [[OTHER, 2000]], [[ME, 1000]])],
    });
    const el = open(`/address/${ME}?view=unspent`);
    await until(() => expect(text(el)).toContain("Totals are not available for this address"));
    expect(text(el)).toContain("exchange and mining pool addresses");
    expect(text(el)).toContain("Unspent outputs are not available for this address");
    expect(el.querySelector('[role="alert"]')).toBeNull();
    // The unspent outputs are not asked for: the node refuses them for the same reason.
    expect(calls.some((c) => c.endsWith("/utxo"))).toBe(false);
    const tab = [...el.querySelectorAll<HTMLButtonElement>('[role="tab"]')][0];
    tab?.click();
    await until(() => expect(el.querySelectorAll("tbody tr .hash")).toHaveLength(1));
    expect(text(el.querySelector(".section__meta") as Element)).toBe("1 shown");
  });

  it("says the server does not index addresses, never that the address is empty", async () => {
    const off = refused(503, "address index is disabled on this server");
    serve({ [`/address/${ME}`]: off, [`/address/${ME}/txs`]: off });
    const el = open(`/address/${ME}`);
    await until(() => expect(text(el)).toContain("This server does not index addresses"));
    expect(text(el)).toContain("This says nothing about this address or its funds");
    expect(text(el)).not.toContain("never been used");
    expect(el.querySelector(".address-stats")).toBeNull();
  });

  it("offers a retry when the node is unavailable", async () => {
    let up = false;
    serve({
      [`/address/${ME}`]: () =>
        up ? Response.json(stats()) : refused(502, "upstream unavailable")(),
      [`/address/${ME}/txs`]: () =>
        up ? Response.json([]) : refused(502, "upstream unavailable")(),
    });
    const el = open(`/address/${ME}`);
    await until(() => expect(text(el)).toContain("Could not load this address"));
    expect(text(el)).toContain("Upstream unavailable.");
    up = true;
    const retry = [...el.querySelectorAll("button")].find((b) => text(b) === "Try again");
    retry?.click();
    await until(() => expect(text(el)).toContain("1.00000000 BTC"));
    expect(text(el)).not.toContain("Could not load");
  });

  it("lists unspent outputs, unconfirmed first, with confirmations from the tip", async () => {
    const utxo = (n: number, value: number, height?: number): Utxo => ({
      txid: txid(n),
      vout: n,
      value,
      status: height ? { confirmed: true, block_height: height } : { confirmed: false },
    });
    serve({
      [`/address/${ME}`]: stats(),
      [`/address/${ME}/txs`]: [],
      [`/address/${ME}/utxo`]: [utxo(1, 1000, 100), utxo(2, 2000), utxo(3, 3000, 110)],
      "/blocks/tip/height": () => new Response("110"),
    });
    const el = open(`/address/${ME}?view=unspent`);
    expect(el.querySelector('[role="tab"][aria-selected="true"]')?.textContent).toBe(
      "Unspent outputs",
    );
    await until(() => expect(el.querySelectorAll("tbody tr .hash")).toHaveLength(3));
    await until(() => expect(text(el.querySelectorAll("tbody tr")[2] as Element)).toContain("11"));
    const rows = [...el.querySelectorAll("tbody tr")].map((r) => [
      text(r.querySelector(".address-outpoint") as Element),
      ...[...r.querySelectorAll("td")].slice(1).map(text),
    ]);
    expect(rows).toEqual([
      ["00000000…00000002:2", "0.00002000", "Unconfirmed"],
      ["00000000…00000003:3", "0.00003000", "1"],
      ["00000000…00000001:1", "0.00001000", "11"],
    ]);
    expect(text(el.querySelector(".section__meta") as Element)).toBe("3 outputs");
  });

  it("moves between the views with the arrow keys", async () => {
    serve({
      [`/address/${ME}`]: never,
      [`/address/${ME}/txs`]: never,
      [`/address/${ME}/utxo`]: never,
    });
    const el = open(`/address/${ME}`);
    const tabs = [...el.querySelectorAll<HTMLButtonElement>('[role="tab"]')];
    tabs[0]?.focus();
    tabs[0]?.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    await until(() => expect(tabs[1]?.getAttribute("aria-selected")).toBe("true"));
    expect(document.activeElement).toBe(tabs[1]);
    expect(tabs[0]?.tabIndex).toBe(-1);
  });

  it("groups digits in counts", async () => {
    serve({
      [`/address/${ME}`]: stats(part(1, 0, 12_345)),
      [`/address/${ME}/txs`]: never,
    });
    const el = open(`/address/${ME}`);
    await until(() =>
      expect(text(el.querySelectorAll(".address-stat__value")[3] as Element)).toBe(`12${NNBSP}345`),
    );
  });
});
