// Stand-ins for tests: a WebSocket that lets the test play the server, and made-up blocks.
import type { Block } from "./esplora";
import type { SocketLike } from "./live";

export class FakeSocket implements SocketLike {
  static all: FakeSocket[] = [];
  static last(): FakeSocket {
    const s = FakeSocket.all.at(-1);
    if (!s) throw new Error("no socket was opened");
    return s;
  }

  onopen: SocketLike["onopen"] = null;
  onmessage: SocketLike["onmessage"] = null;
  onclose: SocketLike["onclose"] = null;
  onerror: SocketLike["onerror"] = null;
  readonly sent: unknown[] = [];
  closedWith: number | undefined | null = null;

  readonly url: string;

  constructor(url: string) {
    this.url = url;
    FakeSocket.all.push(this);
  }

  send(data: string): void {
    this.sent.push(JSON.parse(data));
  }
  close(code?: number): void {
    this.closedWith = code;
  }

  /** The server sends a text frame (an object is sent as JSON) or a binary one. */
  receive(data: object | string | ArrayBuffer): void {
    const payload =
      typeof data === "string" || data instanceof ArrayBuffer ? data : JSON.stringify(data);
    this.onmessage?.call(this as unknown as WebSocket, { data: payload } as MessageEvent);
  }
  hello(extra: object = {}): void {
    this.receive({ type: "hello", v: 1, network: "regtest", tip: null, ...extra });
  }
  /** The connection closes, from either side. */
  drop(code = 1006): void {
    this.onclose?.call(this as unknown as WebSocket, { code } as CloseEvent);
  }
}

/** A block at `height` on branch `branch`, its parent the block below on the same branch. */
export function fakeBlock(height: number, branch = "a", parentBranch = branch): Block {
  const id = (h: number, b: string) => `${b}${h.toString(16).padStart(63, "0")}`;
  return {
    id: id(height, branch),
    height,
    previousblockhash: height === 0 ? null : id(height - 1, parentBranch),
    timestamp: 1_800_000_000 + height * 600,
    tx_count: 1,
    size: 250,
    weight: 1000,
    version: 1,
    merkle_root: "0".repeat(64),
    mediantime: 0,
    nonce: 0,
    bits: 0,
    difficulty: 1,
  };
}
