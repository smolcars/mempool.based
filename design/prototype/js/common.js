// Shared prototype helpers. All chain data goes into the page as text (SECURITY.md rule 14): no innerHTML anywhere.

const SVG_NS = "http://www.w3.org/2000/svg";

/** Build an element. Children are nodes or strings; strings become text nodes. */
export function h(tag, attrs = {}, ...children) {
  const svg = tag.startsWith("svg:");
  const el = svg ? document.createElementNS(SVG_NS, tag.slice(4)) : document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v == null || v === false) continue;
    if (k === "class") el.setAttribute("class", v);
    else if (k === "vars") for (const [p, val] of Object.entries(v)) el.style.setProperty(p, val);
    else if (k.startsWith("on")) el.addEventListener(k.slice(2), v);
    else el.setAttribute(k, v === true ? "" : String(v));
  }
  for (const c of children.flat()) if (c != null && c !== false) el.append(c instanceof Node ? c : String(c));
  return el;
}

export const $ = (sel, root = document) => root.querySelector(sel);
export const reducedMotion = () => matchMedia("(prefers-reduced-motion: reduce)").matches;

/* ---------- Fees ---------- */

// Upper bounds (sat/vB) of fee bands 1–9; band 10 is everything at or above 10.
export const FEE_BOUNDS = [0.2, 0.3, 0.5, 1, 1.5, 2, 3, 5, 10];
export const feeBand = (rate) => {
  const i = FEE_BOUNDS.findIndex((b) => rate < b);
  return i === -1 ? 10 : i + 1;
};
export const feeVar = (rate) => `var(--fee-${feeBand(rate)})`;

/** Fee rate as text: two decimals under 10, one under 100, none above. */
export const fmtRate = (r) => (r < 10 ? r.toFixed(2) : r < 100 ? r.toFixed(1) : Math.round(r).toString());

/** A fee-rate chip: swatch plus number. The color is never the only carrier. */
export function feeChip(rate, unit = true) {
  return h("span", { class: "fee", vars: { "--swatch": feeVar(rate) } },
    h("span", { class: "fee__swatch", "aria-hidden": "true" }),
    h("span", { class: "num" }, fmtRate(rate)),
    unit ? h("span", { class: "unit" }, " sat/vB") : null);
}

/* ---------- Numbers ---------- */

const NNBSP = " "; // narrow no-break space groups digits without letting them wrap
export const group = (n) => Math.round(n).toString().replace(/\B(?=(\d{3})+(?!\d))/g, NNBSP);

/** BTC with all eight decimals; trailing zeros are dimmed so the significant part reads first. */
export function btc(sats, { unit = true } = {}) {
  const neg = sats < 0;
  const abs = Math.abs(Math.round(sats));
  const whole = group(Math.floor(abs / 1e8));
  const frac = String(abs % 1e8).padStart(8, "0");
  const sig = frac.replace(/0+$/, "");
  const keep = sig.length === 0 ? "" : sig;
  return h("span", { class: "amount num", title: `${group(abs)} sats` },
    neg ? "−" : "", whole, ".", keep,
    h("span", { class: "amount__zeros" }, frac.slice(keep.length)),
    unit ? h("span", { class: "unit" }, " BTC") : null);
}

export const sats = (n) => h("span", { class: "num" }, group(n), h("span", { class: "unit" }, " sats"));

export function bytes(vb, unit = "vB") {
  if (vb >= 1e6) return `${(vb / 1e6).toFixed(2)} M${unit}`;
  if (vb >= 1e4) return `${(vb / 1e3).toFixed(1)} k${unit}`;
  return `${group(vb)} ${unit}`;
}

/* ---------- Hashes ---------- */

/** Hash or address, truncated in the middle. The full value is in the title and the copy button. */
export function hash(value, { head = 8, tail = 8, copy = true, href } = {}) {
  const short = value.length > head + tail + 1 ? `${value.slice(0, head)}…${value.slice(-tail)}` : value;
  const text = href ? h("a", { class: "hash__text", href, title: value }, short) : h("span", { class: "hash__text", title: value }, short);
  return h("span", { class: "hash" }, text, copy ? copyButton(value) : null);
}

/** Full hash, broken into groups of eight so it wraps cleanly and is easy to compare by eye. */
export function hashFull(value) {
  const groups = value.match(/.{1,8}/g) ?? [value];
  return h("span", { class: "hash hash--full" },
    h("span", { class: "hash__text" }, ...groups.map((g) => h("span", { class: "hash__group" }, g))),
    copyButton(value));
}

export function copyButton(value) {
  const btn = h("button", { class: "copy", type: "button", "aria-label": "Copy", title: "Copy" },
    h("svg:svg", { viewBox: "0 0 16 16", "aria-hidden": "true" },
      h("svg:rect", { x: 5, y: 5, width: 8.5, height: 8.5, rx: 1.5 }),
      h("svg:path", { d: "M3 10.5V3.5A1 1 0 0 1 4 2.5h6.5" })));
  btn.addEventListener("click", async () => {
    try { await navigator.clipboard.writeText(value); } catch { /* clipboard may be blocked; the state below still confirms the press */ }
    btn.dataset.copied = "";
    btn.setAttribute("aria-label", "Copied");
    setTimeout(() => { delete btn.dataset.copied; btn.setAttribute("aria-label", "Copy"); }, 1400);
  });
  return btn;
}

/* ---------- Time ---------- */

export function ago(ms) {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 10) return "just now";
  if (s < 60) return `${s} s ago`;
  const m = Math.round(s / 60);
  if (m < 60) return `${m} min ago`;
  const hr = Math.floor(m / 60);
  return hr < 48 ? `${hr} h ${m % 60} min ago` : `${Math.floor(hr / 24)} days ago`;
}
export const utc = (t) => new Date(t).toISOString().replace("T", " ").slice(0, 19) + " UTC";

/* ---------- Chrome: theme, network, header ---------- */

const NETWORKS = {
  mainnet: { name: "Mainnet", band: null, prefix: "bc1" },
  signet: { name: "Signet", band: "Signet is a test network. Coins here have no value.", prefix: "tb1" },
  testnet4: { name: "Testnet4", band: "Testnet4 is a test network. Coins here have no value.", prefix: "tb1" },
};

export const params = new URLSearchParams(location.search);
export const network = NETWORKS[params.get("network") ?? ""] ? params.get("network") : "mainnet";
export const addrPrefix = NETWORKS[network].prefix;

function storedTheme() {
  try { return localStorage.getItem("theme"); } catch { return null; }
}

export function initChrome() {
  const root = document.documentElement;
  const theme = params.get("theme") ?? storedTheme();
  if (theme === "light" || theme === "dark") root.dataset.theme = theme;
  root.dataset.network = network;
  const net = NETWORKS[network];
  if (network !== "mainnet") document.title = `${net.name}: ${document.title}`;

  // Keep the network and theme on internal links so the prototype behaves like the routed app.
  for (const a of document.querySelectorAll("a[data-nav]")) {
    const u = new URL(a.getAttribute("href"), location.href);
    if (network !== "mainnet") u.searchParams.set("network", network);
    a.href = u.pathname + u.search;
  }

  const band = $("#network-band");
  if (band && net.band) { band.hidden = false; $(".network-band__text", band).textContent = net.band; }

  const select = $("#network-select");
  if (select) {
    select.value = network;
    select.addEventListener("change", () => {
      const u = new URL(location.href);
      if (select.value === "mainnet") u.searchParams.delete("network"); else u.searchParams.set("network", select.value);
      location.href = u.toString();
    });
  }

  $("#theme-toggle")?.addEventListener("click", () => {
    const dark = getComputedStyle(root).colorScheme === "dark";
    root.dataset.theme = dark ? "light" : "dark";
    try { localStorage.setItem("theme", root.dataset.theme); } catch { /* private mode: theme still applies for this page */ }
  });
}

export const cssVar = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
