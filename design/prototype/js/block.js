import { h, $, initChrome, params, feeBand, fmtRate, feeChip, group, btc, bytes, hash, hashFull, utc, ago, cssVar, reducedMotion, page } from "./common.js";
import { reseed, hex, blockTxs, summarize, pool, TIP } from "./data.js";
import { feeKey, showTip, hideTip } from "./parts.js";

initChrome();
const height = Number.parseInt(params.get("h") ?? "", 10) || TIP;
reseed(height);
document.title = document.title.replace(/\d[\d\s]*/, `${group(height)} `);

const txs = blockTxs(0.985, 0.9);
const sum = summarize(txs);
const time = Date.now() - (TIP - height) * 600_000 - 12 * 60_000;
const subsidy = 312_500_000;
const coinbase = { txid: hex(), rate: 0, vsize: 210, fee: 0, value: subsidy + sum.fees, coinbase: true };
const all = [coinbase, ...txs];

/* ---------- Header and facts ---------- */

$("#height").textContent = group(height);
$("#block-hash").append(hashFull("00000000000000000000" + hex(44)));
$("#prev-h").textContent = group(height - 1);
$("#prev").href = page(`block.html?h=${height - 1}`);
if (height >= TIP) $("#next").hidden = true;
else { $("#next-h").textContent = group(height + 1); $("#next").href = page(`block.html?h=${height + 1}`); }

const weight = Math.round(sum.vsize * 3.98);
const facts = [
  ["Mined", [h("time", { datetime: new Date(time).toISOString() }, utc(time)), h("span", { class: "fact__sub" }, ago(Date.now() - time))]],
  ["Miner", [pool()]],
  ["Transactions", [h("span", { class: "num" }, group(all.length))]],
  ["Size", [h("span", { class: "num" }, bytes(sum.vsize * 1.38, "B")), h("span", { class: "fact__sub num" }, `${group(weight)} WU, ${Math.round((weight / 4e6) * 100)}% full`)]],
  ["Median fee rate", [feeChip(sum.median)]],
  ["Fee range", [h("span", { class: "num" }, `${fmtRate(sum.min)} – ${fmtRate(sum.max)}`), h("span", { class: "unit" }, " sat/vB")]],
  ["Total fees", [btc(sum.fees)]],
  ["Reward", [btc(subsidy + sum.fees), h("span", { class: "fact__sub" }, "Subsidy 3.125 BTC plus fees")]],
];
$("#facts").append(...facts.map(([k, v]) => h("div", { class: "fact" }, h("dt", {}, k), h("dd", {}, ...v))));

/* ---------- Packed grid ---------- */

// Each transaction is a square whose area follows its vsize; highest fee rate fills from the top left.
const COLS = 96;
const cellsTotal = COLS * COLS * 0.86;
const unit = all.reduce((a, t) => a + t.vsize, 0) / cellsTotal;
const occ = new Int32Array(COLS * COLS * 2).fill(-1);
const placed = [];
let cursor = 0;
for (let i = 0; i < all.length; i++) {
  const side = Math.max(1, Math.min(14, Math.round(Math.sqrt(all[i].vsize / unit))));
  for (let p = cursor; p < occ.length; p++) {
    const x = p % COLS, y = Math.floor(p / COLS);
    if (x + side > COLS || !fits(x, y, side)) continue;
    for (let dy = 0; dy < side; dy++) for (let dx = 0; dx < side; dx++) occ[(y + dy) * COLS + x + dx] = i;
    placed.push({ i, x, y, side });
    while (occ[cursor] !== -1) cursor++;
    break;
  }
}
function fits(x, y, side) {
  for (let dy = 0; dy < side; dy++) for (let dx = 0; dx < side; dx++) {
    const k = (y + dy) * COLS + x + dx;
    if (k >= occ.length || occ[k] !== -1) return false;
  }
  return true;
}
const rows = Math.max(...placed.map((p) => p.y + p.side));

const canvas = $("#grid");
const ctx = canvas.getContext("2d");
canvas.setAttribute("aria-label", `${all.length} transactions drawn as squares sized by virtual size and colored by fee rate. The table below lists them.`);
$("#grid-caption").textContent = "Each square is one transaction, sized by virtual size and colored by fee rate. Highest fee rate starts at the top left.";
let highlight = -1;
let shown = placed.length;

function colors() {
  const c = Array.from({ length: 10 }, (_, i) => cssVar(`--fee-${i + 1}`));
  return { fee: c, coinbase: cssVar("--color-text-muted"), ring: cssVar("--color-text"), bg: cssVar("--color-surface-sunken") };
}

function draw() {
  const width = canvas.clientWidth; // the canvas content box, the same width txAt() maps pointers against
  const dpr = devicePixelRatio || 1;
  const cell = width / COLS;
  canvas.style.setProperty("height", `${rows * cell}px`);
  canvas.width = Math.round(width * dpr);
  canvas.height = Math.round(rows * cell * dpr);
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  const pal = colors();
  const gap = cell > 5 ? 1 : 0.5;
  for (let n = 0; n < shown; n++) {
    const p = placed[n], t = all[p.i];
    ctx.fillStyle = t.coinbase ? pal.coinbase : pal.fee[feeBand(t.rate) - 1];
    ctx.fillRect(p.x * cell + gap / 2, p.y * cell + gap / 2, p.side * cell - gap, p.side * cell - gap);
  }
  const p = placed.find((q) => q.i === highlight);
  if (p) {
    ctx.strokeStyle = pal.ring;
    ctx.lineWidth = 2;
    ctx.strokeRect(p.x * cell - 1, p.y * cell - 1, p.side * cell + 2, p.side * cell + 2);
  }
}

// Load moment: squares pour in by fee rate order. The real app does this in WebGL.
if (reducedMotion()) draw();
else {
  const dur = parseFloat(cssVar("--dur-block")) || 900;
  const t0 = performance.now();
  const step = (t) => {
    const k = Math.min(1, (t - t0) / dur);
    shown = Math.ceil(placed.length * (1 - (1 - k) ** 3));
    draw();
    if (k < 1) requestAnimationFrame(step);
  };
  shown = 0;
  requestAnimationFrame(step);
}
new ResizeObserver(() => draw()).observe(canvas.parentElement);
matchMedia("(prefers-color-scheme: dark)").addEventListener("change", draw);
new MutationObserver(draw).observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });

function txAt(ev) {
  const r = canvas.getBoundingClientRect();
  const cell = r.width / COLS;
  const x = Math.floor((ev.clientX - r.left) / cell), y = Math.floor((ev.clientY - r.top) / cell);
  return x < 0 || x >= COLS || y < 0 ? -1 : occ[y * COLS + x] ?? -1;
}
canvas.addEventListener("pointermove", (ev) => {
  const i = txAt(ev);
  if (i !== highlight) { highlight = i; draw(); }
  if (i < 0) return hideTip();
  const t = all[i];
  showTip(t.coinbase
    ? ["Coinbase", `${hashShort(t.txid)}`, `Pays ${(t.value / 1e8).toFixed(8)} BTC to the miner`]
    : [`${fmtRate(t.rate)} sat/vB`, hashShort(t.txid), `${group(t.vsize)} vB, fee ${group(t.fee)} sats`], ev.clientX, ev.clientY);
});
canvas.addEventListener("pointerleave", () => { highlight = -1; draw(); hideTip(); });
canvas.addEventListener("click", (ev) => { if (txAt(ev) >= 0) location.href = page("tx.html"); });
const hashShort = (s) => `${s.slice(0, 8)}…${s.slice(-8)}`;

$("#feekey").append(feeKey());

/* ---------- Transaction list ---------- */

const tbody = $("#txs");
let listed = 0;
function more() {
  const next = all.slice(listed, listed + 25);
  tbody.append(...next.map((t, k) => {
    const i = listed + k;
    const tr = h("tr", { onpointerenter: () => { highlight = i; draw(); }, onpointerleave: () => { highlight = -1; draw(); } },
      h("td", {}, hash(t.txid, { href: page("tx.html") })),
      h("td", {}, t.coinbase ? h("span", { class: "muted" }, "Coinbase") : feeChip(t.rate, false)),
      h("td", { class: "num-col num" }, t.coinbase ? "" : group(t.fee)),
      h("td", { class: "num-col num" }, group(t.vsize)),
      h("td", { class: "num-col" }, btc(t.value, { unit: false })));
    return tr;
  }));
  listed += next.length;
  $("#txs-meta").textContent = `${group(listed)} of ${group(all.length)}, by fee rate`;
  $("#more").hidden = listed >= all.length;
}
$("#more").addEventListener("click", more);
more();

