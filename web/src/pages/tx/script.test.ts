import { describe, expect, it } from "vitest";
import {
  classify,
  disassemble,
  hexToBytes,
  isText,
  lastPush,
  opReturnData,
  printable,
} from "./script";

const h20 = "0b2867e31c2c5e8ebb55631eececd31da8e25108";
const h32 = "8639eaab74c917a6385fd2932e2085dca8401c28b6bd506146ad4e552e229013";
const pk33 = `02${"11".repeat(32)}`;
const pk65 = `04${"22".repeat(64)}`;
const enc = (s: string) =>
  [...new TextEncoder().encode(s)].map((b) => b.toString(16).padStart(2, "0")).join("");
const push = (hex: string) => `${(hex.length / 2).toString(16).padStart(2, "0")}${hex}`;

describe("classify", () => {
  it.each([
    [`76a914${h20}88ac`, "p2pkh", "P2PKH"],
    [`a914${h20}87`, "p2sh", "P2SH"],
    [`0014${h20}`, "p2wpkh", "P2WPKH"],
    [`0020${h32}`, "p2wsh", "P2WSH"],
    [`5120${h32}`, "p2tr", "P2TR"],
    ["51024e73", "anchor", "Pay to anchor"],
    [`5210${"ab".repeat(16)}`, "witness-unknown", "Witness v2"],
    [`21${pk33}ac`, "p2pk", "P2PK"],
    [`41${pk65}ac`, "p2pk", "P2PK"],
    [`51${push(pk33)}${push(pk33)}52ae`, "multisig", "Bare multisig, 1 of 2"],
    [`52${push(pk33)}${push(pk65)}${push(pk33)}53ae`, "multisig", "Bare multisig, 2 of 3"],
    ["6a", "op_return", "OP_RETURN"],
    [`6a24aa21a9ed${h32}`, "op_return", "OP_RETURN"],
    ["755187", "nonstandard", "Non-standard"],
    ["", "nonstandard", "Empty script"],
  ])("%s is %s", (hex, kind, label) => {
    expect(classify(hex)).toEqual({ kind, label });
  });

  it("does not mistake near misses for standard types", () => {
    expect(classify(`76a914${h20}88`).kind).toBe("nonstandard"); // P2PKH missing OP_CHECKSIG
    expect(classify(`0015${h20}00`).kind).toBe("witness-unknown"); // v0 with a 21-byte program
    expect(classify(`53${push(pk33)}${push(pk33)}52ae`).kind).toBe("nonstandard"); // 3 of 2
    expect(classify(`51${push(pk33)}52ae`).kind).toBe("nonstandard"); // says 2 keys, has 1
    expect(classify("zz").kind).toBe("nonstandard");
  });
});

describe("disassemble", () => {
  it("names opcodes and shows pushes as hex", () => {
    expect(disassemble(`76a914${h20}88ac`).text).toBe(
      `OP_DUP OP_HASH160 ${h20} OP_EQUALVERIFY OP_CHECKSIG`,
    );
    expect(disassemble(`5120${h32}`).text).toBe(`OP_1 ${h32}`);
    expect(disassemble("0051604f50b1b2b3b9ba61ff").text).toBe(
      "OP_0 OP_1 OP_16 OP_1NEGATE OP_RESERVED OP_CHECKLOCKTIMEVERIFY OP_CHECKSEQUENCEVERIFY OP_NOP4 OP_NOP10 OP_CHECKSIGADD OP_NOP OP_INVALIDOPCODE",
    );
    expect(disassemble("bb").text).toBe("OP_UNKNOWN_0xbb");
  });

  it("reads the PUSHDATA forms", () => {
    expect(disassemble(`4c02abcd`).text).toBe("OP_PUSHDATA1 abcd");
    expect(disassemble(`4d0200abcd`).text).toBe("OP_PUSHDATA2 abcd");
    expect(disassemble(`4e02000000abcd`).text).toBe("OP_PUSHDATA4 abcd");
    expect(disassemble("4c00").text).toBe("OP_PUSHDATA1");
  });

  it("survives pushes past the end and a cut length byte", () => {
    expect(disassemble("05abcd").text).toBe("abcd [push past end of script]");
    expect(disassemble("4c").text).toBe("OP_PUSHDATA1 [push past end of script]");
    expect(disassemble("4dff").text).toContain("past end");
    expect(disassemble("4effffffff00").text).toContain("past end");
  });

  it("stops at the character bound", () => {
    const big = `4d0010${"ab".repeat(0x1000)}`;
    const r = disassemble(big, 100);
    expect(r.truncated).toBe(true);
    expect(r.text.length).toBe(100);
    expect(disassemble(big).truncated).toBe(false);
  });
});

describe("OP_RETURN data", () => {
  it("joins the pushes after OP_RETURN", () => {
    expect([...opReturnData(`6a${push(enc("hello"))}${push(enc(" world"))}`)]).toEqual([
      ...new TextEncoder().encode("hello world"),
    ]);
    expect(opReturnData(`76a914${h20}88ac`).length).toBe(0);
    // Runes-style: a non-push opcode, then data.
    expect(opReturnData(`6a5d${push("0102")}`)).toEqual(new Uint8Array([1, 2]));
  });

  it("shows hostile text as inert, visible text", () => {
    const hostile = '<img src=x onerror="alert(1)">‮evil​\t\0';
    const p = printable(new TextEncoder().encode(hostile));
    expect(p.text).toBe('<img src=x onerror="alert(1)">\\u{202e}evil\\u{200b}\\u{9}\\u{0}');
    expect(p.escaped).toBe(4);
  });

  it("escapes bytes that are not UTF-8", () => {
    const bytes = new Uint8Array([
      0x68, 0x69, 0xff, 0xc3, 0x28, 0xed, 0xa0, 0x80, 0xf0, 0x9f, 0x98, 0x80,
    ]);
    expect(printable(bytes).text).toBe("hi\\xff\\xc3(\\xed\\xa0\\x80😀");
    // Overlong encoding of "/" is refused.
    expect(printable(new Uint8Array([0xc0, 0xaf])).text).toBe("\\xc0\\xaf");
  });

  it("stops at the character bound", () => {
    const p = printable(new TextEncoder().encode("a".repeat(1000)), 10);
    expect(p.text).toBe("aaaaaaaaaa");
    expect(p.chars).toBe(10);
  });

  it("tells text from binary", () => {
    expect(isText(new TextEncoder().encode("mempool.based regtest"))).toBe(true);
    expect(isText(new TextEncoder().encode("ünïcode 😀"))).toBe(true);
    expect(isText(hexToBytes(`aa21a9ed${h32}`))).toBe(false);
    expect(isText(new Uint8Array([0x14, 0x00, 0x01, 0x02]))).toBe(false);
    expect(isText(new Uint8Array())).toBe(false);
  });
});

describe("lastPush", () => {
  it("finds the redeem script at the end of a scriptSig", () => {
    expect(lastPush(`00${push("3044")}${push(`0014${h20}`)}`)).toBe(`0014${h20}`);
    expect(lastPush("51")).toBeNull();
    expect(lastPush("05ab")).toBeNull();
    expect(lastPush("")).toBeNull();
  });
});

describe("hexToBytes", () => {
  it("stops at the first bad pair", () => {
    expect([...hexToBytes("01zz02")]).toEqual([1]);
    expect([...hexToBytes("abc")]).toEqual([0xab]);
  });
});
