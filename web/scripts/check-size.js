// Fails when the JavaScript a first visit to the home page loads exceeds the budget in
// docs/ARCHITECTURE.md: the scripts index.html loads, the home route's chunk, and every chunk
// those import statically. Other routes load lazily and are listed for information only.
// Also fails when that first visit needs more files than MAX_FILES: on a slow network each one
// waits for one of the browser's six connections (docs/PERFORMANCE.md).
import { readdirSync, readFileSync } from "node:fs";
import { gzipSync } from "node:zlib";

const BUDGET = 100 * 1024;
// The shell, boot.js, the font, the entry's scripts and styles, and the home page's code and styles.
const MAX_FILES = 11;
const HOME_CHUNK = /^HomePage-[\w-]+\.js$/;
const dist = new URL("../dist/", import.meta.url);
const assets = new URL("assets/", dist);

const files = readdirSync(assets).filter((f) => f.endsWith(".js"));
if (files.length === 0) {
  console.error("No JavaScript in dist/assets. Run `bun run build` first.");
  process.exit(1);
}
/** @param {string} f */
const gz = (f) => gzipSync(readFileSync(new URL(f, assets)), { level: 9 }).length;
/** @param {number} n */
const kb = (n) => `${(n / 1024).toFixed(1).padStart(7)} KB`;

const html = readFileSync(new URL("index.html", dist), "utf8");
const entries = [...html.matchAll(/(?:src|href)="\/assets\/([\w-]+\.js)"/g)]
  .map((m) => m[1])
  .filter((f) => f !== undefined);
const home = files.find((f) => HOME_CHUNK.test(f));
if (entries.length === 0 || !home) {
  console.error("Cannot find the entry script in index.html or the HomePage chunk.");
  process.exit(1);
}

// Static imports in Vite's output look like `from"./x-hash.js"` or `import"./x-hash.js"`.
/** @type {Set<string>} */
const firstLoad = new Set();
/** @param {string} f */
const visit = (f) => {
  if (firstLoad.has(f)) return;
  firstLoad.add(f);
  const code = readFileSync(new URL(f, assets), "utf8");
  for (const m of code.matchAll(/(?:\bfrom|\bimport)\s*"\.\/([\w-]+\.js)"/g)) {
    if (m[1] !== undefined) visit(m[1]);
  }
};
for (const f of [...entries, home]) visit(f);

let total = 0;
console.log("First load:");
for (const f of firstLoad) {
  total += gz(f);
  console.log(`${kb(gz(f))}  ${f}`);
}
console.log("Loaded later, by route:");
for (const f of files.filter((name) => !firstLoad.has(name))) console.log(`${kb(gz(f))}  ${f}`);
console.log(`${kb(total)}  first load gzipped, budget ${BUDGET / 1024} KB`);
if (total > BUDGET) {
  console.error("First-load JavaScript is over budget.");
  process.exit(1);
}

// Cascade layers are ordered by where each first appears, so the first stylesheet the shell links
// must open with global.css's order (reset first), or every component would lose to the reset.
const firstStyles = /<link rel="stylesheet"[^>]*href="\/assets\/([\w-]+\.css)"/.exec(html)?.[1];
const firstLayer =
  firstStyles && /@layer\s+(\w+)/.exec(readFileSync(new URL(firstStyles, assets), "utf8"));
if (firstLayer?.[1] !== "reset") {
  console.error(`The first stylesheet, ${firstStyles}, does not open with the reset layer.`);
  process.exit(1);
}

// Every file a cold home page asks for before it shows data: the document, what it links to, and
// the home route's dependencies as the entry's preload list names them (`__vite__mapDeps`).
const linked = [...html.matchAll(/(?:src|href)="\/([\w./-]+)"/g)].flatMap((m) =>
  m[1] === undefined || m[1].endsWith(".svg") ? [] : [m[1]],
);
/** @type {Set<string>} */
const homeFiles = new Set(["index.html", ...linked]);
for (const f of entries) {
  const code = readFileSync(new URL(f, assets), "utf8");
  const list = /__vite__mapDeps=.*?\[(".*?")\]/s.exec(code)?.[1];
  const call = new RegExp(`import\\(\`\\./${home}\`\\),__vite__mapDeps\\(\\[([\\d,]*)\\]`).exec(
    code,
  );
  if (list === undefined || !call?.[1]) continue;
  const deps = [...list.matchAll(/"([^"]*)"/g)].map((m) => m[1]);
  for (const i of call[1].split(",")) homeFiles.add(deps[Number(i)] ?? `missing ${i}`);
}
if (![...homeFiles].some((f) => f.endsWith(home))) {
  console.error("Cannot find the home route's preload list in the entry script.");
  process.exit(1);
}
console.log(
  `${String(homeFiles.size).padStart(7)} files on a cold home page, at most ${MAX_FILES}`,
);
if (homeFiles.size > MAX_FILES) {
  console.error(`The first load needs too many files:\n  ${[...homeFiles].join("\n  ")}`);
  process.exit(1);
}
