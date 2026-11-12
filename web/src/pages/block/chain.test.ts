import { describe, expect, it } from "vitest";
import type { Transaction } from "../../lib/esplora";
import {
  blockReward,
  coinbaseTag,
  formatDifficulty,
  fullness,
  HALVING_INTERVAL,
  hex32,
  parseBlockId,
  subsidy,
} from "./chain";

const MAIN = HALVING_INTERVAL.mainnet;
const tagText = (t: ReturnType<typeof coinbaseTag>) =>
  t?.parts.map((p) => ("text" in p ? p.text : "·".repeat(p.hidden))).join("");

describe("subsidy", () => {
  it("halves every interval", () => {
    expect(subsidy(0, MAIN)).toBe(5_000_000_000);
    expect(subsidy(209_999, MAIN)).toBe(5_000_000_000);
    expect(subsidy(210_000, MAIN)).toBe(2_500_000_000);
    expect(subsidy(840_000, MAIN)).toBe(312_500_000);
    expect(subsidy(975_411, MAIN)).toBe(312_500_000);
    expect(subsidy(1_050_000, MAIN)).toBe(156_250_000);
  });

  it("rounds down in the late eras and stops", () => {
    expect(subsidy(210_000 * 32, MAIN)).toBe(1); // 5e9 / 2^32 = 1.16
    expect(subsidy(210_000 * 33 - 1, MAIN)).toBe(1);
    expect(subsidy(210_000 * 33, MAIN)).toBe(0);
    expect(subsidy(210_000 * 64, MAIN)).toBe(0);
    expect(subsidy(210_000 * 100, MAIN)).toBe(0);
  });

  it("uses each network's interval", () => {
    expect(HALVING_INTERVAL.signet).toBe(210_000);
    expect(HALVING_INTERVAL.testnet4).toBe(210_000);
    expect(subsidy(149, HALVING_INTERVAL.regtest)).toBe(5_000_000_000);
    expect(subsidy(150, HALVING_INTERVAL.regtest)).toBe(2_500_000_000);
    expect(subsidy(301, HALVING_INTERVAL.regtest)).toBe(1_250_000_000);
    expect(subsidy(150 * 64, HALVING_INTERVAL.regtest)).toBe(0);
  });

  it("is zero for nonsense", () => {
    expect(subsidy(-1, MAIN)).toBe(0);
    expect(subsidy(1.5, MAIN)).toBe(0);
    expect(subsidy(Number.NaN, MAIN)).toBe(0);
  });
});

const coinbase = (values: number[], scriptsig = "03"): Transaction => ({
  txid: "00".repeat(32),
  version: 1,
  locktime: 0,
  vin: [{ txid: "00".repeat(32), vout: 0xffffffff, is_coinbase: true, scriptsig }],
  vout: values.map((value) => ({
    value,
    scriptpubkey: "",
    scriptpubkey_asm: "",
    scriptpubkey_type: "p2pkh",
  })),
  status: { confirmed: true },
});

describe("blockReward", () => {
  it("sums every output and takes the subsidy off for fees", () => {
    expect(blockReward(coinbase([312_000_000, 2_059_121, 0]), 975_516, MAIN)).toEqual({
      reward: 314_059_121,
      subsidy: 312_500_000,
      fees: 1_559_121,
    });
  });

  it("gives zero fees for the genesis block", () => {
    expect(blockReward(coinbase([5_000_000_000]), 0, MAIN)?.fees).toBe(0);
  });

  it("does not invent fees when the miner claimed less than the subsidy", () => {
    expect(blockReward(coinbase([100]), 975_516, MAIN)?.fees).toBeNull();
  });

  it("refuses transactions that are not a coinbase or carry broken amounts", () => {
    const tx = coinbase([1]);
    const notCoinbase = { txid: tx.txid, vout: 0, is_coinbase: false };
    expect(blockReward({ ...tx, vin: [notCoinbase] }, 1, MAIN)).toBeNull();
    expect(blockReward(undefined, 1, MAIN)).toBeNull();
    expect(blockReward(coinbase([0.5]), 1, MAIN)).toBeNull();
  });
});

describe("fullness", () => {
  it("is the share of the weight limit", () => {
    expect(fullness(3_994_821)).toBeCloseTo(0.9987, 4);
    expect(fullness(1140)).toBeCloseTo(0.000285, 6);
    expect(fullness(4_000_000)).toBe(1);
    expect(fullness(9_000_000)).toBe(1);
    expect(fullness(-1)).toBe(0);
    expect(fullness(Number.NaN)).toBe(0);
  });
});

describe("parseBlockId", () => {
  const hash = "000000000019D6689C085AE165831E934FF763AE46A2A6C172B3F1B60A8CE26F";
  it("reads hashes and heights", () => {
    expect(parseBlockId(hash)).toEqual({ kind: "hash", hash: hash.toLowerCase() });
    expect(parseBlockId("0")).toEqual({ kind: "height", height: 0 });
    expect(parseBlockId("4294967295")).toEqual({ kind: "height", height: 4_294_967_295 });
  });

  it("rejects everything else", () => {
    for (const bad of [
      "",
      "4294967296",
      "-1",
      "1.5",
      "1e3",
      " 12",
      `0x${hash.slice(2)}`,
      hash.slice(1),
      "abc",
    ])
      expect(parseBlockId(bad)).toBeNull();
  });
});

describe("coinbaseTag", () => {
  it("reads the genesis message and shows the bytes before it", () => {
    const t = coinbaseTag(
      "04ffff001d0104455468652054696d65732030332f4a616e2f32303039204368616e63656c6c6f72206f6e206272696e6b206f66207365636f6e64206261696c6f757420666f722062616e6b73",
      0,
    );
    expect(t?.readable).toBe(true);
    expect(tagText(t)).toBe(
      "·····The Times 03/Jan/2009 Chancellor on brink of second bailout for banks",
    );
  });

  it("drops the BIP34 height and decodes UTF-8", () => {
    const t = coinbaseTag(
      "039ce20e041331f56a7c204d415241204d61646520696e2055534120f09f87baf09f87b8207c76303513dede437a0000000000ffffffff",
      975_516,
    );
    expect(tagText(t)).toBe("·1·j·MARA Made in USA 🇺🇸 |v05···z·········");
    expect(t?.parts[0]).toEqual({ hidden: 1 });
  });

  it("reads a signet tag", () => {
    expect(tagText(coinbaseTag("03e20a050a2f7369676e65743a332f", 330_466))).toBe("/signet:3/");
  });

  it("says when there is no text and keeps the hex", () => {
    const t = coinbaseTag("03f3730200b22731", 160_755);
    expect(t?.readable).toBe(false);
    expect(t?.hex).toBe("00b22731");
    expect(tagText(t)).toBe("·1");
  });

  it("drops a small height pushed as an opcode", () => {
    expect(coinbaseTag("5500", 5)?.hex).toBe("00");
  });

  it("makes hostile characters visible and never yields markup", () => {
    // "<b>" + RLO + NUL + ZWSP + "x" + invalid byte
    const t = coinbaseTag(`0f3c623ee280ae00e2808b78ff${"41".repeat(3)}`, 999_999);
    expect(tagText(t)).toBe("<b>···x·AAA");
    for (const p of t?.parts ?? []) if ("text" in p) expect(p.text).not.toMatch(/[‮​\0]/);
  });

  it("bounds the length and survives broken input", () => {
    const long = coinbaseTag(`4c${"ff"}${"41".repeat(400)}`, 999_999);
    expect(tagText(long)?.length).toBeLessThanOrEqual(100);
    expect(coinbaseTag("zz", 1)).toBeNull();
    expect(coinbaseTag("abc", 1)).toBeNull();
    expect(coinbaseTag(undefined, 1)).toBeNull();
    expect(coinbaseTag("4effffffff41", 999_999)?.parts).toEqual([{ text: "A" }]);
  });
});

describe("header fields", () => {
  it("formats difficulty and hex fields", () => {
    expect(formatDifficulty(148_364_561_003_867.4).short).toBe("148.36 T");
    expect(formatDifficulty(1).short).toBe("1");
    expect(formatDifficulty(0.00123).short).toBe("0.00123");
    expect(hex32(386_000_300)).toBe("0x1701e5ac");
    expect(hex32(-1)).toBe("0xffffffff");
  });
});
