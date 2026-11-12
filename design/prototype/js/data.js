// Invented but realistic sample data for late 2026: most transactions pay under 1 sat/vB, relay floor 0.1.
import { FEE_BOUNDS, feeBand, addrPrefix } from "./common.js";

let seed = 0x5eed1e55;
export function reseed(s) { seed = s >>> 0; }
export function rand() { // mulberry32: deterministic so screenshots are repeatable
  seed = (seed + 0x6d2b79f5) >>> 0;
  let t = seed;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
}
const pick = (arr) => arr[Math.floor(rand() * arr.length)];
const logUniform = (lo, hi) => Math.exp(Math.log(lo) + rand() * (Math.log(hi) - Math.log(lo)));

export const hex = (n = 64) => Array.from({ length: n }, () => "0123456789abcdef"[Math.floor(rand() * 16)]).join("");
const BECH32 = "qpzry9x8gf2tvdw0s3jn54khce6mua7l";
export function address() {
  const taproot = rand() < 0.45;
  const body = Array.from({ length: taproot ? 58 : 38 }, () => BECH32[Math.floor(rand() * 32)]).join("");
  return `${addrPrefix}${taproot ? "p" : "q"}${body}`;
}

export function feeRate() {
  const r = rand();
  if (r < 0.34) return logUniform(0.1, 0.3);
  if (r < 0.62) return logUniform(0.3, 1);
  if (r < 0.84) return logUniform(1, 2.5);
  if (r < 0.95) return logUniform(2.5, 8);
  return logUniform(8, 60);
}
export function vsize() {
  const r = rand();
  if (r < 0.8) return Math.round(110 + rand() * 120);
  if (r < 0.98) return Math.round(230 + rand() * 700);
  return Math.round(1000 + rand() * 5000);
}

export function tx(rate = feeRate()) {
  const v = vsize();
  return { txid: hex(), rate, vsize: v, fee: Math.max(1, Math.round(rate * v)), value: Math.round(logUniform(2e3, 4e8)) };
}

const BLOCK_VB = 998_000;

/** Transactions for one block, best fee rate first, as a block template would order them. */
export function blockTxs(fill = 1, bias = 1) {
  const out = [];
  let used = 0;
  while (used < BLOCK_VB * fill) {
    const t = tx(Math.max(0.1, feeRate() * bias));
    out.push(t);
    used += t.vsize;
  }
  return out.sort((a, b) => b.rate - a.rate);
}

/** Summary used by a block tile: vsize per fee band, median and range. */
export function summarize(txs) {
  const bands = new Array(FEE_BOUNDS.length + 1).fill(0);
  let vb = 0, fees = 0;
  for (const t of txs) { bands[feeBand(t.rate) - 1] += t.vsize; vb += t.vsize; fees += t.fee; }
  const rates = txs.map((t) => t.rate).sort((a, b) => a - b);
  return { txCount: txs.length, vsize: vb, fees, bands, median: rates[Math.floor(rates.length / 2)] ?? 0, min: rates[0] ?? 0, max: rates.at(-1) ?? 0 };
}

export const POOLS = ["Halite Pool", "Northbank", "Kestrel Hash", "Ozark Mining", "Basin", "Tern", "Unknown"];
export const pool = () => pick(POOLS);

export const TIP = 975_411;
