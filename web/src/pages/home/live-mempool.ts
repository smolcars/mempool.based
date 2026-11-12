// The home page's mempool data from the WebSocket's mempool topics (docs/WS.md), falling back to
// polling the public API when the server does not track the mempool, the socket is down, or the
// topics have not said anything yet.
import { type Accessor, createMemo, createSignal, onCleanup } from "solid-js";
import type { MempoolSummary, RecentTx } from "../../lib/esplora";
import { type LiveStatus, useTopic } from "../../lib/live";
import {
  type MempoolState,
  mempoolTopic,
  projectedTopic,
  type SeenTxs,
  transactionsTopic,
} from "../../lib/mempool-topics";
import type { Network } from "../../lib/network";
import { createPoll, type Feed } from "./feeds";
import {
  figuresFromSummary,
  figuresFromTopic,
  isMempoolSummary,
  isRecentList,
  type MempoolFigures,
} from "./mempool";
import { EMPTY_MODEL, type ProjectionModel, receive } from "./projection";

/** How often the fallback asks. Tip and mempool answers are cached for a second on the server. */
const MEMPOOL_MS = 10_000;
const RECENT_MS = 5_000;
/**
 * A server starting up sends nothing until its first mempool cycle (about 10 s on mainnet): poll
 * if the topics are silent this long after the socket is up.
 */
export const TOPIC_GRACE_MS = 5_000;

export interface LiveMempool {
  mempool: Feed<MempoolFigures>;
  recent: Feed<RecentTx[]>;
  projection: Accessor<ProjectionModel>;
  /** The server says it has lost its node: the figures shown are its last good ones. */
  stale: Accessor<boolean>;
  /** Whether the panels are being polled, so a new tip should ask them at once. */
  polling: Accessor<boolean>;
}

export function createLiveMempool(network: Network, status: Accessor<LiveStatus>): LiveMempool {
  const [off, setOff] = createSignal(false);
  const [state, setState] = createSignal<MempoolState>();
  const [seen, setSeen] = createSignal<SeenTxs>();
  const [projection, setProjection] = createSignal<ProjectionModel>(EMPTY_MODEL);
  const [silent, setSilent] = createSignal(false);
  const grace = setTimeout(() => setSilent(true), TOPIC_GRACE_MS);
  onCleanup(() => clearTimeout(grace));

  const heard = () => {
    clearTimeout(grace);
    setSilent(false);
  };
  // A topic is off for every topic at once (one setting per network), so any `off` counts.
  useTopic(mempoolTopic, {
    message: (m) => {
      heard();
      setOff(m.type === "off");
      if (m.type === "mempool") setState(m);
    },
    resync: () => {
      // Each message is the whole state: the snapshot after a reconnect replaces what was missed.
    },
  });
  useTopic(transactionsTopic, {
    message: (m) => {
      heard();
      setOff(m.type === "off");
      if (m.type === "transactions") setSeen(m);
    },
    resync: () => {
      // Each message is the whole state: the snapshot after a reconnect replaces what was missed.
    },
  });
  useTopic(projectedTopic, {
    message: (m) => {
      heard();
      setOff(m.type === "off");
      setProjection((p) => receive(p, m, Date.now()));
    },
    resync: () => {
      // Each message is the whole state: the snapshot after a reconnect replaces what was missed.
    },
  });

  const live = () => status() === "live" && !off();
  const polling = () =>
    off() || status() === "reconnecting" || status() === "offline" || (!state() && silent());
  const polledMempool = createPoll<MempoolSummary>(
    network,
    "/mempool",
    MEMPOOL_MS,
    isMempoolSummary,
    polling,
  );
  const polledRecent = createPoll<RecentTx[]>(
    network,
    "/mempool/recent",
    RECENT_MS,
    isRecentList,
    polling,
  );

  // The socket's state wins while it is live; otherwise the newer of the two.
  const mempool: Feed<MempoolFigures> = {
    data: createMemo(() => {
      const s = state();
      const polled = polledMempool.data();
      if (s && (live() || !polled)) return figuresFromTopic(s);
      return polled ? figuresFromSummary(polled) : undefined;
    }),
    error: () => (live() && state() ? undefined : polledMempool.error()),
    refresh: polledMempool.refresh,
  };
  const recent: Feed<RecentTx[]> = {
    data: () => {
      const s = seen();
      const polled = polledRecent.data();
      return s && (live() || !polled) ? s.txs : polled;
    },
    error: () => (live() && seen() ? undefined : polledRecent.error()),
    refresh: polledRecent.refresh,
  };
  return {
    mempool,
    recent,
    projection,
    stale: () =>
      live() && (state()?.stale === true || projection().entries.at(-1)?.p.stale === true),
    polling,
  };
}
