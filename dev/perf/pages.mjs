// Page loads in headless Chromium, cold each time: first paint, the main content showing data,
// bytes, requests and JavaScript run, unthrottled and as a slow phone. Then, with --frames, the
// frame times through a new block on the home page and through a block's transaction grid loading.
// Results: docs/PERFORMANCE.md "The site in a browser". Run from web/, so Playwright resolves:
//
//   cd web && node ../dev/perf/pages.mjs SITE BASE [--runs N] [--manifest FILE] [--frames] [--only a,b]
//
// SITE is `regtest` (our server on the scenario chain; needs --manifest, and --frames mines
// through REGTEST_RPC_* from `regtest.py env`), `mainnet` (our server on mainnet) or `reference`
// (a mempool.space-style site, for page-level comparison only: keep --runs small there). Prints
// one JSON object.
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { join } from "node:path";

const { chromium } = createRequire(join(process.cwd(), "package.json"))("playwright");
const args = process.argv.slice(2);
const flag = (name, fallback) => {
  const i = args.indexOf(`--${name}`);
  return i < 0 ? fallback : args[i + 1];
};
const [site, base] = args;
const runs = Number(flag("runs", "5"));

// Public mainnet data, the same on both sites: block 840,000, a transaction in it, its address.
const MAINNET = {
  block: "0000000000000000000320283a032748cef8227873ff4872689bf23f1cda83a5",
  tx: flag("tx", ""),
  address: flag("address", ""),
};

/** name -> [path, what shows the page's main content has its data (see `shows`), or null] */
function pagesFor() {
  const home = { all: ['.home-stats[aria-busy="false"]', ".home-strip__projected li"] };
  if (site === "regtest") {
    const m = JSON.parse(readFileSync(flag("manifest"), "utf8"));
    return {
      home: ["/regtest/", home],
      block: [`/regtest/block/${m.blocks["106"]}`, { texts: ["Coinbase", "50.00000000 BTC"] }],
      tx: [`/regtest/tx/${m.txs.script_types}`, { texts: ["Confirmed", "OP_RETURN"] }],
      address: [`/regtest/address/${m.addresses.p2tr}`, { regex: "\\d\\.\\d{8} BTC" }],
      mining: ["/regtest/mining", { regex: "\\d%" }],
      projected: ["/regtest/block/next", { any: ['a[href*="/tx/"]', "text:Nothing waiting here"] }],
      mempool: ["/regtest/mempool", { all: ['.mempool-busy[aria-busy="false"]'] }],
    };
  }
  if (site === "mainnet") {
    return {
      home: ["/", home],
      block: [`/block/${MAINNET.block}`, { texts: ["Coinbase"] }],
      tx: [`/tx/${MAINNET.tx}`, { texts: ["Confirmed"] }],
      address: [`/address/${MAINNET.address}`, { regex: "\\d\\.\\d{8} BTC" }],
      mining: ["/mining", { regex: "\\d%" }],
      projected: ["/block/next", { any: ['a[href*="/tx/"]'] }],
      mempool: ["/mempool", { all: ['.mempool-busy[aria-busy="false"]'] }],
    };
  }
  // The reference site: only paint and load times, which need no knowledge of its page.
  return {
    home: ["/", null],
    block: [`/block/${MAINNET.block}`, null],
    tx: [`/tx/${MAINNET.tx}`, null],
    address: [`/address/${MAINNET.address}`, null],
    mining: ["/mining", null],
    projected: ["/mempool-block/0", null],
    mempool: ["/graphs/mempool", null],
  };
}

/** Runs in the page (a function, not a string: the site's CSP forbids eval). */
function shows(s) {
  const t = document.body ? document.body.innerText : "";
  const has = (q) => (q.startsWith("text:") ? t.includes(q.slice(5)) : !!document.querySelector(q));
  if (s.all && !s.all.every(has)) return false;
  if (s.texts && !s.texts.every((x) => t.includes(x))) return false;
  if (s.regex && !new RegExp(s.regex).test(t)) return false;
  return s.any ? s.any.some(has) : true;
}

// Lighthouse's "Slow 4G" as DevTools applies it (150 ms RTT and 1.6 Mbps, adjusted for
// per-request throttling), with a 4x slower CPU.
const PROFILES = {
  none: null,
  mobile: { cpu: 4, latency: 562.5, down: (1474.56 * 1024) / 8, up: (675 * 1024) / 8 },
};

const median = (v) => {
  const s = v.filter((x) => x !== null).sort((a, b) => a - b);
  return s.length ? s[Math.floor((s.length - 1) / 2)] : null;
};
const summary = (v) => {
  const s = v.filter((x) => x !== null);
  return {
    median: median(s),
    min: s.length ? Math.min(...s) : null,
    max: s.length ? Math.max(...s) : null,
    n: s.length,
  };
};

async function newPage(browser, profile) {
  const context = await browser.newContext({ viewport: { width: 1280, height: 800 } });
  const page = await context.newPage();
  const cdp = await context.newCDPSession(page);
  await cdp.send("Network.enable");
  await cdp.send("Performance.enable");
  if (profile) {
    await cdp.send("Emulation.setCPUThrottlingRate", { rate: profile.cpu });
    await cdp.send("Network.emulateNetworkConditions", {
      offline: false,
      latency: profile.latency,
      downloadThroughput: profile.down,
      uploadThroughput: profile.up,
    });
  }
  return { context, page, cdp };
}

async function load(browser, profile, path, ready) {
  const { context, page, cdp } = await newPage(browser, profile);
  let bytes = 0;
  let jsBytes = 0;
  let requests = 0;
  const scripts = new Set();
  cdp.on("Network.requestWillBeSent", (e) => {
    requests++;
    if (e.type === "Script") scripts.add(e.requestId);
  });
  cdp.on("Network.loadingFinished", (e) => {
    bytes += e.encodedDataLength;
    if (scripts.has(e.requestId)) jsBytes += e.encodedDataLength;
  });
  await page.addInitScript(() => {
    window.__lcp = null;
    new PerformanceObserver((l) => {
      for (const e of l.getEntries()) window.__lcp = e.startTime;
    }).observe({ type: "largest-contentful-paint", buffered: true });
  });
  const timeout = profile ? 60_000 : 20_000;
  await page.goto(base + path, { waitUntil: "commit", timeout });
  let dataMs = null;
  if (ready) {
    await page.waitForFunction(shows, ready, { timeout, polling: "raf" });
    dataMs = await page.evaluate(() => performance.now());
  }
  await page.waitForLoadState("load", { timeout });
  // Let late requests and the last contentful paint settle.
  await page.waitForTimeout(profile ? 4000 : 1500);
  const t = await page.evaluate(() => {
    const paint = Object.fromEntries(
      performance.getEntriesByType("paint").map((e) => [e.name, e.startTime]),
    );
    const nav = performance.getEntriesByType("navigation")[0];
    return {
      fp: paint["first-paint"] ?? null,
      fcp: paint["first-contentful-paint"] ?? null,
      lcp: window.__lcp,
      load: nav ? nav.loadEventEnd : null,
    };
  });
  const metrics = Object.fromEntries(
    (await cdp.send("Performance.getMetrics")).metrics.map((m) => [m.name, m.value]),
  );
  await context.close();
  return {
    ...t,
    data: dataMs,
    bytes,
    js_bytes: jsBytes,
    requests,
    script_ms: metrics.ScriptDuration * 1000,
    task_ms: metrics.TaskDuration * 1000,
  };
}

// rAF intervals for `ms` after `start()` runs, and the long tasks in that window.
async function frames(page, start, ms) {
  await page.evaluate(() => {
    window.__frames = [];
    window.__long = [];
    let last = performance.now();
    const tick = (now) => {
      window.__frames.push(now - last);
      last = now;
      if (window.__recording) requestAnimationFrame(tick);
    };
    window.__recording = true;
    requestAnimationFrame(tick);
    new PerformanceObserver((l) => {
      for (const e of l.getEntries()) window.__long.push(e.duration);
    }).observe({ type: "longtask" });
  });
  await start();
  await page.waitForTimeout(ms);
  const { f, long } = await page.evaluate(() => {
    window.__recording = false;
    return { f: window.__frames.slice(1), long: window.__long };
  });
  const s = [...f].sort((a, b) => a - b);
  const p = (q) => (s.length ? s[Math.min(s.length - 1, Math.ceil(q * s.length) - 1)] : null);
  const round = (x) => (x === null ? null : Math.round(x * 10) / 10);
  return {
    frames: s.length,
    fps: round((s.length / ms) * 1000),
    p50_ms: round(p(0.5)),
    p99_ms: round(p(0.99)),
    max_ms: round(s.at(-1) ?? null),
    over_33ms: s.filter((x) => x > 33.4).length,
    long_tasks: long.length,
    long_task_max_ms: round(long.length ? Math.max(...long) : null),
  };
}

async function rpc(method, params = []) {
  const res = await fetch(`http://127.0.0.1:${process.env.REGTEST_RPC_PORT}/`, {
    method: "POST",
    headers: {
      authorization: `Basic ${btoa(`${process.env.REGTEST_RPC_USER}:${process.env.REGTEST_RPC_PASSWORD}`)}`,
    },
    body: JSON.stringify({ jsonrpc: "1.0", id: 1, method, params }),
  });
  return (await res.json()).result;
}
const MINER = "raw(00204ae81572f06e1b88fd5ced7a1a000945432e83e1551e6f721ee9c00b8cc33260)";

// The machine's GPU, as a laptop's browser would use it; headless Chromium otherwise draws WebGL
// in software (SwiftShader), which would make the frame times a CPU benchmark.
const browser = await chromium.launch({
  args: ["--use-angle=metal", "--enable-gpu", "--ignore-gpu-blocklist"],
});
const probe = await browser.newPage();
const renderer = await probe.evaluate(() => {
  const gl = document.createElement("canvas").getContext("webgl2");
  const ext = gl && gl.getExtension("WEBGL_debug_renderer_info");
  return gl ? gl.getParameter(ext ? ext.UNMASKED_RENDERER_WEBGL : gl.RENDERER) : "no WebGL2";
});
await probe.close();
const out = { site, runs, chromium: browser.version(), renderer, pages: {} };
const only = flag("only", "");
for (const [name, [path, ready]] of Object.entries(pagesFor())) {
  if (only && !only.split(",").includes(name)) continue;
  out.pages[name] = { path };
  for (const [profileName, profile] of Object.entries(PROFILES)) {
    const results = [];
    for (let i = 0; i < runs; i++) {
      try {
        results.push(await load(browser, profile, path, ready));
      } catch (e) {
        results.push({ error: String(e).split("\n")[0] });
      }
    }
    const ok = results.filter((r) => !r.error);
    const keys = [
      "fp",
      "fcp",
      "lcp",
      "data",
      "load",
      "bytes",
      "js_bytes",
      "requests",
      "script_ms",
      "task_ms",
    ];
    out.pages[name][profileName] = Object.fromEntries(
      keys.map((k) => [k, summary(ok.map((r) => (r[k] == null ? null : Math.round(r[k]))))]),
    );
    const errors = results.filter((r) => r.error).map((r) => r.error);
    if (errors.length) out.pages[name][profileName].errors = errors;
    console.error(`pages: ${name} ${profileName} done`);
  }
}

if (args.includes("--frames") && site === "regtest") {
  const m = JSON.parse(readFileSync(flag("manifest"), "utf8"));
  out.frames = {};
  for (const [profileName, profile] of Object.entries(PROFILES)) {
    const cpu = profile ? { ...profile, latency: 0, down: -1, up: -1 } : null; // CPU only
    const blocks = [];
    for (let i = 0; i < Math.min(runs, 3); i++) {
      const { context, page } = await newPage(browser, cpu);
      await page.goto(`${base}/regtest/`);
      await page.waitForFunction(shows, pagesFor().home[1], { timeout: 30_000 });
      await page.waitForTimeout(2000);
      let hash = "";
      const f = await frames(
        page,
        async () => ([hash] = await rpc("generatetodescriptor", [1, MINER])),
        6000,
      );
      // The new block reached the page: its strip links to it.
      f.shown = await page.evaluate((h) => !!document.querySelector(`a[href$="${h}"]`), hash);
      blocks.push(f);
      await context.close();
    }
    const grids = [];
    const big = flag("grid-block", m.blocks["106"]);
    for (let i = 0; i < Math.min(runs, 3); i++) {
      const { context, page } = await newPage(browser, cpu);
      await page.goto(`${base}/regtest/`);
      await page.waitForTimeout(500);
      // An in-app navigation, as from a click, so the window holds the grid and not a page load.
      const f = await frames(
        page,
        () =>
          page.evaluate(
            (u) => history.pushState(null, "", u) || dispatchEvent(new PopStateEvent("popstate")),
            `/regtest/block/${big}`,
          ),
        6000,
      );
      f.shown = await page.evaluate(() => !!document.querySelector(".block-core--grid canvas"));
      grids.push(f);
      await context.close();
    }
    out.frames[profileName] = { new_block_on_home: blocks, grid_load: grids };
    console.error(`pages: frames ${profileName} done`);
  }
}
await browser.close();
console.log(JSON.stringify(out, null, 1));
