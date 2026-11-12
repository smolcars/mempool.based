// Fails when the JavaScript a first visit to the home page loads exceeds the budget in
// docs/ARCHITECTURE.md: the scripts index.html loads, the home route's chunk, and every chunk
// those import statically. Other routes load lazily and are listed for information only.
import { readdirSync, readFileSync } from "node:fs";
import { gzipSync } from "node:zlib";

const BUDGET = 100 * 1024;
const HOME_CHUNK = /^HomePage-[\w-]+\.js$/;
const dist = new URL("../dist/", import.meta.url);
const assets = new URL("assets/", dist);

const files = readdirSync(assets).filter((f) => f.endsWith(".js"));
if (files.length === 0) {
  console.error("No JavaScript in dist/assets. Run `bun run build` first.");
  process.exit(1);
}
const gz = (f) => gzipSync(readFileSync(new URL(f, assets)), { level: 9 }).length;
const kb = (n) => `${(n / 1024).toFixed(1).padStart(7)} KB`;

const html = readFileSync(new URL("index.html", dist), "utf8");
const entries = [...html.matchAll(/(?:src|href)="\/assets\/([\w-]+\.js)"/g)].map((m) => m[1]);
const home = files.find((f) => HOME_CHUNK.test(f));
if (entries.length === 0 || !home) {
  console.error("Cannot find the entry script in index.html or the HomePage chunk.");
  process.exit(1);
}

// Static imports in Vite's output look like `from"./x-hash.js"` or `import"./x-hash.js"`.
const firstLoad = new Set();
const visit = (f) => {
  if (firstLoad.has(f)) return;
  firstLoad.add(f);
  const code = readFileSync(new URL(f, assets), "utf8");
  for (const m of code.matchAll(/(?:\bfrom|\bimport)\s*"\.\/([\w-]+\.js)"/g)) visit(m[1]);
};
for (const f of [...entries, home]) visit(f);

let total = 0;
console.log("First load:");
for (const f of firstLoad) {
  total += gz(f);
  console.log(`${kb(gz(f))}  ${f}`);
}
console.log("Loaded later, by route:");
for (const f of files.filter((f) => !firstLoad.has(f))) console.log(`${kb(gz(f))}  ${f}`);
console.log(`${kb(total)}  first load gzipped, budget ${BUDGET / 1024} KB`);
if (total > BUDGET) {
  console.error("First-load JavaScript is over budget.");
  process.exit(1);
}
