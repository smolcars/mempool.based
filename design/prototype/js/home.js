import { h, $, initChrome, params, FEE_BOUNDS, feeVar, fmtRate, feeChip, group, btc, bytes, hash, ago, cssVar, reducedMotion, page } from "./common.js";
import { rand, reseed, tx, blockTxs, summarize, pool, TIP } from "./data.js";
import { feeKey, tooltip } from "./parts.js";

initChrome();
reseed(Number(params.get("seed") ?? 7));

const ms = (name) => parseFloat(cssVar(name)) || 0;

/* ---------- State ---------- */

const now = Date.now();
let tip = TIP;
const mined = Array.from({ length: 6 }, (_, i) => ({
  height: TIP - i, pool: pool(), time: now - (i * 9.6 + 1.7 + rand() * 3) * 60_000, ...summarize(blockTxs(0.97 + rand() * 0.03, 0.9)),
}));
const projected = [summarize(blockTxs(1, 1.05)), summarize(blockTxs(1, 0.6)), summarize(blockTxs(1, 0.4)), restOfMempool()];

function restOfMempool() {
  // The last projected tile stands for everything still waiting behind the first three blocks.
  const s = summarize(blockTxs(1, 0.25));
  const blocks = 9 + Math.floor(rand() * 4);
  return { ...s, rest: { blocks, txCount: s.txCount * blocks } };
}

/* ---------- Block tiles ---------- */

// Tiles are narrow: "13 min", not "13 min ago". The aria-label and title carry the full phrase.
const shortAgo = (t) => ago(Date.now() - t).replace(/ ago$/, "");

function strata(s, fill) {
  const total = s.bands.reduce((a, b) => a + b, 0) || 1;
  return h("span", { class: "tile__strata", "aria-hidden": "true" },
    h("span", { class: "tile__fill", vars: { "--fill": String(fill) } },
      ...s.bands.map((vb, i) => vb > 0 ? h("span", { class: "stratum", vars: { "--grow": String(vb / total), "--c": `var(--fee-${i + 1})` } }) : null)));
}

function projectedLabel(i, s) {
  if (i === 0) return ["Next block", "~10 min"];
  if (s.rest) return [`+${s.rest.blocks} blocks`, ""];
  return [`In ~${(i + 1) * 10} min`, ""];
}

function tile(s, kind, i = 0) {
  const isMined = kind === "mined";
  const [title, sub] = isMined ? [group(s.height), null] : projectedLabel(i, s);
  const body = h(isMined ? "a" : "div", { class: "tile__body", href: isMined ? page(`block.html?h=${s.height}`) : null },
    h("span", { class: "tile__title num" }, title),
    h("span", { class: "tile__fee" }, h("span", { class: "num" }, `~${fmtRate(s.median)}`), h("span", { class: "unit" }, " sat/vB")),
    h("span", { class: "tile__range num" }, `${fmtRate(s.min)} – ${fmtRate(s.max)}`),
    strata(s, isMined ? 1 : Math.min(1, s.vsize / 998_000)),
    h("span", { class: "tile__foot" },
      h("span", { class: "num" }, `${group(s.rest?.txCount ?? s.txCount)} tx`),
      isMined ? h("time", { datetime: new Date(s.time).toISOString(), title: ago(Date.now() - s.time) }, shortAgo(s.time)) : h("span", {}, sub)));
  const li = h("li", { class: `tile tile--${kind}` }, body, h("span", { class: "tile__pool" }, isMined ? s.pool : ""));
  li.setAttribute("aria-label", isMined
    ? `Block ${s.height}, mined by ${s.pool} ${ago(Date.now() - s.time)}, ${s.txCount} transactions, median ${fmtRate(s.median)} sat/vB`
    : `${title}, ${s.txCount} transactions, median ${fmtRate(s.median)} sat/vB`);
  li._data = s;
  return li;
}

const laneP = $("#lane-projected");
const laneM = $("#lane-mined");
laneP.append(...projected.map((s, i) => tile(s, "projected", i)));
laneM.append(...mined.map((s) => tile(s, "mined")));

// Phones scroll the strip; start with the "now" line in the middle.
const strip = $("#strip");
const centerStrip = () => { const n = $("#now"); strip.scrollLeft = n.offsetLeft - strip.clientWidth / 2 + n.offsetWidth / 2; };
centerStrip();

setInterval(() => {
  for (const li of laneM.children) {
    const t = li.querySelector("time");
    if (t && li._data) t.textContent = shortAgo(li._data.time);
  }
}, 15_000);

/* ---------- The new-block sequence ---------- */

let busy = false;

function mineBlock() {
  if (busy) return;
  busy = true;
  const dSlow = ms("--dur-slow"), dBlock = ms("--dur-block"), stagger = ms("--stagger");
  const tiles = [...laneP.children, ...laneM.children];
  const first = new Map(tiles.map((t) => [t, t.getBoundingClientRect()]));

  // The mined tile at the far end leaves the flow and fades where it stands.
  const leaving = laneM.lastElementChild;
  const lr = first.get(leaving), laneRect = laneM.getBoundingClientRect();
  leaving.classList.add("tile--leaving");
  leaving.style.setProperty("left", `${lr.left - laneRect.left}px`);
  const still = reducedMotion(); // reduced motion keeps the fades and drops every movement
  const left = leaving.animate([{ opacity: 1 }, { opacity: 0, transform: still ? "none" : "translateX(24px)" }], { duration: dSlow || ms("--dur-fade"), easing: "ease-out", fill: "forwards" })
    .finished.then(() => leaving.remove());

  // The next projected block crosses the line and becomes the newest mined block.
  const crossing = laneP.firstElementChild;
  const s = { ...crossing._data, height: ++tip, pool: pool(), time: Date.now() };
  const minedTile = tile(s, "mined");
  crossing.remove();
  laneM.prepend(minedTile);
  first.set(minedTile, first.get(crossing));

  // The old "rest of the mempool" tile becomes an ordinary block, and a new one forms at the far end.
  const formerRest = laneP.lastElementChild;
  formerRest._data = { ...formerRest._data, rest: undefined };
  [...laneP.children].forEach(relabel);
  const newTile = tile(restOfMempool(), "projected", laneP.children.length);
  newTile.classList.add("tile--forming");
  laneP.append(newTile);

  // FLIP: every tile that stayed glides from where it was to where it is.
  for (const [el, r] of first) {
    if (el === leaving || !el.isConnected || el === minedTile) continue;
    const n = el.getBoundingClientRect();
    const dx = r.left - n.left;
    if (Math.abs(dx) < 1) continue;
    // Leaders move first so no tile catches the one ahead: the far mined tile, the projected tile nearest the line.
    const isMined = el.parentElement === laneM;
    const order = isMined ? laneM.children.length - 1 - [...laneM.children].indexOf(el) : 2 + [...laneP.children].indexOf(el);
    el.animate([{ transform: `translateX(${dx}px)` }, { transform: "none" }],
      { duration: dSlow, easing: cssVar("--ease-out"), delay: order * stagger, fill: "backwards" });
  }

  // The crossing tile arcs over the line, lifts, and settles with a small overshoot.
  const r0 = first.get(minedTile), r1 = minedTile.getBoundingClientRect();
  const dx = r0.left - r1.left;
  minedTile.classList.add("tile--landing");
  minedTile.animate([
    { transform: `translateX(${dx}px)` },
    { transform: `translate(${dx * 0.45}px, -14px) scale(1.05)`, offset: 0.45 },
    { transform: "none" },
  ], { duration: dBlock, easing: cssVar("--ease-in-out") });
  minedTile.querySelector(".tile__body").animate(
    [{ transform: "scaleY(1)" }, { transform: "scaleY(1)", offset: 0.82 }, { transform: "scaleY(0.94)", offset: 0.9 }, { transform: "scaleY(1)" }],
    { duration: dBlock, easing: cssVar("--ease-settle") });

  // The forming tile's layers drop in from above, cheapest first, like sediment.
  newTile.animate([{ opacity: 0, transform: still ? "none" : "translateX(-28px)" }, { opacity: 1, transform: "none" }],
    { duration: dSlow || ms("--dur-fade"), easing: cssVar("--ease-out"), delay: dBlock * 0.35, fill: "backwards" });
  [...newTile.querySelectorAll(".stratum")].forEach((el, i) => el.animate(
    [{ opacity: 0, transform: "translateY(-18px)" }, { opacity: 1, transform: "none" }],
    { duration: dSlow, easing: cssVar("--ease-settle"), delay: dBlock * 0.4 + i * stagger, fill: "backwards" }));

  const now = $("#now");
  now.classList.remove("strip__now--pulse");
  void now.offsetWidth; // restart the CSS animation
  now.classList.add("strip__now--pulse");

  $("#announce").textContent = `Block ${s.height} mined by ${s.pool}: ${s.txCount} transactions, median fee ${fmtRate(s.median)} sat/vB.`;
  mempool.count = Math.max(1000, mempool.count - s.txCount + Math.round(s.txCount * (0.6 + rand() * 0.3)));
  mempool.vsize = Math.max(1e6, mempool.vsize - s.vsize + Math.round(s.vsize * (0.5 + rand() * 0.4)));
  drift();
  renderMempool();
  updateRecs();
  // Stay busy until the old tile is gone too, or a quick second block would pick it again.
  const settled = new Promise((r) => setTimeout(r, dBlock + 4 * stagger + dSlow * 0.5));
  Promise.all([settled, left]).then(() => {
    minedTile.classList.remove("tile--landing");
    newTile.classList.remove("tile--forming");
    busy = false;
  });
}

function relabel(li, i) {
  const [title, sub] = projectedLabel(i, li._data);
  li.querySelector(".tile__title").textContent = title;
  li.querySelector(".tile__foot > :first-child").textContent = `${group(li._data.txCount)} tx`;
  li.querySelector(".tile__foot > :last-child").textContent = sub;
  li.setAttribute("aria-label", `${title}, ${li._data.txCount} transactions, median ${fmtRate(li._data.median)} sat/vB`);
}

$("#mine").addEventListener("click", mineBlock);
const auto = $("#auto");
let timer = null;
const setAuto = (on) => {
  auto.setAttribute("aria-pressed", String(on));
  clearInterval(timer);
  timer = on ? setInterval(mineBlock, 6500) : null;
};
auto.addEventListener("click", () => setAuto(auto.getAttribute("aria-pressed") !== "true"));
setAuto(params.get("autoplay") !== "0");

/* ---------- Mempool ---------- */

const mempool = { count: 41_208, vsize: 14_620_000, fees: 7_120_000, bands: [0.3, 0.17, 0.14, 0.13, 0.08, 0.05, 0.04, 0.04, 0.03, 0.02] };
function drift() {
  mempool.bands = mempool.bands.map((b) => b * (0.85 + rand() * 0.3));
  const t = mempool.bands.reduce((a, b) => a + b, 0);
  mempool.bands = mempool.bands.map((b) => b / t);
  mempool.fees = Math.round(mempool.vsize * 0.48 * (0.9 + rand() * 0.2));
}
drift();

const stats = $("#mempool-stats");
const statEls = {};
for (const [key, label] of [["count", "Transactions"], ["vsize", "Size"], ["fees", "Fees waiting"], ["floor", "Lowest accepted"]]) {
  statEls[key] = h("dd", { class: "stat__value" });
  stats.append(h("div", { class: "stat" }, h("dt", { class: "stat__label" }, label), statEls[key]));
}

const feebar = $("#feebar");
const segs = mempool.bands.map((_, i) => {
  const lo = i === 0 ? 0.1 : FEE_BOUNDS[i - 1], hi = FEE_BOUNDS[i];
  const seg = h("span", { class: "feebar__seg", vars: { "--c": `var(--fee-${i + 1})` }, tabindex: "0" });
  seg._range = hi ? `${lo}–${hi} sat/vB` : `${lo} sat/vB and up`;
  tooltip(seg, () => [seg._range, `${bytes(seg._vb)}, about ${group(seg._n)} transactions`]);
  feebar.append(seg);
  return seg;
});

function renderMempool() {
  statEls.count.replaceChildren(h("span", { class: "num" }, group(mempool.count)));
  statEls.vsize.replaceChildren(h("span", { class: "num" }, bytes(mempool.vsize)), h("span", { class: "stat__sub" }, `≈ ${Math.ceil(mempool.vsize / 1e6)} blocks`));
  statEls.fees.replaceChildren(btc(mempool.fees));
  statEls.floor.replaceChildren(feeChip(0.1));
  let x = 0;
  segs.forEach((seg, i) => {
    const w = mempool.bands[i];
    seg.style.setProperty("--x", String(x));
    seg.style.setProperty("--w", String(w));
    seg._vb = w * mempool.vsize;
    seg._n = w * mempool.count;
    x += w;
  });
  feebar.setAttribute("aria-label", `Mempool by fee rate: ${segs.map((s) => `${s._range} ${Math.round((s._vb / mempool.vsize) * 100)}%`).join(", ")}`);
}
renderMempool();
$("#feekey").append(feeKey());

/* ---------- Fee ladder ---------- */

const ladder = $("#ladder");
const LO = Math.log10(0.1), HI = Math.log10(20);
const pos = (r) => (Math.log10(Math.min(20, Math.max(0.1, r))) - LO) / (HI - LO);
const scale = h("div", { class: "ladder__scale" },
  ...[0.1, ...FEE_BOUNDS].map((b, i) => h("span", { class: "ladder__band", vars: { "--x": String(pos(b)), "--w": String(pos(FEE_BOUNDS[i] ?? 20) - pos(b)), "--c": `var(--fee-${i + 1})` } })));
const ticks = h("div", { class: "ladder__ticks", "aria-hidden": "true" },
  ...[0.1, 0.2, 0.5, 1, 2, 5, 10].map((t) => h("span", { class: "ladder__tick num", vars: { "--x": String(pos(t)) } }, String(t))));
const PRIORITIES = [["No rush", "within a day"], ["Low", "within an hour"], ["Medium", "within 30 min"], ["High", "next block"]];
let recs = [0.1, 0.21, 0.52, 1.24];
const pins = PRIORITIES.map(([name]) => h("div", { class: "ladder__pin" },
  h("span", { class: "ladder__pin-label" }, h("span", { class: "ladder__pin-name" }, name), h("span", { class: "ladder__pin-value num" }))));
const list = h("dl", { class: "ladder__list" });
const listVals = PRIORITIES.map(([name, when]) => {
  const dd = h("dd", {});
  list.append(h("div", { class: "ladder__item" }, h("dt", {}, name, h("span", { class: "ladder__when" }, when)), dd));
  return dd;
});
const track = h("div", { class: "ladder__track" }, scale, ...pins, ticks);
ladder.append(track, list);

function updateRecs() {
  recs = recs.map((r, i) => (i === 0 ? 0.1 : Math.max(0.1, r * (0.8 + rand() * 0.45)))).sort((a, b) => a - b);
  pins.forEach((p, i) => {
    p.style.setProperty("--c", feeVar(recs[i]));
    p.querySelector(".ladder__pin-value").textContent = fmtRate(recs[i]);
    listVals[i].replaceChildren(feeChip(recs[i]));
  });
  layoutLadder();
}

// Pins sit on a log scale; labels stack into rows so close values never overlap, and flip left near the end.
function layoutLadder() {
  const W = track.clientWidth;
  const rows = [];
  pins.forEach((p, i) => {
    const x = pos(recs[i]) * W;
    const w = p.querySelector(".ladder__pin-label").offsetWidth + 12;
    const flip = x + w > W;
    const [from, to] = flip ? [x - w, x] : [x, x + w];
    let row = rows.findIndex((end) => end < from);
    if (row === -1) { rows.push(0); row = rows.length - 1; }
    rows[row] = to;
    p.style.setProperty("--px", `${x}px`);
    p.style.setProperty("--row", String(row));
    p.classList.toggle("ladder__pin--flip", flip);
  });
  ladder.style.setProperty("--rows", String(rows.length));
}
updateRecs();
new ResizeObserver(layoutLadder).observe(track);

/* ---------- Latest transactions ---------- */

const recent = $("#recent");
function row(t, fresh = false) {
  return h("tr", { class: fresh ? "row--new" : null },
    h("td", {}, hash(t.txid, { href: page("tx.html"), copy: false })),
    h("td", {}, feeChip(t.rate, false)),
    h("td", { class: "num-col num" }, group(t.vsize)),
    h("td", { class: "num-col" }, btc(t.value, { unit: false })));
}
recent.append(...Array.from({ length: 8 }, () => row(tx())));
setInterval(() => {
  recent.prepend(row(tx(), true));
  recent.lastElementChild.remove();
}, 2200);
