import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeSocket } from "./fakes";
import { backoff, LiveConnection, liveUrl, TIMING, type Topic } from "./live";

const echo: Topic<Record<string, unknown>> = {
  name: "blocks",
  parse: (m) => (m["type"] === "block" ? m : undefined),
};
const other: Topic<Record<string, unknown>> = { name: "mempool", parse: (m) => m };

let hidden = false;
let online = true;
let live: LiveConnection;

function connect(random = () => 1) {
  live = new LiveConnection(
    "regtest",
    "ws://test/regtest/app/ws",
    (u) => new FakeSocket(u),
    random,
  );
  return live;
}

beforeEach(() => {
  vi.useFakeTimers();
  FakeSocket.all = [];
  hidden = false;
  online = true;
  Object.defineProperty(document, "hidden", { configurable: true, get: () => hidden });
  Object.defineProperty(navigator, "onLine", { configurable: true, get: () => online });
});
afterEach(() => {
  live?.dispose();
  vi.useRealTimers();
});

describe("liveUrl", () => {
  it("follows the page's scheme and network prefix", () => {
    expect(liveUrl("mainnet", { protocol: "https:", host: "x.example" })).toBe(
      "wss://x.example/app/ws",
    );
    expect(liveUrl("signet", { protocol: "http:", host: "127.0.0.1:9" })).toBe(
      "ws://127.0.0.1:9/signet/app/ws",
    );
  });
});

describe("backoff", () => {
  it("doubles from one second up to the cap, with jitter between half and all of it", () => {
    expect(backoff(0, () => 0)).toBe(500);
    expect(backoff(0, () => 1)).toBe(1000);
    expect(backoff(3, () => 1)).toBe(8000);
    expect(backoff(30, () => 1)).toBe(TIMING.backoffMaxMs);
    expect(backoff(30, () => 0)).toBe(TIMING.backoffMaxMs / 2);
  });
});

describe("LiveConnection", () => {
  it("connects on the first subscriber and subscribes only after the greeting", () => {
    const c = connect();
    expect(FakeSocket.all).toHaveLength(0);
    c.add(echo, { message: () => {}, resync: () => {} });
    const ws = FakeSocket.last();
    expect(ws.url).toBe("ws://test/regtest/app/ws");
    expect(c.status()).toBe("connecting");
    expect(ws.sent).toStrictEqual([]);
    ws.hello();
    expect(c.status()).toBe("live");
    expect(ws.sent).toStrictEqual([{ op: "subscribe", topics: ["blocks"] }]);
  });

  it("stops for good when the greeting is from another protocol version or network", () => {
    const c = connect();
    c.add(echo, { message: () => {}, resync: () => {} });
    FakeSocket.last().hello({ v: 2 });
    expect(c.status()).toBe("offline");
    expect(FakeSocket.last().closedWith).toBe(1000);
    vi.advanceTimersByTime(120_000);
    expect(FakeSocket.all).toHaveLength(1);

    const d = connect();
    d.add(echo, { message: () => {}, resync: () => {} });
    FakeSocket.last().hello({ network: "signet" });
    expect(d.status()).toBe("offline");
  });

  it("delivers a topic's messages to its subscribers only, parsed", () => {
    const c = connect();
    const got: unknown[] = [];
    const others: unknown[] = [];
    c.add(echo, { message: (m) => got.push(m), resync: () => {} });
    c.add(other, { message: (m) => others.push(m), resync: () => {} });
    const ws = FakeSocket.last();
    ws.hello();
    ws.receive({ topic: "blocks", type: "block", n: 1 });
    ws.receive({ topic: "blocks", type: "something-new" });
    expect(got).toStrictEqual([{ topic: "blocks", type: "block", n: 1 }]);
    expect(others).toStrictEqual([]);
  });

  it("ignores binary frames, bad JSON, unknown message types and messages before the greeting", () => {
    const c = connect();
    const got: unknown[] = [];
    c.add(echo, { message: (m) => got.push(m), resync: () => {} });
    const ws = FakeSocket.last();
    ws.receive({ topic: "blocks", type: "block" });
    ws.hello();
    ws.receive(new ArrayBuffer(8));
    ws.receive("not json");
    ws.receive("[1,2]");
    ws.receive({ type: "pong" });
    ws.receive({ type: "notice", text: "new kind of message" });
    expect(got).toStrictEqual([]);
    expect(c.status()).toBe("live");
  });

  it("asks for binary frames as bytes and offers each to the topics that read them", () => {
    const c = connect();
    const bin: Topic<number> = {
      name: "projected-txs:1",
      parse: () => undefined,
      binary: (f) => (new Uint8Array(f)[2] === 1 ? f.byteLength : undefined),
    };
    const got: number[] = [];
    const text: unknown[] = [];
    c.add(bin, { message: (m) => got.push(m), resync: () => {} });
    c.add(echo, { message: (m) => text.push(m), resync: () => {} });
    const ws = FakeSocket.last();
    expect(ws.binaryType).toBe("arraybuffer");
    ws.receive(Uint8Array.of(1, 1, 1, 0, 9).buffer); // before the greeting: ignored
    ws.hello();
    ws.receive(Uint8Array.of(1, 1, 1, 0).buffer);
    ws.receive(Uint8Array.of(1, 1, 2, 0).buffer); // another block's frame
    expect(got).toStrictEqual([4]);
    expect(text).toStrictEqual([]);
  });

  it("restarts a topic by unsubscribing and subscribing again, batched with other changes", () => {
    const c = connect();
    c.add(echo, { message: () => {}, resync: () => {} });
    const ws = FakeSocket.last();
    ws.hello();
    ws.sent.length = 0;
    c.restart("blocks");
    c.restart("blocks");
    c.restart("not-subscribed");
    vi.advanceTimersByTime(TIMING.controlGapMs);
    expect(ws.sent).toStrictEqual([
      { op: "unsubscribe", topics: ["blocks"] },
      { op: "subscribe", topics: ["blocks"] },
    ]);
  });

  it("unsubscribes before it subscribes, so switching capped topics never holds one too many", () => {
    const c = connect();
    c.add({ name: "projected", parse: () => undefined }, { message: () => {}, resync: () => {} });
    const off = c.add(echo, { message: () => {}, resync: () => {} });
    const ws = FakeSocket.last();
    ws.hello();
    ws.sent.length = 0;
    off();
    c.add(other, { message: () => {}, resync: () => {} });
    vi.advanceTimersByTime(TIMING.controlGapMs);
    expect(ws.sent).toStrictEqual([
      { op: "unsubscribe", topics: ["blocks"] },
      { op: "subscribe", topics: ["mempool"] },
    ]);
  });

  it("reconnects with growing backoff, resubscribes, and asks every subscriber to resync", () => {
    const c = connect();
    const resyncs = vi.fn<() => void>();
    c.add(echo, { message: () => {}, resync: resyncs });
    FakeSocket.last().hello();
    expect(resyncs).not.toHaveBeenCalled();

    FakeSocket.last().drop(1006);
    expect(c.status()).toBe("reconnecting");
    vi.advanceTimersByTime(999);
    expect(FakeSocket.all).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(FakeSocket.all).toHaveLength(2);

    // The new socket fails before its greeting: the next wait is twice as long.
    FakeSocket.last().drop(1006);
    vi.advanceTimersByTime(1999);
    expect(FakeSocket.all).toHaveLength(2);
    vi.advanceTimersByTime(1);
    expect(FakeSocket.all).toHaveLength(3);

    const ws = FakeSocket.last();
    ws.hello();
    expect(c.status()).toBe("live");
    expect(ws.sent).toStrictEqual([{ op: "subscribe", topics: ["blocks"] }]);
    expect(resyncs).toHaveBeenCalledOnce();

    // A greeting resets the backoff.
    ws.drop(1001);
    vi.advanceTimersByTime(1000);
    expect(FakeSocket.all).toHaveLength(4);
  });

  it("gives up after a close code that says the client is at fault", () => {
    const c = connect();
    c.add(echo, { message: () => {}, resync: () => {} });
    FakeSocket.last().hello();
    FakeSocket.last().drop(4002);
    expect(c.status()).toBe("offline");
    vi.advanceTimersByTime(120_000);
    expect(FakeSocket.all).toHaveLength(1);
  });

  it("retries after too slow and idle timeout", () => {
    for (const code of [4003, 4004]) {
      FakeSocket.all = [];
      const c = connect();
      c.add(echo, { message: () => {}, resync: () => {} });
      FakeSocket.last().hello();
      FakeSocket.last().drop(code);
      expect(c.status()).toBe("reconnecting");
      vi.advanceTimersByTime(TIMING.backoffBaseMs);
      expect(FakeSocket.all).toHaveLength(2);
      c.dispose();
    }
  });

  it("gives up on a socket that never greets", () => {
    connect().add(echo, { message: () => {}, resync: () => {} });
    vi.advanceTimersByTime(TIMING.helloTimeoutMs);
    expect(FakeSocket.all[0]?.closedWith).toBeUndefined();
    vi.advanceTimersByTime(TIMING.backoffBaseMs);
    expect(FakeSocket.all).toHaveLength(2);
  });

  it("pings, and drops a socket whose pong never comes", () => {
    const c = connect();
    c.add(echo, { message: () => {}, resync: () => {} });
    const ws = FakeSocket.last();
    ws.hello();
    vi.advanceTimersByTime(TIMING.pingEveryMs);
    expect(ws.sent.at(-1)).toStrictEqual({ op: "ping" });
    ws.receive({ type: "pong" });
    vi.advanceTimersByTime(TIMING.pongTimeoutMs);
    expect(c.status()).toBe("live");
    vi.advanceTimersByTime(TIMING.pingEveryMs - TIMING.pongTimeoutMs);
    expect(ws.sent.filter((m) => JSON.stringify(m) === '{"op":"ping"}')).toHaveLength(2);
    vi.advanceTimersByTime(TIMING.pongTimeoutMs);
    expect(c.status()).toBe("reconnecting");
  });

  it("batches subscription changes so navigation never floods the server", () => {
    const c = connect();
    const off = c.add(echo, { message: () => {}, resync: () => {} });
    const ws = FakeSocket.last();
    ws.hello();
    ws.sent.length = 0;
    for (let i = 0; i < 20; i++) {
      const o = c.add(other, { message: () => {}, resync: () => {} });
      o();
    }
    c.add(other, { message: () => {}, resync: () => {} });
    vi.advanceTimersByTime(TIMING.controlGapMs);
    expect(ws.sent).toStrictEqual([{ op: "subscribe", topics: ["mempool"] }]);
    off();
    vi.advanceTimersByTime(TIMING.controlGapMs);
    expect(ws.sent.at(-1)).toStrictEqual({ op: "unsubscribe", topics: ["blocks"] });
  });

  it("closes when the last subscriber leaves, and connects afresh for the next", () => {
    const c = connect();
    const off = c.add(echo, { message: () => {}, resync: () => {} });
    FakeSocket.last().hello();
    off();
    expect(FakeSocket.last().closedWith).toBe(1000);
    const resync = vi.fn<() => void>();
    c.add(echo, { message: () => {}, resync });
    expect(FakeSocket.all).toHaveLength(2);
    FakeSocket.last().hello();
    expect(resync).not.toHaveBeenCalled();
  });

  it("waits for a hidden tab to come back before reconnecting, then reconnects at once", () => {
    const c = connect();
    c.add(echo, { message: () => {}, resync: () => {} });
    FakeSocket.last().hello();
    hidden = true;
    FakeSocket.last().drop(1006);
    vi.advanceTimersByTime(120_000);
    expect(FakeSocket.all).toHaveLength(1);
    expect(c.status()).toBe("reconnecting");
    hidden = false;
    document.dispatchEvent(new Event("visibilitychange"));
    expect(FakeSocket.all).toHaveLength(2);
  });

  it("keeps a live socket open while the tab is hidden", () => {
    const c = connect();
    const got: unknown[] = [];
    c.add(echo, { message: (m) => got.push(m), resync: () => {} });
    FakeSocket.last().hello();
    hidden = true;
    document.dispatchEvent(new Event("visibilitychange"));
    FakeSocket.last().receive({ topic: "blocks", type: "block" });
    expect(got).toHaveLength(1);
    expect(FakeSocket.all).toHaveLength(1);
  });

  it("shows offline while the browser is offline, and tries again when it is back", () => {
    const c = connect();
    c.add(echo, { message: () => {}, resync: () => {} });
    FakeSocket.last().hello();
    online = false;
    window.dispatchEvent(new Event("offline"));
    FakeSocket.last().drop(1006);
    expect(c.status()).toBe("offline");
    vi.advanceTimersByTime(120_000);
    expect(FakeSocket.all).toHaveLength(1);
    online = true;
    window.dispatchEvent(new Event("online"));
    expect(FakeSocket.all).toHaveLength(2);
    FakeSocket.last().hello();
    expect(c.status()).toBe("live");
  });

  it("goes offline with the browser even while its socket still looks open", () => {
    const c = connect();
    c.add(echo, { message: () => {}, resync: () => {} });
    FakeSocket.last().hello();
    expect(c.status()).toBe("live");
    online = false;
    window.dispatchEvent(new Event("offline"));
    expect(c.status()).toBe("offline");
    online = true;
    window.dispatchEvent(new Event("online"));
    expect(FakeSocket.all).toHaveLength(2);
  });

  it("forgets a dropped socket, so a late message from it reaches nobody", () => {
    const c = connect();
    const got: unknown[] = [];
    c.add(echo, { message: (m) => got.push(m), resync: () => {} });
    const old = FakeSocket.last();
    old.hello();
    old.drop(1006);
    old.receive({ topic: "blocks", type: "block" });
    expect(got).toStrictEqual([]);
  });
});
