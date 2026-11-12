import { describe, expect, it } from "vitest";
import { decodeTxs, TxsFormatError } from "./decode";

/** The server's format: version, count, then vsize u32, fee u64, value u64 per record. */
function encode(records: [number, number, number][], version = 1, count = records.length) {
  const view = new DataView(new ArrayBuffer(8 + records.length * 20));
  view.setUint32(0, version, true);
  view.setUint32(4, count, true);
  for (const [i, [vsize, fee, value]] of records.entries()) {
    view.setUint32(8 + i * 20, vsize, true);
    view.setBigUint64(12 + i * 20, BigInt(fee), true);
    view.setBigUint64(20 + i * 20, BigInt(value), true);
  }
  return view.buffer;
}

/** The kind of error a reply is refused with, or "read" when it is not. */
const refusal = (buffer: ArrayBuffer) => {
  try {
    decodeTxs(buffer);
    return "read";
  } catch (e) {
    return e instanceof TxsFormatError ? e.kind : "other";
  }
};

describe("decodeTxs", () => {
  it("reads every record in block order, the coinbase first", () => {
    const txs = decodeTxs(
      encode([
        [180, 0, 313_000_000],
        [141, 1410, 2_099_999_997_690_000], // the most sats there can be, still exact
      ]),
    );
    expect(txs).toStrictEqual([
      { id: 0, vsize: 180, fee: 0, value: 313_000_000, coinbase: true },
      { id: 1, vsize: 141, fee: 1410, value: 2_099_999_997_690_000, coinbase: false },
    ]);
  });

  it("reads a block holding only its coinbase", () => {
    expect(decodeTxs(encode([[100, 0, 5_000_000_000]]))).toHaveLength(1);
  });

  it("refuses another version, so the page can ask for a reload", () => {
    expect(refusal(encode([[100, 0, 1]], 2))).toBe("version");
  });

  it("refuses a truncated reply, trailing bytes and a short header", () => {
    expect(refusal(encode([[100, 0, 1]]).slice(0, 20))).toBe("malformed");
    expect(refusal(encode([[100, 0, 1]], 1, 2))).toBe("malformed");
    expect(refusal(encode([[100, 0, 1]], 1, 0))).toBe("malformed");
    expect(refusal(new ArrayBuffer(4))).toBe("malformed");
  });
});
