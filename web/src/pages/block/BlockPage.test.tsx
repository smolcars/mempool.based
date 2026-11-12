import { createMemoryHistory, MemoryRouter, Route } from "@solidjs/router";
import { render } from "solid-js/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import { clearApiCache } from "../../lib/api";
import type { Block, BlockStatus, Transaction } from "../../lib/esplora";
import BlocksPage from "../blocks/BlocksPage";
import BlockPage from "./BlockPage";

const HASH = "00000000000000000001b0e1f2c63fd0a1ec0d0b4f50f1c1e2f9b8c1d2e3f4a5";
const PREV = "00000000000000000002aa1d55e93c43a9ab7fa3e3c88f7c5d7f2c4f04b5c6d7";
const NEXT = "00000000000000000000ff0e1c2b3a495867768594a3b2c1d0e0f1a2b3c4d5e6";

const block: Block = {
  id: HASH,
  height: 975_411,
  version: 0x20000000,
  timestamp: 1_794_000_000,
  tx_count: 30,
  size: 1_600_000,
  weight: 3_990_000,
  merkle_root: "ab".repeat(32),
  previousblockhash: PREV,
  mediantime: 1_793_998_000,
  nonce: 12_345,
  bits: 386_000_300,
  difficulty: 148_364_561_003_867.4,
};

const tx = (i: number, extra: Partial<Transaction> = {}): Transaction => ({
  txid: i.toString(16).padStart(64, "0"),
  version: 2,
  locktime: 0,
  fee: 1000,
  weight: 800,
  vin: [{ txid: "11".repeat(32), vout: 0, is_coinbase: false }],
  vout: [{ value: 5000, scriptpubkey: "", scriptpubkey_asm: "", scriptpubkey_type: "p2pkh" }],
  status: { confirmed: true },
  ...extra,
});

// "<img src=x onerror=alert(1)>" and a NUL as the miner's tag, after the BIP34 height push for 975 411.
const hostileTag = `0333e20e1d${Array.from(new TextEncoder().encode("<img src=x onerror=alert(1)>"), (b) => b.toString(16).padStart(2, "0")).join("")}00`;
const coinbase = tx(0, {
  fee: 0,
  vin: [{ txid: "00".repeat(32), vout: 0xffffffff, is_coinbase: true, scriptsig: hostileTag }],
  vout: [
    { value: 313_000_000, scriptpubkey: "", scriptpubkey_asm: "", scriptpubkey_type: "v0_p2wpkh" },
  ],
});

type Reply = { status?: number; body: unknown };
let replies: Record<string, Reply | ((n: number) => Reply)> = {};
const calls: string[] = [];

function serve(table: typeof replies) {
  replies = table;
  calls.length = 0;
  vi.stubGlobal("fetch", async (input: string) => {
    const path = input.replace(/^(\/(signet|testnet4|regtest))?\/api/, "");
    calls.push(path);
    const r = replies[path];
    const reply = typeof r === "function" ? r(calls.filter((c) => c === path).length) : r;
    if (!reply) return Response.json({ error: "not found" }, { status: 404 });
    const status = reply.status ?? 200;
    // Block JSON is immutable, as the server marks it, so the client keeps it in memory.
    const headers = /^\/block\/[0-9a-f]{64}$/.test(path)
      ? { "cache-control": "public, max-age=31536000, immutable" }
      : {};
    return typeof reply.body === "string"
      ? new Response(reply.body, { status, headers })
      : Response.json(reply.body, { status, headers });
  });
}

const status = (s: Partial<BlockStatus> = {}): Reply => ({
  body: { in_best_chain: true, height: block.height, next_best: NEXT, ...s },
});
const normal = () => ({
  [`/block/${HASH}`]: { body: block },
  [`/block/${HASH}/status`]: status(),
  [`/block/${HASH}/txs/0`]: {
    body: [coinbase, ...Array.from({ length: 24 }, (_, i) => tx(i + 1))],
  },
  [`/block/${HASH}/txs/25`]: { body: Array.from({ length: 5 }, (_, i) => tx(i + 25)) },
  "/blocks/tip/height": { body: "975420" },
  "/block-height/975411": { body: HASH },
});

afterEach(() => {
  vi.unstubAllGlobals();
  clearApiCache();
  document.body.replaceChildren();
});

let history = createMemoryHistory();

function open(url: string) {
  history = createMemoryHistory();
  history.set({ value: url });
  const el = document.createElement("div");
  document.body.append(el);
  render(
    () => (
      <MemoryRouter history={history}>
        <Route path={["/block/:id", "/signet/block/:id"]} component={BlockPage} />
        <Route path={["/blocks", "/blocks/:from"]} component={BlocksPage} />
      </MemoryRouter>
    ),
    el,
  );
  return el;
}

const text = (el: Element) => (el.textContent ?? "").replaceAll(" ", " ");
const fact = (el: Element, label: string) =>
  text(
    [...el.querySelectorAll(".block-fact")]
      .find((f) => f.querySelector("dt")?.textContent === label)
      ?.querySelector("dd") ?? document.createElement("dd"),
  );
const until = <T,>(fn: () => T) => vi.waitFor(fn, { timeout: 2000 });

describe("BlockPage", () => {
  it("shows skeletons until the data arrives", () => {
    vi.stubGlobal("fetch", () => new Promise(() => {}));
    const el = open(`/block/${HASH}`);
    expect(el.querySelectorAll(".block-skel").length).toBeGreaterThan(10);
    expect(el.querySelector(".hash--full")?.textContent).toContain(HASH.slice(0, 8));
  });

  it("shows a block by hash with reward, fees and neighbours", async () => {
    serve(normal());
    const el = open(`/block/${HASH}`);
    await until(() => expect(fact(el, "Reward")).toContain("3.13000000 BTC"));
    expect(text(el.querySelector("h1") as Element)).toBe("Block 975 411");
    expect(fact(el, "Total fees")).toContain("500 000 sats");
    expect(fact(el, "Reward")).toContain("3.12500000 BTC subsidy");
    expect(fact(el, "Confirmations")).toContain("10");
    expect(fact(el, "Weight")).toContain("99.8% of the limit");
    expect(fact(el, "Miner")).toContain("Not available yet");
    expect(fact(el, "Bits")).toContain("0x1701e5ac");
    const prev = el.querySelector<HTMLAnchorElement>('a[rel="prev"]');
    const next = el.querySelector<HTMLAnchorElement>('a[rel="next"]');
    expect(prev?.getAttribute("href")).toBe(`/block/${PREV}`);
    expect(text(next as Element)).toContain("975 412");
    expect(el.querySelectorAll("tbody tr")).toHaveLength(25);
    expect(text(el.querySelector("tbody tr") as Element)).toContain("Coinbase");
    expect(document.title).toBe("Block 975 411 | mempool.based".replace(" 4", " 4"));
  });

  it("shows the right header after moving between cached blocks", async () => {
    const prevBlock = {
      ...block,
      id: PREV,
      height: block.height - 1,
      version: 4,
      previousblockhash: null,
    };
    serve({
      ...normal(),
      [`/block/${PREV}`]: { body: prevBlock },
      [`/block/${PREV}/status`]: status({ height: prevBlock.height, next_best: HASH }),
      [`/block/${PREV}/txs/0`]: { body: [coinbase] },
    });
    const el = open(`/block/${HASH}`);
    await until(() => expect(fact(el, "Version")).toBe("0x20000000"));
    history.set({ value: `/block/${PREV}` });
    await until(() => expect(fact(el, "Version")).toBe("0x00000004"));
    history.set({ value: `/block/${HASH}` });
    await until(() => expect(text(el.querySelector("h1") as Element)).toBe("Block 975 411"));
    expect(fact(el, "Version")).toBe("0x20000000");
  });

  it("renders a hostile coinbase tag as text", async () => {
    serve(normal());
    const el = open(`/block/${HASH}`);
    await until(() => expect(fact(el, "Coinbase tag")).toContain("<img src=x onerror=alert(1)>"));
    expect(el.querySelector("img")).toBeNull();
    expect(el.querySelector(".block-tag__hidden")?.textContent).toBe("·");
  });

  it("loads more transactions below the first page", async () => {
    serve(normal());
    const el = open(`/block/${HASH}`);
    const button = await until(() => {
      const b = [...el.querySelectorAll("button")].find((x) => x.textContent === "Show 5 more");
      expect(b).toBeDefined();
      return b as HTMLButtonElement;
    });
    button.click();
    await until(() => expect(el.querySelectorAll("tbody tr")).toHaveLength(30));
    expect(text(el.querySelector(".section__meta") as Element)).toContain("30 of 30");
    expect([...el.querySelectorAll("button")].some((x) => x.textContent?.includes("more"))).toBe(
      false,
    );
  });

  it("resolves a height, and marks the genesis and the newest block", async () => {
    serve({
      ...normal(),
      [`/block/${HASH}`]: { body: { ...block, previousblockhash: null, height: 0 } },
      [`/block/${HASH}/status`]: status({ next_best: null, height: 0 }),
      "/block-height/0": { body: HASH },
    });
    const el = open("/block/0");
    await until(() =>
      expect(text(el.querySelector(".block-pager") as Element)).toContain("Genesis block"),
    );
    expect(text(el.querySelector(".block-pager") as Element)).toContain("The newest block");
    expect(calls).toContain("/block-height/0");
  });

  it("marks a block that is not on the best chain", async () => {
    serve({
      ...normal(),
      [`/block/${HASH}/status`]: status({ in_best_chain: false, next_best: null, height: null }),
    });
    const el = open(`/block/${HASH}`);
    await until(() => expect(el.querySelector(".block-status")?.textContent).toContain("Stale"));
    expect(text(el.querySelector(".block-notice") as Element)).toContain("not on the best chain");
    expect(fact(el, "Confirmations")).toContain("0");
    expect(el.querySelector('a[rel="next"]')).toBeNull();
  });

  it("says when a hash is unknown", async () => {
    serve({ "/blocks/tip/height": { body: "975420" } });
    const el = open(`/block/${"ab".repeat(32)}`);
    await until(() => expect(el.querySelector("h1")?.textContent).toBe("Block not found"));
  });

  it("says when a height is above the tip", async () => {
    serve({ "/blocks/tip/height": { body: "975420" } });
    const el = open("/block/99999999");
    await until(() =>
      expect(text(el.querySelector("h1") as Element)).toBe("No block 99 999 999 yet"),
    );
    await until(() => expect(text(el)).toContain("The newest block is 975 420"));
  });

  it("refuses input that is neither a hash nor a height", () => {
    serve({});
    const el = open("/block/%3Cscript%3Ex");
    expect(el.querySelector("h1")?.textContent).toBe("Not a block");
    expect(text(el)).toContain("“<script>x” is not a block hash or a height");
    expect(el.querySelector("script")).toBeNull();
    expect(calls).toHaveLength(0);
  });

  it("offers a retry when the node is unavailable", async () => {
    const down = { status: 503, body: { error: "node unavailable" } };
    serve({ ...normal(), [`/block/${HASH}`]: (n) => (n === 1 ? down : { body: block }) });
    const el = open(`/block/${HASH}`);
    const retry = await until(() => {
      const b = el.querySelector<HTMLButtonElement>(".state--error button");
      expect(b).not.toBeNull();
      return b as HTMLButtonElement;
    });
    expect(text(el)).toContain("Node unavailable.");
    retry.click();
    await until(() => expect(text(el.querySelector("h1") as Element)).toBe("Block 975 411"));
  });

  it("uses the network's halving interval", async () => {
    serve(normal());
    const el = open(`/signet/block/${HASH}`);
    await until(() => expect(fact(el, "Reward")).toContain("3.12500000 BTC subsidy"));
  });
});

describe("BlocksPage", () => {
  const list = (from: number) =>
    Array.from({ length: Math.min(10, from + 1) }, (_, i) => ({
      ...block,
      id: `${from - i}`.padStart(64, "0"),
      height: from - i,
    }));

  it("lists the newest blocks and pages back", async () => {
    serve({ "/blocks": { body: list(975_420) }, "/blocks/tip/height": { body: "975420" } });
    const el = open("/blocks");
    await until(() => expect(text(el.querySelector("tbody tr td") as Element)).toBe("975 420"));
    expect(el.querySelectorAll("tbody tr")).toHaveLength(10);
    expect(el.querySelector('a[rel="next"]')?.getAttribute("href")).toBe("/blocks/975410");
    expect(el.querySelector('a[rel="prev"]')).toBeNull();
  });

  it("links newer pages and stops at the genesis block", async () => {
    serve({ "/blocks/5": { body: list(5) }, "/blocks/tip/height": { body: "975420" } });
    const el = open("/blocks/5");
    await until(() => expect(el.querySelectorAll("tbody tr")).toHaveLength(6));
    expect(el.querySelector('a[rel="prev"]')?.getAttribute("href")).toBe("/blocks/15");
    expect(el.querySelector('a[rel="next"]')).toBeNull();
  });

  it("refuses a bad height", () => {
    serve({});
    const el = open("/blocks/abc");
    expect(text(el)).toContain("“abc” is not a block height");
  });
});
