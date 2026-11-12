import { describe, expect, it, vi } from "vitest";
import { ApiError } from "../lib/api";
import type { Network } from "../lib/network";
import { classify } from "./query";
import { createSearch, describe as read, type Get, probeOrder, resolve as lead } from "./search";

const TXID = "65428488a6202f1d681d053b3ff5d8ea9f5b0e5e1e0dcf7bf2e4c1e5a3f9c2e1";
const BLOCK = "00000000000000000000e13ad5e424f6ba7208b0c1a9a687e9ccc4d62b3b094c";
const REGTEST_BLOCK = "2ac64444e3172ee94aeeff3b09014b1d69b4daeccddac025d493332d54f6f145";

const notFound = () => Promise.reject(new ApiError(404, "Not found."));

/** A fake API: the paths that exist answer, the rest are 404. Records every path asked. */
function api(exist: readonly string[]) {
  const asked: string[] = [];
  const probe = (path: string) => {
    asked.push(path);
    return exist.includes(path) ? Promise.resolve({}) : notFound();
  };
  return { asked, probe };
}

describe("read (describe)", () => {
  it.each([
    ["840000", "mainnet", "Block 840\u{202F}000", true],
    [TXID, "mainnet", "Transaction or block", true],
    [`${TXID}:1`, "mainnet", "Transaction", true],
    [`https://mempool.space/block/${BLOCK}`, "mainnet", "Block hash", true],
    ["bc1pfeessrawgf", "mainnet", "P2A address", true],
    ["tb1pfees9rn5nz", "mainnet", "Signet or Testnet4 address", true],
    ["2ND8PB9RrfCaAcjfjP1Y6nAgFd9zWHYX4DN", "signet", "P2SH address", true],
    ["2ND8PB9RrfCaAcjfjP1Y6nAgFd9zWHYX4DN", "mainnet", "Signet, Testnet4 or Regtest address", true],
    [TXID.slice(0, 20), "mainnet", "20 of 64 characters", false],
    ["hello", "mainnet", "Not recognised", false],
    ["x".repeat(600), "mainnet", "Too long", false],
  ] as const)("%s on %s reads %j", (q, network, label, ready) => {
    expect(read(q, network)).toStrictEqual({ label, ready });
  });
});

describe("probeOrder", () => {
  it("asks for a block first when the hash meets proof of work, else a transaction", () => {
    expect(probeOrder(BLOCK, null)).toStrictEqual(["block", "tx"]);
    expect(probeOrder(TXID, null)).toStrictEqual(["tx", "block"]);
    expect(probeOrder(REGTEST_BLOCK, null)).toStrictEqual(["tx", "block"]);
  });
  it("follows what a link or an outpoint says", () => {
    expect(probeOrder(TXID, "block")).toStrictEqual(["block", "tx"]);
    expect(probeOrder(BLOCK, "tx")).toStrictEqual(["tx", "block"]);
  });
});

describe("resolve (lead)", () => {
  it("goes to a height's block page without asking: that page says when it is not mined yet", async () => {
    const { asked, probe } = api([]);
    await expect(lead(classify("#99,999,999"), probe)).resolves.toStrictEqual({
      kind: "go",
      path: "/block/99999999",
    });
    expect(asked).toStrictEqual([]);
  });

  it("goes to an address page without asking, whatever its network", async () => {
    const { asked, probe } = api([]);
    await expect(lead(classify("tb1pfees9rn5nz"), probe)).resolves.toStrictEqual({
      kind: "go",
      path: "/address/tb1pfees9rn5nz",
    });
    expect(asked).toStrictEqual([]);
  });

  it("finds a block hash in one request", async () => {
    const { asked, probe } = api([`/block/${BLOCK}`]);
    await expect(lead(classify(BLOCK), probe)).resolves.toStrictEqual({
      kind: "go",
      path: `/block/${BLOCK}`,
    });
    expect(asked).toStrictEqual([`/block/${BLOCK}`]);
  });

  it("finds a transaction in one request", async () => {
    const { asked, probe } = api([`/tx/${TXID}/status`]);
    await expect(lead(classify(TXID.toUpperCase()), probe)).resolves.toStrictEqual({
      kind: "go",
      path: `/tx/${TXID}`,
    });
    expect(asked).toStrictEqual([`/tx/${TXID}/status`]);
  });

  it("finds a block whose hash looks like a txid in two, never showing the wrong guess", async () => {
    const { asked, probe } = api([`/block/${REGTEST_BLOCK}`]);
    await expect(lead(classify(REGTEST_BLOCK), probe)).resolves.toStrictEqual({
      kind: "go",
      path: `/block/${REGTEST_BLOCK}`,
    });
    expect(asked).toStrictEqual([`/tx/${REGTEST_BLOCK}/status`, `/block/${REGTEST_BLOCK}`]);
  });

  it("says nothing matches when neither exists, after two requests", async () => {
    const { asked, probe } = api([]);
    const query = classify(TXID);
    await expect(lead(query, probe)).resolves.toStrictEqual({ kind: "missing", query });
    expect(asked).toHaveLength(2);
  });

  it("says nothing matches for an unrecognised query without asking", async () => {
    const { asked, probe } = api([]);
    await expect(lead(classify("hello"), probe)).resolves.toStrictEqual({
      kind: "missing",
      query: null,
    });
    expect(asked).toStrictEqual([]);
  });

  it("stops at the first failure that is not a 404: the node being down is not 'nothing'", async () => {
    const asked: string[] = [];
    const down = new ApiError(502, "Upstream unavailable.");
    const probe = (path: string) => {
      asked.push(path);
      return Promise.reject(down);
    };
    await expect(lead(classify(TXID), probe)).rejects.toBe(down);
    expect(asked).toHaveLength(1);
  });
});

describe("createSearch", () => {
  /** A fake fetch whose answers the test releases by hand. */
  function manual() {
    const pending: { path: string; signal: AbortSignal; answer: (found: boolean) => void }[] = [];
    const get: Get = (_network: Network, path, signal) =>
      new Promise((resolve, reject) => {
        signal.addEventListener("abort", () => reject(new DOMException("Aborted", "AbortError")));
        pending.push({
          path,
          signal,
          answer: (found) => (found ? resolve({}) : reject(new ApiError(404, "Not found."))),
        });
      });
    return { pending, get };
  }

  it("lets a second search overtake a slow first one", async () => {
    const { pending, get } = manual();
    const search = createSearch(get);
    const first = search.run(TXID, "mainnet");
    const second = search.run(BLOCK, "mainnet");
    await vi.waitFor(() => expect(pending).toHaveLength(2));
    expect(pending[0]?.signal.aborted).toBe(true);
    // The first answers late, and successfully: it must still lose.
    pending[0]?.answer(true);
    pending[1]?.answer(true);
    await expect(first).resolves.toBeUndefined();
    await expect(second).resolves.toStrictEqual({ kind: "go", path: `/block/${BLOCK}` });
  });

  it("answers nothing once cancelled", async () => {
    const { pending, get } = manual();
    const search = createSearch(get);
    const run = search.run(TXID, "mainnet");
    await vi.waitFor(() => expect(pending).toHaveLength(1));
    search.cancel();
    await expect(run).resolves.toBeUndefined();
  });

  it("reports the node being unavailable as a failure", async () => {
    const down = new ApiError(503, "Node unavailable.");
    const search = createSearch(() => Promise.reject(down));
    await expect(search.run(TXID, "signet")).resolves.toStrictEqual({
      kind: "failed",
      error: down,
    });
  });

  it("asks the network it was given", async () => {
    const seen: Network[] = [];
    const search = createSearch((network) => {
      seen.push(network);
      return Promise.resolve({});
    });
    await search.run(TXID, "testnet4");
    expect(seen).toStrictEqual(["testnet4"]);
  });
});
