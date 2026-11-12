import { createMemoryHistory, MemoryRouter, Route } from "@solidjs/router";
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearApiCache } from "../../lib/api";
import type { Outspend, Transaction, TxStatus } from "../../lib/esplora";
import TransactionPage, { POLL_MS } from "./TransactionPage";

const TXID = "d1d7cf2b9189a07521354c5d5012c3147d9825596b6ec6cc3100904f0785fcfe";
const SPENDER = "3207f7d34cbb4c3210986ed8f357e5ebeb8189716f571d5c08731e5e46e7fab4";
const BLOCK = "0000000000000000000159affbb5acfb646390efa277f0feeabc2429b5388c36";
const confirmed: TxStatus = {
  confirmed: true,
  block_height: 975_516,
  block_hash: BLOCK,
  block_time: 1_794_453_849,
};

const out = (value: number, scriptpubkey: string, address?: string) => ({
  scriptpubkey,
  scriptpubkey_asm: "",
  scriptpubkey_type: "unknown" as const,
  value,
  ...(address ? { scriptpubkey_address: address } : {}),
});
const P2WPKH = `0014${"05982249066c3cabd6265565b81bb024acc28488"}`;
const payment = (status: TxStatus): Transaction => ({
  txid: TXID,
  version: 2,
  locktime: 0,
  size: 222,
  weight: 561,
  fee: 141,
  status,
  vin: [
    {
      txid: SPENDER,
      vout: 25,
      is_coinbase: false,
      sequence: 0xfffffffd,
      scriptsig: "",
      witness: ["3045", "02aa"],
      prevout: out(100_000, P2WPKH, "bc1qqkvzyjgxds72h43x24jmsxasyjkv9pyg4wn5uu"),
    },
  ],
  vout: [out(60_000, P2WPKH, "bc1qpay"), out(39_859, P2WPKH, "bc1qchange")],
});

type Reply = { status: number; body: unknown } | (() => { status: number; body: unknown });
let replies: Record<string, Reply>;
const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
  const url = String(input);
  const r = replies[url];
  const reply =
    typeof r === "function" ? r() : (r ?? { status: 404, body: { error: "not found" } });
  const text = typeof reply.body === "string" ? reply.body : JSON.stringify(reply.body);
  return new Response(text, {
    status: reply.status,
    headers: { "cache-control": "public, max-age=1" },
  });
});

const disposers: (() => void)[] = [];
beforeEach(() => {
  clearApiCache();
  fetchMock.mockClear();
  vi.stubGlobal("fetch", fetchMock);
});
afterEach(() => {
  for (const d of disposers.splice(0)) d();
  document.body.replaceChildren();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

function page(path: string): HTMLElement {
  const history = createMemoryHistory();
  history.set({ value: path });
  const el = document.createElement("div");
  document.body.append(el);
  disposers.push(
    render(
      () => (
        <MemoryRouter history={history}>
          <Route path={["/tx/:txid", "/regtest/tx/:txid"]} component={TransactionPage} />
        </MemoryRouter>
      ),
      el,
    ),
  );
  return el;
}

const settle = async () => {
  for (let i = 0; i < 10; i++) await new Promise((r) => setTimeout(r, 0));
};
const text = (el: Element) => el.textContent?.replace(/\s+/g, " ") ?? "";
const api = (path: string) => `/api${path}`;
const serve = (tx: Transaction, spends: Outspend[] = [], tip = "975518") => {
  replies = {
    [api(`/tx/${tx.txid}`)]: { status: 200, body: tx },
    [api(`/tx/${tx.txid}/outspends`)]: { status: 200, body: spends },
    [api("/blocks/tip/height")]: { status: 200, body: tip },
  };
};

describe("TransactionPage", () => {
  it("shows a confirmed payment: status, block, confirmations, fee and where the money went", async () => {
    serve(payment(confirmed), [
      { spent: true, txid: SPENDER, vin: 0, status: confirmed },
      { spent: false },
    ]);
    const el = page(`/tx/${TXID}`);
    await settle();
    expect(el.querySelector(".tx-status")?.textContent).toBe("Confirmed");
    const block = el.querySelector<HTMLAnchorElement>(".tx-step--done a");
    expect(block?.getAttribute("href")).toBe(`/block/${BLOCK}`);
    expect(text(el)).toContain("3 confirmations of 6 so far.");
    // 141 sats over ceil(561 / 4) = 141 vB.
    expect(text(el)).toContain("1.00 sat/vB");
    expect(text(el)).toContain("No: it is confirmed");
    const outs = el.querySelectorAll(".tx-flow__col--out .tx-flow__item");
    expect(outs).toHaveLength(3); // two outputs and the fee
    expect(outs[0]?.querySelector("a.tx-flow__spent")?.getAttribute("href")).toBe(`/tx/${SPENDER}`);
    expect(text(outs[1] as Element)).toContain("Unspent");
    expect(el.querySelector('.tx-flow__col--out a[href="/address/bc1qpay"]')).not.toBeNull();
  });

  it("names a coinbase as what it is", async () => {
    const coinbase: Transaction = {
      ...payment(confirmed),
      fee: 0,
      vin: [
        {
          txid: "00".repeat(32),
          vout: 0xffffffff,
          is_coinbase: true,
          sequence: 0,
          scriptsig: "039ce20e",
        },
      ],
      vout: [out(314_059_121, `a914${"0b".repeat(20)}87`, "32i1m6gNcSHwiPX9nfTNXVjme9j5DU8y5g")],
    };
    serve(coinbase, [{ spent: false }], "975517");
    const el = page(`/tx/${TXID}`);
    await settle();
    expect(text(el.querySelector(".tx-flow__col--in") as Element)).toContain("Coinbase");
    expect(text(el)).toContain("None: a coinbase pays no fee");
    expect(text(el)).toContain("Spendable");
    expect(text(el)).toContain("2 confirmations of the 100 a miner's reward needs");
    el.querySelector<HTMLButtonElement>(".tx-flow__col--in .tx-flow__open")?.click();
    await settle();
    expect(text(el.querySelector("#tx-inspector") as Element)).toContain(
      "Block height in script975 516",
    );
  });

  it("notices when an unconfirmed transaction confirms, without a reload", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    serve(payment({ confirmed: false }));
    let status: TxStatus = { confirmed: false };
    replies[api(`/tx/${TXID}/status`)] = () => ({ status: 200, body: status });
    const el = page(`/tx/${TXID}`);
    await settle();
    expect(el.querySelector(".tx-status")?.textContent).toBe("Unconfirmed");
    expect(text(el)).toContain("Waiting for a block");
    expect(text(el)).toContain("Yes, until it confirms");

    vi.advanceTimersByTime(POLL_MS);
    await settle();
    expect(el.querySelector(".tx-status")?.textContent).toBe("Unconfirmed");

    status = { ...confirmed };
    replies[api("/blocks/tip/height")] = { status: 200, body: "975516" };
    vi.advanceTimersByTime(POLL_MS);
    await settle();
    expect(el.querySelector(".tx-status")?.textContent).toBe("Confirmed");
    expect(el.querySelector(".tx-step--landed")).not.toBeNull();
    expect(text(el)).toContain("1 confirmation of 6 so far.");
    expect(text(el.querySelector("p[aria-live=polite]") as Element)).toBe(
      "Confirmed in block 975 516.",
    );
  });

  it("handles a transaction that vanishes while unconfirmed", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    serve(payment({ confirmed: false }));
    replies[api(`/tx/${TXID}/status`)] = { status: 404, body: { error: "not found" } };
    const el = page(`/tx/${TXID}`);
    await settle();
    vi.advanceTimersByTime(POLL_MS);
    await settle();
    expect(el.querySelector(".tx-status")?.textContent).toBe("Removed");
    expect(text(el)).toContain("No longer in the mempool");
    expect(text(el)).toContain("No: it is no longer in the mempool");
    // The page keeps what it knew; it does not fall back to "not found".
    expect(el.querySelectorAll(".tx-flow__item")).not.toHaveLength(0);
    // And it stops asking.
    const calls = fetchMock.mock.calls.length;
    vi.advanceTimersByTime(POLL_MS * 3);
    await settle();
    expect(fetchMock.mock.calls.length).toBe(calls);
  });

  it("goes back to unconfirmed when its block leaves the chain", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    serve(payment(confirmed), [], "975516");
    replies[api(`/tx/${TXID}/status`)] = { status: 200, body: { confirmed: false } };
    const el = page(`/tx/${TXID}`);
    await settle();
    expect(el.querySelector(".tx-status")?.textContent).toBe("Confirmed");
    vi.advanceTimersByTime(POLL_MS);
    await settle();
    expect(el.querySelector(".tx-status")?.textContent).toBe("Unconfirmed");
    expect(text(el)).toContain("Waiting for a block");
    expect(text(el.querySelector("p[aria-live=polite]") as Element)).toContain("Unconfirmed again");
  });

  it("reveals a long witness a hundred items at a time", async () => {
    const witness = Array.from({ length: 300 }, (_, i) => i.toString(16).padStart(2, "0"));
    const tx = payment(confirmed);
    const first = tx.vin?.[0];
    if (!first) throw new Error("fixture has an input");
    serve({ ...tx, vin: [{ ...first, witness }] });
    const el = page(`/tx/${TXID}`);
    await settle();
    el.querySelector<HTMLButtonElement>(".tx-flow__col--in .tx-flow__open")?.click();
    await settle();
    const items = () => el.querySelectorAll(".tx-witness .tx-code").length;
    const more = () => el.querySelector<HTMLButtonElement>(".tx-witness > .btn");
    expect(items()).toBe(24);
    expect(more()?.textContent).toBe("Show 100 more of 300 items");
    more()?.click();
    await settle();
    expect(items()).toBe(124);
    more()?.click();
    more()?.click();
    await settle();
    expect(items()).toBe(300);
    expect(more()).toBeNull();
  });

  it("says when the transaction is not found", async () => {
    replies = {};
    const el = page(`/tx/${TXID}`);
    await settle();
    expect(text(el)).toContain("Transaction not found");
    expect(text(el)).toContain("belong to another network");
  });

  it("refuses bad input without asking the server", async () => {
    replies = {};
    const el = page("/tx/not-a-txid");
    await settle();
    expect(text(el)).toContain("That is not a transaction ID");
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("offers a retry when the node is unavailable, and retries every request that failed", async () => {
    const down = { status: 503, body: { error: "node unavailable" } };
    replies = {
      [api(`/tx/${TXID}`)]: down,
      [api(`/tx/${TXID}/outspends`)]: down,
      [api("/blocks/tip/height")]: down,
    };
    const el = page(`/tx/${TXID}`);
    await settle();
    expect(text(el)).toContain("Could not load this transaction");
    expect(text(el)).toContain("Node unavailable.");
    serve(payment(confirmed), [
      { spent: true, txid: SPENDER, vin: 0, status: confirmed },
      { spent: false },
    ]);
    el.querySelector<HTMLButtonElement>(".state--error .btn")?.click();
    await settle();
    expect(el.querySelector(".tx-status")?.textContent).toBe("Confirmed");
    expect(el.querySelector("a.tx-flow__spent")?.getAttribute("href")).toBe(`/tx/${SPENDER}`);
    expect(text(el)).toContain("3 confirmations of 6 so far.");
  });

  it("says when spending could not be checked instead of waiting forever", async () => {
    serve(payment(confirmed));
    replies[api(`/tx/${TXID}/outspends`)] = { status: 503, body: { error: "node unavailable" } };
    const el = page(`/tx/${TXID}`);
    await settle();
    el.querySelector<HTMLButtonElement>(".tx-flow__col--out .tx-flow__open")?.click();
    await settle();
    expect(text(el.querySelector("#tx-inspector") as Element)).toContain(
      "Could not check right now",
    );
  });

  it("shows ten outputs of a big transaction, then a hundred more per press", async () => {
    const big = {
      ...payment(confirmed),
      vout: Array.from({ length: 1500 }, (_, i) => out(1000 + i, P2WPKH, `bc1q${i}`)),
    };
    serve(big);
    const el = page(`/tx/${TXID}`);
    await settle();
    const count = () =>
      el.querySelectorAll(
        ".tx-flow__col--out .tx-flow__item[data-key]:not([data-key=more]):not([data-key=fee])",
      ).length;
    expect(count()).toBe(10);
    const more = el.querySelector<HTMLButtonElement>(".tx-flow__col--out .tx-flow__more");
    expect(more?.textContent).toBe("Show 100 more");
    expect(text(el.querySelector(".tx-flow__item--more") as Element)).toContain(
      "1 490 more outputs",
    );
    more?.click();
    await settle();
    expect(count()).toBe(110);
  });

  it("renders hostile OP_RETURN text as inert text and opens its details", async () => {
    const hostile = '<img src=x onerror="alert(1)">‮';
    const bytes = [...new TextEncoder().encode(hostile)]
      .map((b) => b.toString(16).padStart(2, "0"))
      .join("");
    const tx = {
      ...payment(confirmed),
      vout: [out(0, `6a${(bytes.length / 2).toString(16)}${bytes}`)],
    };
    serve(tx);
    const el = page(`/tx/${TXID}`);
    await settle();
    expect(el.querySelector("img")).toBeNull();
    const card = el.querySelector(".tx-flow__col--out .tx-flow__item") as Element;
    expect(text(card)).toContain('OP_RETURN “<img src=x onerror="alert(1)">\\u{202e}”');
    expect(text(card)).toContain("Unspendable");
    const open = card.querySelector<HTMLButtonElement>(".tx-flow__open");
    open?.click();
    await settle();
    expect(open?.getAttribute("aria-expanded")).toBe("true");
    const inspector = el.querySelector("#tx-inspector") as Element;
    expect(text(inspector)).toContain("Output 0");
    expect(inspector.querySelector(".tx-code__text")?.textContent).toBe(
      '<img src=x onerror="alert(1)">\\u{202e}',
    );
    expect(el.querySelector("img")).toBeNull();
  });
});
