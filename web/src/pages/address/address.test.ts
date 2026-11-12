import { describe, expect, it } from "vitest";
import { ApiError } from "../../lib/api";
import type { Transaction, Utxo, Vin, Vout } from "../../lib/esplora";
import {
  confirmations,
  mergePages,
  netChange,
  pageIsFull,
  recognise,
  refusal,
  sortUtxos,
  txFeeRate,
} from "./address";

const TEST = ["signet", "testnet4", "regtest"];
const TB = ["signet", "testnet4"];

describe("recognise", () => {
  it.each([
    // Mainnet
    ["1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa", "p2pkh", ["mainnet"]],
    ["3J98t1WpEZ73CNmQviecrnyiWrnqRhWNLy", "p2sh", ["mainnet"]],
    ["bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4", "p2wpkh", ["mainnet"]],
    ["bc1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3qccfmv3", "p2wsh", ["mainnet"]],
    ["bc1p0xlxvlhemja6c4dqv22uapctqupfhlxm9h8z3k2e72q4k9hcz7vqzk5jj0", "p2tr", ["mainnet"]],
    ["bc1pfeessrawgf", "p2a", ["mainnet"]],
    ["bc1zw508d6qejxtdg4y5r3zarvaryvaxxpcs", "witness", ["mainnet"]],
    // Signet and testnet4 share every form; regtest shares the base58 ones
    ["mipcBbFg9gMiCh81Kj8tqqdgoZub1ZJRfn", "p2pkh", TEST],
    ["n3GNqMveyvaPvUbH469vDRadqpJMPc84JA", "p2pkh", TEST],
    ["2ND8PB9RrfCaAcjfjP1Y6nAgFd9zWHYX4DN", "p2sh", TEST],
    ["tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx", "p2wpkh", TB],
    ["tb1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3q0sl5k7", "p2wsh", TB],
    ["tb1pqqqqp399et2xygdj5xreqhjjvcmzhxw4aywxecjdzew6hylgvsesf3hn0c", "p2tr", TB],
    ["tb1pfees9rn5nz", "p2a", TB],
    // Regtest
    ["bcrt1qw5puevk8wgy5t9d2qjg6frpfmesuqnnpx95u7s", "p2wpkh", ["regtest"]],
    ["bcrt1qgpsh04whvyp5fvrul6rr8384qt0mf4a7h507m5ed79d72euknpgqpmza40", "p2wsh", ["regtest"]],
    ["bcrt1pz9ml8p3h96z9hlvz0esm3uwdq9pxssmykz209jrg57znrdvx5y0shy3atk", "p2tr", ["regtest"]],
    ["bcrt1pfeesnyr2tx", "p2a", ["regtest"]],
  ])("%s is %s", (address, type, networks) => {
    expect(recognise(address)).toEqual({ address, type, networks });
  });

  it("reads upper-case bech32 as lower case, and keeps base58 as given", () => {
    expect(recognise("BC1QW508D6QEJXTDG4Y5R3ZARVARY0C5XW7KV8F3T4")?.address).toBe(
      "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4",
    );
    expect(recognise("1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa")?.address).toBe(
      "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa",
    );
  });

  it.each([
    "",
    "hello",
    "bc1",
    "bc1q",
    // Mixed case
    "bc1QW508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4",
    // b, i, o and 1 are not in the bech32 alphabet
    "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3tb",
    // Version 0 needs a 20- or 32-byte program
    "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7k",
    // Unknown prefix
    "ltc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4",
    // 0, O, I and l are not base58; 4 starts no address
    "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfN0",
    "4J98t1WpEZ73CNmQviecrnyiWrnqRhWNLy",
    // Too long for base58, too short
    "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNaaaaa",
    "1A1zP1eP5Qa",
    "65428488a6202f1d681d053b3ff5d8ea9f5b0e5e1e0dcf7bf2e4c1e5a3f9c2e1",
    '<img src=x onerror="alert(1)">',
    `bc1q${"q".repeat(90)}`,
  ])("%j is not an address", (input) => {
    expect(recognise(input)).toBeNull();
  });
});

const ME = "bc1qme";
const OTHER = "bc1qother";

const out = (address: string, value: number): Vout => ({
  scriptpubkey: "",
  scriptpubkey_asm: "",
  scriptpubkey_type: "v0_p2wpkh",
  scriptpubkey_address: address,
  value,
});
const spend = (address: string, value: number): Vin => ({
  txid: "a".repeat(64),
  vout: 0,
  is_coinbase: false,
  prevout: out(address, value),
});
const tx = (vin: Vin[], vout: Vout[], extra: Partial<Transaction> = {}): Transaction => ({
  txid: "b".repeat(64),
  version: 2,
  locktime: 0,
  vin,
  vout,
  status: { confirmed: true, block_height: 1 },
  fee: 1000,
  weight: 800,
  ...extra,
});

describe("netChange", () => {
  it("counts a receive", () => {
    expect(netChange(tx([spend(OTHER, 50_000)], [out(ME, 30_000), out(OTHER, 19_000)]), ME)).toBe(
      30_000,
    );
  });

  it("counts a spend", () => {
    expect(netChange(tx([spend(ME, 50_000)], [out(OTHER, 49_000)]), ME)).toBe(-50_000);
  });

  it("nets a spend with change back to the address", () => {
    const t = tx([spend(ME, 50_000)], [out(OTHER, 20_000), out(ME, 29_000)]);
    expect(netChange(t, ME)).toBe(-21_000);
  });

  it("leaves only the fee on a self-transfer", () => {
    expect(netChange(tx([spend(ME, 50_000)], [out(ME, 49_000)]), ME)).toBe(-1000);
  });

  it("adds up many inputs from the address", () => {
    const inputs = Array.from({ length: 40 }, () => spend(ME, 2_500));
    expect(netChange(tx([...inputs, spend(OTHER, 7)], [out(OTHER, 100_000)]), ME)).toBe(-100_000);
  });

  it("counts several outputs to the address and is exact for large amounts", () => {
    const t = tx(
      [spend(OTHER, 2_100_000_000_000_000)],
      [out(ME, 2_099_999_999_999_999), out(ME, 1)],
    );
    expect(netChange(t, ME)).toBe(2_100_000_000_000_000);
  });

  it("takes coinbase and outputs without an address in its stride", () => {
    const coinbase: Vin = {
      txid: "0".repeat(64),
      vout: 0xffffffff,
      is_coinbase: true,
      prevout: null,
    };
    const opReturn: Vout = { ...out(ME, 0), scriptpubkey_type: "op_return" };
    delete opReturn.scriptpubkey_address;
    expect(netChange(tx([coinbase], [out(ME, 312_500_000), opReturn]), ME)).toBe(312_500_000);
  });

  it("is unknown when the node has pruned the inputs", () => {
    expect(netChange(tx([], [out(ME, 1)], { pruned: true }), ME)).toBeNull();
  });
});

describe("txFeeRate", () => {
  it("is fee over virtual size, and absent for a coinbase", () => {
    expect(txFeeRate(tx([spend(OTHER, 1)], [], { fee: 1000, weight: 801 }))).toBeCloseTo(
      1000 / 201,
    );
    const coinbase: Vin = { txid: "0".repeat(64), vout: 0, is_coinbase: true };
    expect(txFeeRate(tx([coinbase], []))).toBeNull();
    const noFee = tx([spend(OTHER, 1)], []);
    delete noFee.fee;
    expect(txFeeRate(noFee)).toBeNull();
  });
});

describe("refusal", () => {
  it.each([
    [
      new ApiError(503, "Address history too large for this route; use the paged /txs routes."),
      "too-large",
    ],
    [new ApiError(503, "Address index is disabled on this server."), "no-index"],
    [new ApiError(400, "Address is for another network."), "wrong-network"],
    [new ApiError(400, "Invalid address."), "invalid"],
    [new ApiError(503, "Node unavailable."), "failed"],
    [new ApiError(502, "Upstream unavailable."), "failed"],
    [new ApiError(0, "The server cannot be reached."), "failed"],
    [new Error("boom"), "failed"],
  ])("%s is %s", (error, kind) => {
    expect(refusal(error)).toBe(kind);
  });
});

const utxo = (value: number, height?: number): Utxo => ({
  txid: "c".repeat(64),
  vout: 0,
  value,
  status: height === undefined ? { confirmed: false } : { confirmed: true, block_height: height },
});

describe("unspent outputs", () => {
  it("sort unconfirmed first, then newest, then largest", () => {
    const sorted = sortUtxos([utxo(1, 5), utxo(2, 9), utxo(3), utxo(9, 5)]);
    expect(sorted.map((u) => u.value)).toEqual([3, 2, 9, 1]);
  });

  it("count confirmations from the tip", () => {
    expect(confirmations(utxo(1, 106), 110)).toBe(5);
    expect(confirmations(utxo(1, 110), 110)).toBe(1);
    expect(confirmations(utxo(1), 110)).toBe(0);
    // A tip read before the output's block counts as none rather than negative
    expect(confirmations(utxo(1, 111), 110)).toBe(0);
  });
});

describe("mergePages", () => {
  const t = (id: string) => tx([], [], { txid: id });
  it("puts the fresh first page first and drops what it repeats", () => {
    const merged = mergePages([t("new"), t("a"), t("b")], [t("b"), t("c")]);
    expect(merged.map((x) => x.txid)).toEqual(["new", "a", "b", "c"]);
  });
});

describe("pageIsFull", () => {
  const t = (confirmed: boolean) => tx([], [], { status: { confirmed } });
  it("counts only confirmed transactions against the page size", () => {
    expect(pageIsFull([t(false), t(false), t(true)], 2)).toBe(false);
    expect(pageIsFull([t(false), t(true), t(true)], 2)).toBe(true);
    expect(pageIsFull([], 25)).toBe(false);
  });
});
