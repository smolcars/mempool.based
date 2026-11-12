import { h, $, initChrome, params, feeBand, fmtRate, feeChip, feeVar, group, btc, sats, hash, hashFull, utc, cssVar, reducedMotion, page } from "./common.js";
import { reseed, hex, address, summarize, blockTxs, TIP } from "./data.js";

initChrome();
reseed(42);

/* ---------- Sample story: version A is replaced by version B, which then confirms ---------- */

const ins = [{ addr: address(), value: 4_210_000 }, { addr: address(), value: 1_503_112 }];
const pay = address(), change = address();
const vsize = 209;
const seen = Date.now() - 41 * 60_000;
const A = { txid: hex(), rate: 44 / vsize, fee: 44, outs: [{ addr: pay, value: 3_500_000 }, { addr: change, value: 2_213_068 }] };
const B = { txid: hex(), rate: 167 / vsize, fee: 167, outs: [{ addr: pay, value: 3_500_000 }, { addr: change, value: 2_212_945 }] };
const replacedAt = seen + 2 * 60_000 + 28_000;
const confirmedAt = seen + 9 * 60_000 + 5_000;
const nextBlock = summarize(blockTxs(1, 1.05));

const STATES = {
  unconfirmed: { tx: B, label: "Unconfirmed", tone: "caution" },
  replaced: { tx: A, label: "Replaced", tone: "negative" },
  confirmed: { tx: B, label: "Confirmed", tone: "positive" },
};
let state = STATES[params.get("state") ?? ""] ? params.get("state") : "confirmed";

/* ---------- Render ---------- */

const ICONS = {
  caution: [h("svg:circle", { cx: 8, cy: 8, r: 5.5, "stroke-dasharray": "2.6 2" })],
  negative: [h("svg:path", { d: "M5 5l6 6M11 5l-6 6" })],
  positive: [h("svg:path", { d: "M4.5 8.5l2.4 2.4L11.8 5.6" })],
};
const icon = (tone) => h("svg:svg", { class: "icon", viewBox: "0 0 16 16", "aria-hidden": "true" }, ...ICONS[tone].map((n) => n.cloneNode()));

function render() {
  const st = STATES[state];
  const t = st.tx;
  for (const b of document.querySelectorAll(".seg")) b.setAttribute("aria-pressed", String(b.dataset.state === state));

  $("#status").className = `status status--${st.tone}`;
  $("#status").replaceChildren(icon(st.tone), st.label);
  $("#txid").replaceChildren(hashFull(t.txid));

  const notice = $("#notice");
  notice.hidden = state !== "replaced";
  if (state === "replaced") {
    notice.replaceChildren(icon("negative"),
      h("p", {}, "This version was replaced by ", hash(B.txid, { href: page("tx.html?state=confirmed"), copy: false }),
        ", which pays ", h("strong", {}, `${fmtRate(B.rate)} sat/vB`), "."));
  }

  // Status timeline. Steps are a real sequence, so they are numbered.
  const steps = [
    { title: "First seen", detail: utc(seen), done: true },
    state === "replaced"
      ? { title: "Replaced", detail: `${utc(replacedAt)}. The new version pays ${fmtRate(B.rate)} sat/vB, up from ${fmtRate(A.rate)}.`, tone: "negative", current: true }
      : { title: "Replaced an earlier version", detail: `${utc(replacedAt)}. Fee raised from ${fmtRate(A.rate)} to ${fmtRate(B.rate)} sat/vB.`, done: true },
    state === "confirmed"
      ? { title: "Confirmed", detail: `Block ${group(TIP)}, 6 confirmations. ${utc(confirmedAt)}.`, done: true, tone: "positive" }
      : state === "replaced"
        ? { title: "Will not confirm", detail: "Its inputs are now spent by the new version.", dead: true }
        : { title: "Waiting for a block", detail: "Projected for the next block.", current: true, tone: "caution" },
  ];
  $("#timeline").replaceChildren(...steps.map((s, i) => h("li", {
    class: `step${s.done ? " step--done" : ""}${s.current ? " step--current" : ""}${s.dead ? " step--dead" : ""}${s.tone ? ` step--${s.tone}` : ""}`,
    "aria-current": s.current ? "step" : null,
  }, h("span", { class: "step__marker num", "aria-hidden": "true" }, String(i + 1)),
    h("span", { class: "step__title" }, s.title),
    h("span", { class: "step__detail" }, s.detail))));

  // Fee facts.
  const inTotal = ins.reduce((a, b) => a + b.value, 0);
  $("#fee-facts").replaceChildren(...[
    ["Fee rate", [feeChip(t.rate)]],
    ["Fee", [sats(t.fee), h("span", { class: "fact__sub" }, btc(t.fee))]],
    ["Virtual size", [h("span", { class: "num" }, `${vsize} vB`), h("span", { class: "fact__sub num" }, `${vsize * 4 - 3} WU`)]],
    ["Chunk fee rate", [feeChip(t.rate), h("span", { class: "fact__sub" }, "Mined together with its cluster at this rate")]],
    ["Replaceable", [{ confirmed: "No, confirmed", replaced: "No, already replaced" }[state] ?? "Yes"]],
    ["Total in", [btc(inTotal)]],
  ].map(([k, v]) => h("div", { class: "fact" }, h("dt", {}, k), h("dd", {}, ...v))));

  renderPosition(t);
  renderFlow(t);
  $("#flow-meta").textContent = `${ins.length} inputs, ${t.outs.length} outputs`;
}

/** Where this fee rate sits against the next projected block, drawn as that block's strata laid on its side. */
function renderPosition(t) {
  const total = nextBlock.bands.reduce((a, b) => a + b, 0);
  const below = nextBlock.bands.slice(0, feeBand(t.rate) - 1).reduce((a, b) => a + b, 0) / total;
  const pos = $("#position");
  let x = 0;
  pos.replaceChildren(
    h("p", { class: "position__title" }, state === "confirmed" ? "Against the block it confirmed in" : "Against the next projected block"),
    h("div", { class: "position__bar", role: "img", "aria-label": `Pays more than about ${Math.round(below * 100)}% of the block by size` },
      ...nextBlock.bands.map((vb, i) => {
        const seg = h("span", { class: "position__seg", vars: { "--x": String(x), "--w": String(vb / total), "--c": `var(--fee-${i + 1})` } });
        x += vb / total;
        return seg;
      }),
      h("span", { class: "position__pin", vars: { "--x": String(below), "--c": feeVar(t.rate) } })),
    h("p", { class: "position__text" }, `Pays more than about ${Math.round(below * 100)}% of it by size.`));
}

/* ---------- Flow: inputs to outputs, ribbon width follows value ---------- */

function flowItem(label, addr, value, cls = "") {
  return h("li", { class: `flow__item ${cls}` },
    h("span", { class: "flow__amount" }, btc(value, { unit: false })),
    addr ? hash(addr, { head: 7, tail: 5, copy: false, href: "#" }) : h("span", { class: "flow__label" }, label));
}

function renderFlow(t) {
  const fin = $("#flow-in"), fout = $("#flow-out");
  fin.replaceChildren(...ins.map((i) => flowItem("", i.addr, i.value)));
  fout.replaceChildren(...t.outs.map((o) => flowItem("", o.addr, o.value)),
    h("li", { class: "flow__item flow__item--fee", vars: { "--c": feeVar(t.rate) } },
      h("span", { class: "flow__amount" }, btc(t.fee, { unit: false })),
      h("span", { class: "flow__label" }, `Fee, ${fmtRate(t.rate)} sat/vB`)));
  requestAnimationFrame(() => drawRibbons(t, true));
}

function drawRibbons(t, animate = false) {
  const svg = $("#flow-svg");
  const box = svg.getBoundingClientRect();
  const W = box.width, H = box.height;
  svg.setAttribute("viewBox", `0 0 ${W} ${H}`);
  const inTotal = ins.reduce((a, b) => a + b.value, 0);
  const mid = W / 2;
  const trunk = Math.min(H * 0.8, 96); // the thickness of the transaction itself at the middle
  const trunkTop = H / 2 - trunk / 2;
  const min = 3;

  const side = (items, values, toRight) => {
    let y = trunkTop;
    return values.map((v, k) => {
      const r = items[k].getBoundingClientRect();
      const cy = r.top + r.height / 2 - box.top;
      const th = Math.max(min, (v / inTotal) * trunk);
      const a = { x: toRight ? 0 : W, y0: cy - th / 2, y1: cy + th / 2 };
      const b = { x: mid, y0: y, y1: y + th };
      y += th;
      const c = (toRight ? 1 : -1) * W * 0.32;
      return `M${a.x},${a.y0} C${a.x + c},${a.y0} ${b.x - c},${b.y0} ${b.x},${b.y0} L${b.x},${b.y1} C${b.x - c},${b.y1} ${a.x + c},${a.y1} ${a.x},${a.y1} Z`;
    });
  };

  const inPaths = side([...$("#flow-in").children], ins.map((i) => i.value), true);
  const outItems = [...$("#flow-out").children];
  const outPaths = side(outItems, [...t.outs.map((o) => o.value), t.fee], false);
  const ribbons = [
    ...inPaths.map((d) => h("svg:path", { d, class: "ribbon ribbon--in" })),
    ...outPaths.map((d, k) => h("svg:path", { d, class: k === t.outs.length ? "ribbon ribbon--fee" : "ribbon ribbon--out" })),
  ];
  const trunkEl = h("svg:rect", { class: "flow__trunk", x: mid - 3, y: trunkTop - 4, width: 6, height: trunk + 8, rx: 3 });
  svg.replaceChildren(...ribbons, trunkEl);
  const fee = svg.querySelector(".ribbon--fee");
  fee?.style.setProperty("fill", feeVar(t.rate));

  if (animate && !reducedMotion()) {
    const d = parseFloat(cssVar("--dur-slow")) || 0, s = parseFloat(cssVar("--stagger")) || 0;
    ribbons.forEach((r, k) => r.animate([{ opacity: 0, transform: "translateX(-8px)" }, { opacity: 1, transform: "none" }],
      { duration: d, delay: (k < inPaths.length ? 0 : d * 0.5) + k * s, easing: cssVar("--ease-out"), fill: "backwards" }));
  }
}

for (const b of document.querySelectorAll(".seg")) b.addEventListener("click", () => {
  state = b.dataset.state;
  const u = new URL(location.href);
  u.searchParams.set("state", state);
  history.replaceState(null, "", u);
  render();
});
new ResizeObserver(() => drawRibbons(STATES[state].tx)).observe($("#flow"));
render();
