// Records what a cold load of each page asks of the server, for fixtures/cold-load.json, which
// crates/server/tests/cold_load.rs replays against the per-client limits (docs/CONFIG.md).
//
// Needs the server on the regtest scenario chain (dev/regtest/regtest.py scenario) serving the
// frontend build to check. Run from web/, so Playwright resolves from its node_modules:
//   cd web && node ../dev/cold-load/record.mjs http://127.0.0.1:PORT manifest.json COMMIT \
//     > ../fixtures/cold-load.json
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { join } from "node:path";

const { chromium } = createRequire(join(process.cwd(), "package.json"))("playwright");
const [base, manifestPath, commit] = process.argv.slice(2);
const m = JSON.parse(readFileSync(manifestPath, "utf8"));
const pages = {
  home: "/regtest/",
  blocks: "/regtest/blocks",
  block: `/regtest/block/${m.blocks["106"]}`,
  tx: `/regtest/tx/${m.txs.script_types}`,
  address: `/regtest/address/${m.addresses.miner}`,
  mining: "/regtest/mining",
  lightning: "/regtest/lightning",
};
const isApi = (path) => /^(\/[^/]+)?\/(api|app)(\/|$)/.test(path) || path === "/healthz";

const browser = await chromium.launch();
const out = { commit, pages: {} };
for (const [name, path] of Object.entries(pages)) {
  const ctx = await browser.newContext();
  const page = await ctx.newPage();
  const requests = [];
  const inflight = { api: 0, static: 0 };
  const peak = { api: 0, static: 0 };
  let sockets = 0;
  page.on("websocket", () => sockets++);
  page.on("request", (r) => {
    const p = new URL(r.url()).pathname;
    const kind = isApi(p) ? "api" : "static";
    requests.push(p);
    inflight[kind]++;
    peak[kind] = Math.max(peak[kind], inflight[kind]);
  });
  const done = (r) => inflight[isApi(new URL(r.url()).pathname) ? "api" : "static"]--;
  page.on("requestfinished", done);
  page.on("requestfailed", done);
  await page.goto(base + path, { waitUntil: "networkidle" });
  await page.waitForTimeout(2000);
  out.pages[name] = { requests, sockets, peak_api: peak.api, peak_static: peak.static };
  await ctx.close();
}
await browser.close();
console.log(JSON.stringify(out, null, 1));
