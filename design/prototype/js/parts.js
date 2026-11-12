// Components shared by more than one prototype page.
import { h, $, FEE_BOUNDS } from "./common.js";

/** The fee key: one swatch per band with its lower bound. The same key appears everywhere fee color does. */
export function feeKey() {
  return h("ol", { class: "feekey__list" },
    ...[0.1, ...FEE_BOUNDS].map((lo, i) => h("li", { class: "feekey__band", vars: { "--c": `var(--fee-${i + 1})` } },
      h("span", { class: "feekey__swatch", "aria-hidden": "true" }),
      h("span", { class: "feekey__label num" }, i === FEE_BOUNDS.length ? `${lo}+` : String(lo)))));
}

/** Show the shared tooltip at a point. Lines are plain strings; the first is the title. */
export function showTip(lines, x, y) {
  const tip = $("#tooltip");
  tip.replaceChildren(...lines.map((l, i) => h(i === 0 ? "strong" : "span", {}, l)));
  tip.hidden = false;
  const w = tip.offsetWidth, ht = tip.offsetHeight;
  const left = Math.min(innerWidth - w - 8, Math.max(8, x - w / 2));
  const top = y - ht - 12 < 8 ? y + 18 : y - ht - 12;
  tip.style.setProperty("translate", `${left}px ${top}px`);
}
export const hideTip = () => { $("#tooltip").hidden = true; };

/** Tooltip on hover and keyboard focus of an element. */
export function tooltip(el, lines) {
  const at = () => { const r = el.getBoundingClientRect(); showTip(lines(), r.left + r.width / 2, r.top); };
  el.addEventListener("pointerenter", at);
  el.addEventListener("focus", at);
  el.addEventListener("pointerleave", hideTip);
  el.addEventListener("blur", hideTip);
}
