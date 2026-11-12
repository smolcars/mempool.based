// The live channel: one WebSocket per tab and network at /app/ws (docs/WS.md), shared by every page.
// Pages use it through `useTopic` and `useLiveStatus`; docs/FRONTEND.md "Live data" shows how.
import { type Accessor, createEffect, createSignal } from "solid-js";
import { type Network, onNetwork } from "./network";
import { useNetwork } from "./page";

/** The protocol version this client speaks; `hello.v` must match. */
export const PROTOCOL_VERSION = 1;

/**
 * - `connecting`: the first connection is being made.
 * - `live`: connected, greeted and subscribed.
 * - `reconnecting`: the connection dropped; another attempt is scheduled.
 * - `offline`: not connected and not trying: the browser is offline, or the server refused us
 *   for good (an old page talking to a newer server, or a close code that says retrying won't help).
 */
export type LiveStatus = "connecting" | "live" | "reconnecting" | "offline";

/** A topic: its name on the wire and how to read its messages. */
export interface Topic<M> {
  readonly name: string;
  /** Reads one of this topic's messages; `undefined` ignores it (an unknown or malformed type). */
  parse: (message: Record<string, unknown>) => M | undefined;
  /**
   * Reads a binary frame, for a topic that sends them. Every binary frame is offered to every such
   * subscriber, since a frame names its topic in its own bytes: return `undefined` for one that is
   * not this topic's, or of a kind this client does not know (docs/WS.md "Binary frames").
   */
  binary?: (frame: ArrayBuffer) => M | undefined;
}

export interface TopicHandlers<M> {
  /** One message, in order. The first one after subscribing is the topic's current state, if any. */
  message: (message: M) => void;
  /** Messages may have been missed (the socket reconnected): fetch the current state over HTTP. */
  resync: () => void;
}

/** The part of the browser's WebSocket the client uses, so tests can hand it a fake. */
export type SocketLike = Pick<
  WebSocket,
  "onopen" | "onmessage" | "onclose" | "onerror" | "send" | "close" | "binaryType"
>;

/** Close codes after which reconnecting cannot help: the client sent something wrong (docs/WS.md). */
const FATAL_CLOSE = new Set([1002, 1003, 1007, 1009, 4000, 4001, 4002]);

export const TIMING = {
  /** Backoff before reconnect attempt n is random in [0.5, 1] × min(MAX, BASE × 2^n). */
  backoffBaseMs: 1000,
  backoffMaxMs: 30_000,
  /** No greeting this long after opening a socket: give up on it. */
  helloTimeoutMs: 10_000,
  /** The server closes a silent socket after 60 s; a ping every 30 s finds a dead one sooner. */
  pingEveryMs: 30_000,
  pongTimeoutMs: 10_000,
  /** Subscription changes are sent at most this often, so navigating never floods the server. */
  controlGapMs: 250,
};

/** The delay before reconnect attempt `attempt` (0-based): exponential, capped, with jitter. */
export function backoff(attempt: number, random: () => number = Math.random): number {
  const ceiling = Math.min(TIMING.backoffMaxMs, TIMING.backoffBaseMs * 2 ** Math.min(attempt, 16));
  return Math.round(ceiling * (0.5 + random() / 2));
}

interface Subscriber {
  topic: Topic<unknown>;
  handlers: TopicHandlers<unknown>;
}

/** One network's socket: connects while anything is subscribed, reconnects, resubscribes. */
export class LiveConnection {
  readonly status: Accessor<LiveStatus>;
  private readonly setStatus: (s: LiveStatus) => void;
  private readonly subscribers = new Set<Subscriber>();
  /** Topics the server has been asked for on the current socket. */
  private subscribed = new Set<string>();
  /** Topics to unsubscribe and subscribe again at the next flush: a binary topic lost its place. */
  private restarting = new Set<string>();
  private ws: SocketLike | undefined;
  private greeted = false;
  /** Whether any socket was ever greeted: every later greeting is a reconnect. */
  private everGreeted = false;
  private attempt = 0;
  private stopped = false;
  private waiting = false;
  private retryTimer: ReturnType<typeof setTimeout> | undefined;
  private helloTimer: ReturnType<typeof setTimeout> | undefined;
  private pingTimer: ReturnType<typeof setInterval> | undefined;
  private pongTimer: ReturnType<typeof setTimeout> | undefined;
  private flushTimer: ReturnType<typeof setTimeout> | undefined;
  private lastControlAt = -Infinity;
  private lastReceivedAt = 0;
  private readonly detach: () => void;

  readonly network: Network;
  private readonly url: string;
  private readonly open: (url: string) => SocketLike;
  private readonly random: () => number;

  constructor(
    network: Network,
    url: string,
    open: (url: string) => SocketLike = (u) => new WebSocket(u),
    random: () => number = Math.random,
  ) {
    this.network = network;
    this.url = url;
    this.open = open;
    this.random = random;
    const [status, setStatus] = createSignal<LiveStatus>("connecting");
    this.status = status;
    this.setStatus = setStatus;
    // A hidden tab keeps its socket, but a dropped one waits for the tab to come back.
    const wake = () => {
      if (!document.hidden && navigator.onLine) this.wakeUp();
    };
    // A socket can stay open long after the network has gone, so the page would call itself live
    // and blame the node for every failed request. Drop it: the reader is offline.
    const offline = () => {
      if (this.ws) this.drop();
      else if (!this.greeted) this.setStatus("offline");
    };
    document.addEventListener("visibilitychange", wake);
    window.addEventListener("online", wake);
    window.addEventListener("offline", offline);
    this.detach = () => {
      document.removeEventListener("visibilitychange", wake);
      window.removeEventListener("online", wake);
      window.removeEventListener("offline", offline);
    };
  }

  /** Subscribes to a topic until the returned function is called. */
  add<M>(topic: Topic<M>, handlers: TopicHandlers<M>): () => void {
    const sub = { topic, handlers } as Subscriber;
    this.subscribers.add(sub);
    if (!this.ws && this.retryTimer === undefined && !this.waiting && !this.stopped) this.connect();
    else this.scheduleFlush();
    return () => {
      this.subscribers.delete(sub);
      if (this.subscribers.size === 0) this.idle();
      else this.scheduleFlush();
    };
  }

  /**
   * Unsubscribes from a topic and subscribes again, so its current state is sent afresh: for a
   * topic of deltas whose client has lost its place. Sent with the next batch of changes.
   */
  restart(name: string): void {
    this.restarting.add(name);
    this.scheduleFlush();
  }

  /** Stops for good and forgets the page's listeners. For tests. */
  dispose(): void {
    this.subscribers.clear();
    this.idle();
    this.detach();
  }

  private connect(): void {
    this.clearTimers();
    this.waiting = false;
    if (document.hidden || !navigator.onLine) {
      this.wait();
      return;
    }
    if (!this.everGreeted) this.setStatus("connecting");
    let ws: SocketLike;
    try {
      ws = this.open(this.url);
    } catch {
      this.drop();
      return;
    }
    // Binary frames as bytes in hand, not Blobs to read asynchronously, so they stay in order.
    ws.binaryType = "arraybuffer";
    this.ws = ws;
    this.greeted = false;
    this.subscribed = new Set();
    this.restarting = new Set();
    // oxlint-disable unicorn/prefer-add-event-listener -- `forget` detaches these by setting them to null
    ws.onmessage = (ev) => this.receive(ev.data);
    ws.onclose = (ev) => this.closed(ev.code);
    ws.onerror = () => {
      // A close always follows, and is handled there.
    };
    // oxlint-enable unicorn/prefer-add-event-listener
    this.helloTimer = setTimeout(() => this.drop(), TIMING.helloTimeoutMs);
  }

  private receive(data: unknown): void {
    this.lastReceivedAt = Date.now();
    if (data instanceof ArrayBuffer) {
      this.receiveBinary(data);
      return;
    }
    if (typeof data !== "string") return;
    let msg: unknown;
    try {
      msg = JSON.parse(data);
    } catch {
      return;
    }
    if (typeof msg !== "object" || msg === null || Array.isArray(msg)) return;
    const m = msg as Record<string, unknown>;
    if (!this.greeted) {
      if (m["type"] === "hello") this.hello(m);
      return;
    }
    if (typeof m["topic"] !== "string") return; // pong, and connection messages added later
    // oxlint-disable-next-line unicorn/no-useless-spread -- a copy: a handler may subscribe while this runs
    for (const sub of [...this.subscribers]) {
      if (sub.topic.name !== m["topic"] || !this.subscribers.has(sub)) continue;
      const parsed = sub.topic.parse(m);
      if (parsed !== undefined) sub.handlers.message(parsed);
    }
  }

  /** A binary frame goes to each topic that reads it; topics without a reader never see one. */
  private receiveBinary(data: ArrayBuffer): void {
    if (!this.greeted) return;
    // oxlint-disable-next-line unicorn/no-useless-spread -- a copy: a handler may subscribe while this runs
    for (const sub of [...this.subscribers]) {
      if (!sub.topic.binary || !this.subscribers.has(sub)) continue;
      const parsed = sub.topic.binary(data);
      if (parsed !== undefined) sub.handlers.message(parsed);
    }
  }

  private hello(m: Record<string, unknown>): void {
    clearTimeout(this.helloTimer);
    if (m["v"] !== PROTOCOL_VERSION || m["network"] !== this.network) {
      // A server speaking another version: this page is out of date, and retrying won't change that.
      this.stop();
      return;
    }
    this.greeted = true;
    this.attempt = 0;
    this.setStatus("live");
    const reconnected = this.everGreeted;
    this.everGreeted = true;
    this.lastControlAt = -Infinity;
    this.flush();
    this.pingTimer = setInterval(() => this.ping(), TIMING.pingEveryMs);
    // oxlint-disable-next-line unicorn/no-useless-spread -- a copy: a handler may subscribe while this runs
    if (reconnected) for (const sub of [...this.subscribers]) sub.handlers.resync();
  }

  private ping(): void {
    const sentAt = Date.now();
    this.send({ op: "ping" });
    clearTimeout(this.pongTimer);
    this.pongTimer = setTimeout(() => {
      // Checked by time, not by timer order: a throttled background timer may fire late.
      if (this.lastReceivedAt < sentAt) this.drop();
    }, TIMING.pongTimeoutMs);
  }

  private send(msg: object): void {
    try {
      this.ws?.send(JSON.stringify(msg));
    } catch {
      // A socket that is closing throws; its close event handles the rest.
    }
  }

  private scheduleFlush(): void {
    if (!this.greeted || this.flushTimer !== undefined) return;
    const wait = Math.max(0, this.lastControlAt + TIMING.controlGapMs - Date.now());
    this.flushTimer = setTimeout(() => {
      this.flushTimer = undefined;
      this.flush();
    }, wait);
  }

  /**
   * Brings the server's subscriptions in line with the page's, in at most two messages.
   * Unsubscribing goes first, so switching between capped topics never holds one too many.
   */
  private flush(): void {
    if (!this.greeted) return;
    const wanted = new Set([...this.subscribers].map((s) => s.topic.name));
    const again = [...this.restarting].filter((t) => wanted.has(t) && this.subscribed.has(t));
    this.restarting.clear();
    const add = [...[...wanted].filter((t) => !this.subscribed.has(t)), ...again];
    const remove = [...[...this.subscribed].filter((t) => !wanted.has(t)), ...again];
    if (remove.length > 0) this.send({ op: "unsubscribe", topics: remove });
    if (add.length > 0) this.send({ op: "subscribe", topics: add });
    if (add.length > 0 || remove.length > 0) this.lastControlAt = Date.now();
    this.subscribed = wanted;
  }

  private closed(code: number): void {
    this.forget();
    if (FATAL_CLOSE.has(code)) this.stop();
    else this.retry();
  }

  /** Gives up on the current socket without waiting for it to close, then retries. */
  private drop(): void {
    const ws = this.ws;
    this.forget();
    try {
      ws?.close();
    } catch {
      // Already closed.
    }
    this.retry();
  }

  private retry(): void {
    if (this.subscribers.size === 0 || this.stopped) return;
    this.clearTimers();
    if (document.hidden || !navigator.onLine) {
      this.wait();
      return;
    }
    this.setStatus("reconnecting");
    this.retryTimer = setTimeout(
      () => {
        this.retryTimer = undefined;
        this.connect();
      },
      backoff(this.attempt++, this.random),
    );
  }

  /** Waits for the tab to be visible and the browser online before trying again. */
  private wait(): void {
    this.waiting = true;
    this.setStatus(
      navigator.onLine ? (this.everGreeted ? "reconnecting" : "connecting") : "offline",
    );
  }

  private wakeUp(): void {
    if (this.subscribers.size === 0 || this.stopped || this.ws) return;
    if (this.waiting || this.retryTimer !== undefined) this.connect();
  }

  private stop(): void {
    const ws = this.ws;
    this.forget();
    try {
      ws?.close(1000);
    } catch {
      // Already closed.
    }
    this.stopped = true;
    this.setStatus("offline");
  }

  /** Nothing is subscribed: close the socket and wait for the next subscriber. */
  private idle(): void {
    const ws = this.ws;
    this.forget();
    this.clearTimers();
    this.waiting = false;
    this.everGreeted = false;
    try {
      ws?.close(1000);
    } catch {
      // Already closed.
    }
    if (!this.stopped) this.setStatus("connecting");
  }

  /** Detaches the current socket so nothing it does later reaches this connection. */
  private forget(): void {
    if (this.ws) {
      // oxlint-disable unicorn/prefer-add-event-listener -- detaching the handlers set in `connect`
      this.ws.onmessage = null;
      this.ws.onclose = null;
      this.ws.onerror = null;
      this.ws.onopen = null;
      // oxlint-enable unicorn/prefer-add-event-listener
    }
    this.ws = undefined;
    this.greeted = false;
    this.clearTimers();
  }

  private clearTimers(): void {
    clearTimeout(this.retryTimer);
    clearTimeout(this.helloTimer);
    clearInterval(this.pingTimer);
    clearTimeout(this.pongTimer);
    clearTimeout(this.flushTimer);
    this.retryTimer = undefined;
    this.flushTimer = undefined;
  }
}

const connections = new Map<Network, LiveConnection>();

/** The socket URL for a network, on the page's own host. */
export function liveUrl(
  network: Network,
  loc: Pick<Location, "protocol" | "host"> = location,
): string {
  return `${loc.protocol === "https:" ? "wss:" : "ws:"}//${loc.host}${onNetwork(network, "")}/app/ws`;
}

/** The tab's connection for a network, made on first use. It connects only while subscribed. */
export function liveConnection(network: Network): LiveConnection {
  let c = connections.get(network);
  if (!c) {
    c = new LiveConnection(network, liveUrl(network));
    connections.set(network, c);
  }
  return c;
}

/**
 * Subscribes the calling component to a topic on the page's network for as long as it is mounted.
 * Returns a function that subscribes again (`LiveConnection.restart`), for a topic of deltas.
 */
export function useTopic<M>(topic: Topic<M>, handlers: TopicHandlers<M>): () => void {
  const network = useNetwork();
  createEffect(network, (n) => liveConnection(n).add(topic, handlers));
  return () => liveConnection(network()).restart(topic.name);
}

/** The state of the page network's live connection. */
export function useLiveStatus(): Accessor<LiveStatus> {
  const network = useNetwork();
  return () => liveConnection(network()).status();
}
