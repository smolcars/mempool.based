// Fails when the built JavaScript exceeds the first-load budget in docs/ARCHITECTURE.md.
// shortcut: sums every chunk in dist/assets; count only the entry's import graph once routes are lazy-loaded.
import { readdirSync, readFileSync } from "node:fs";
import { gzipSync } from "node:zlib";

const BUDGET = 100 * 1024;
const dir = new URL("../dist/assets/", import.meta.url);
const files = readdirSync(dir).filter((f) => f.endsWith(".js"));
if (files.length === 0) {
  console.error("No JavaScript in dist/assets. Run `bun run build` first.");
  process.exit(1);
}
let total = 0;
for (const f of files) {
  const size = gzipSync(readFileSync(new URL(f, dir)), { level: 9 }).length;
  total += size;
  console.log(`${(size / 1024).toFixed(1).padStart(7)} KB  ${f}`);
}
console.log(
  `${(total / 1024).toFixed(1).padStart(7)} KB  total gzipped, budget ${BUDGET / 1024} KB`,
);
if (total > BUDGET) {
  console.error("JavaScript is over budget.");
  process.exit(1);
}
