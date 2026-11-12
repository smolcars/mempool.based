// Drives the mempool.space-shaped socket (/regtest/api/v1/ws, docs/WS.md) against the server on
// the regtest scenario chain (dev/regtest/regtest.py scenario): follows an address and a
// transaction, broadcasts, replaces and mines on regtest, and checks each expected message
// arrives. Prints every message received, in order. Writes happen on the regtest node only.
//   bun dev/regtest/ws-v1.mjs 127.0.0.1:PORT manifest.json
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";

const [addr, manifestPath] = process.argv.slice(2);
const m = JSON.parse(readFileSync(manifestPath, "utf8"));
const payee = m.addresses.p2wpkh;
const t0 = Date.now();
const regtest = (...args) =>
  execFileSync("python3", ["dev/regtest/regtest.py", ...args], { encoding: "utf8" }).trim();

function open(name) {
  const ws = new WebSocket(`ws://${addr}/regtest/api/v1/ws`);
  const seen = [];
  const waiters = [];
  ws.onmessage = (e) => {
    const v = JSON.parse(String(e.data));
    console.log(`${Date.now() - t0}ms ${name} << ${Object.keys(v)} ${String(e.data).slice(0, 300)}`);
    seen.push(v);
    for (const w of waiters.splice(0)) w();
  };
  const ready = new Promise((ok, fail) => {
    ws.onopen = ok;
    ws.onerror = fail;
  });
  return {
    send(msg) {
      console.log(`${Date.now() - t0}ms ${name} >> ${JSON.stringify(msg)}`);
      ws.send(JSON.stringify(msg));
    },
    // The first message since `from` matching `f`, within 10 s.
    async wait(what, f, from = 0) {
      const deadline = Date.now() + 10_000;
      for (;;) {
        const i = seen.findIndex((v, i) => i >= from && f(v));
        if (i >= 0) return i;
        if (Date.now() > deadline) throw new Error(`${name}: no ${what}`);
        await new Promise((ok) => {
          waiters.push(ok);
          setTimeout(ok, 200);
        });
      }
    },
    seen,
    ready,
    close: () => ws.close(),
  };
}

// Three blocks first: their coinbases mature three more coins, so the script can run again.
const tip = regtest("mine", "3").split("\n").at(-1);
await new Promise((ok) => setTimeout(ok, 500));
const a = open("A");
const b = open("B");
await Promise.all([a.ready, b.ready]);
a.send({ action: "ping" });
await a.wait("pong", (v) => v.pong === true);
a.send({ action: "want", data: ["blocks", "stats", "mempool-blocks"] });
await a.wait("blocks", (v) => v.blocks?.at(-1)?.id === tip && v.blocks.length === 8);
// `fees` joins once a recommendation can be served: not in the first minutes on regtest.
await a.wait("stats", (v) => v.mempoolInfo && v.da);
await a.wait("mempool-blocks", (v) => Array.isArray(v["mempool-blocks"]));
a.send({ "track-address": payee });
b.send({ "track-addresses": [payee, m.addresses.p2tr] });
// Give the server its one read of the address's history, which says nothing.
await new Promise((ok) => setTimeout(ok, 500));

// A payment to the address enters the mempool.
const paid = regtest("send", payee, "100000");
a.send({ "track-tx": paid });
await a.wait("txPosition", (v) => v.txPosition?.txid === paid);
await a.wait("address-transactions", (v) => v["address-transactions"]?.some((t) => t.txid === paid));
await b.wait("multi-address-transactions", (v) =>
  v["multi-address-transactions"]?.[payee]?.mempool.some((t) => t.txid === paid),
);

// A replacement, seen by the server's mempool poll before it happens: following the replaced
// transaction then says what replaced it.
const { rbf_original: original, rbf_replacement: replacement } = JSON.parse(regtest("rbf", "--gap", "3"));
a.send({ "track-tx": original });
await a.wait("txReplaced", (v) => v.txReplaced?.txid === replacement);

// The payment confirms.
a.send({ "track-tx": paid });
await a.wait("txPosition again", (v) => v.txPosition?.txid === paid, a.seen.length);
const from = a.seen.length;
const mined = regtest("mine", "1");
const block = await a.wait("block", (v) => v.block?.id === mined, from);
const confirmed = await a.wait("txConfirmed", (v) => v.txConfirmed === paid, from);
await a.wait("block-transactions", (v) => v["block-transactions"]?.some((t) => t.txid === paid), from);
await b.wait("multi-address confirmed", (v) =>
  v["multi-address-transactions"]?.[payee]?.confirmed.some((t) => t.txid === paid),
);
const blocksAfter = await a.wait("mempool-blocks after the block", (v) => v["mempool-blocks"], block + 1);
console.log(
  JSON.stringify({ ok: true, paid, original, replacement, mined, order: { block, confirmed, blocksAfter } }),
);
a.close();
b.close();
process.exit(0);
