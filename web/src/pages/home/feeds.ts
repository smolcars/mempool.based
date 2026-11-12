// The home page's data: the block list kept live over the socket, and the panels polled over HTTP.
import { type Accessor, batch, createEffect, createSignal, onCleanup } from "solid-js";
import { fetchApi } from "../../lib/api";
import {
  applyBlock,
  applyReorg,
  type BlocksMessage,
  blocksTopic,
  differsFrom,
  isBlock,
} from "../../lib/blocks-topic";
import type { Block } from "../../lib/esplora";
import { type LiveStatus, useTopic } from "../../lib/live";
import type { Network } from "../../lib/network";

/** Mined blocks kept in the strip: what `/blocks` answers with. */
export const STRIP_MAX = 10;
/** While the socket is down, the strip still moves: `/blocks` is fetched this often. */
export const FALLBACK_POLL_MS = 15_000;
/** Messages held while a fetch is in flight; past this the fetch is simply repeated. */
const QUEUE_MAX = 32;
/**
 * Tip answers are cached for a second (docs/API.md), and the node may serve a new block a moment
 * after announcing it: a list behind what is known is asked for again after this, a few times.
 */
const STALE_RETRY_MS = 1_500;
const STALE_RETRIES = 8;

export interface Feed<T> {
  data: Accessor<T | undefined>;
  error: Accessor<unknown>;
  refresh: () => void;
}

const isAbort = (e: unknown) => e instanceof DOMException && e.name === "AbortError";

/**
 * The mined blocks, newest first. Loaded over HTTP, then kept current from the `blocks` topic.
 * A gap, a `resync` that disagrees, and a reconnect all fetch the list again; while the socket is
 * down it is fetched every {@link FALLBACK_POLL_MS}.
 */
export function createBlockFeed(network: Network, status: Accessor<LiveStatus>): Feed<Block[]> {
  const [data, setData] = createSignal<Block[]>();
  const [error, setError] = createSignal<unknown>();
  let controller: AbortController | undefined;
  /** Messages that arrived while a fetch was in flight, replayed on top of its answer. */
  let queue: BlocksMessage[] | undefined;
  let again = false;
  /** The tip the socket said exists, until a list that reaches it arrives. */
  let expected: { height: number; hash: string } | undefined;
  let staleRetries = 0;
  let staleTimer: ReturnType<typeof setTimeout> | undefined;

  /** A newly announced tip gets its own run of retries. */
  const expect = (tip: { height: number; hash: string }) => {
    expected = tip;
    staleRetries = 0;
  };

  /** Asks again in a moment, unless it has already asked enough times. */
  const retryLater = () => {
    if (staleRetries >= STALE_RETRIES) return false;
    staleRetries++;
    clearTimeout(staleTimer);
    staleTimer = setTimeout(refresh, STALE_RETRY_MS);
    return true;
  };

  const refresh = () => {
    if (controller) {
      again = true;
      return;
    }
    void load();
  };

  async function load() {
    controller = new AbortController();
    const { signal } = controller;
    queue = [];
    again = false;
    try {
      const reply = await fetchApi<unknown>(network, "/blocks", { signal });
      const list = (Array.isArray(reply) ? reply.filter(isBlock) : [])
        .sort((a, b) => b.height - a.height)
        .slice(0, STRIP_MAX);
      const shown = data();
      const newest = list[0];
      const behindShown = !!shown?.[0] && !!newest && newest.height < shown[0].height;
      const behindExpected = !!expected && (!newest || newest.height < expected.height);
      // A list older than what is on screen is never shown; one that is ahead of it but not yet at
      // the announced tip is shown, and asked for again.
      if (!behindShown && !sameList(shown, list)) setData(list);
      if ((behindShown || behindExpected) && retryLater()) return;
      staleRetries = 0;
      expected = undefined;
      // Retries ran out: take the node's word over the screen's.
      if (behindShown) setData(list);
      setError(undefined);
    } catch (e) {
      if (signal.aborted || isAbort(e)) return;
      setError(e);
      if (expected && !retryLater()) {
        // Out of retries: wait for the next announcement, which starts afresh.
        expected = undefined;
        staleRetries = 0;
      }
    } finally {
      if (!signal.aborted) {
        controller = undefined;
        const held = queue ?? [];
        queue = undefined;
        // One change for the strip, so held messages play as one sequence, not several at once.
        batch(() => {
          for (const m of held) handle(m);
        });
        if (again) refresh();
      }
    }
  }

  function handle(m: BlocksMessage) {
    if (queue) {
      if (queue.length < QUEUE_MAX) queue.push(m);
      else again = true;
      return;
    }
    const shown = data();
    if (!shown) {
      refresh();
      return;
    }
    if (m.type === "block") {
      const next = applyBlock(shown, m.block, STRIP_MAX);
      if (next === "gap") {
        expect({ height: m.block.height, hash: m.block.id });
        refresh();
      } else if (!sameList(shown, next)) setData(next);
    } else if (m.type === "reorg") {
      const next = applyReorg(shown, m.forkHeight);
      if (next.length !== shown.length) setData(next);
    } else if (m.tip && differsFrom(shown, m.tip)) {
      // Often the hub's answer to a block the node could not serve yet: wait for HTTP to have it.
      expect(m.tip);
      refresh();
    }
  }

  useTopic(blocksTopic, { message: handle, resync: refresh });
  refresh();

  // The socket is the fast path; HTTP keeps the strip moving while it is down.
  createEffect(() => {
    if (status() === "live") return;
    const timer = setInterval(() => {
      if (!document.hidden) refresh();
    }, FALLBACK_POLL_MS);
    onCleanup(() => clearInterval(timer));
  });
  onCleanup(() => {
    controller?.abort();
    clearTimeout(staleTimer);
  });
  return { data, error, refresh };
}

function sameList(a: readonly Block[] | undefined, b: readonly Block[]): boolean {
  return !!a && a.length === b.length && a.every((x, i) => x.id === b[i]?.id);
}

/**
 * One API path fetched every `everyMs` while the tab is visible, and at once when it comes back.
 * Keeps the last good answer through a failure; `error` says the latest attempt failed.
 */
export function createPoll<T>(
  network: Network,
  path: string,
  everyMs: number,
  valid: (v: unknown) => v is T,
): Feed<T> {
  const [data, setData] = createSignal<T>();
  const [error, setError] = createSignal<unknown>();
  let controller: AbortController | undefined;
  let lastAt = 0;

  const refresh = () => {
    controller?.abort();
    const c = new AbortController();
    controller = c;
    lastAt = Date.now();
    fetchApi<unknown>(network, path, { signal: c.signal }).then(
      (v) => {
        if (c.signal.aborted) return;
        if (valid(v)) {
          setData(() => v);
          setError(undefined);
        } else {
          setError(new Error("unexpected reply"));
        }
      },
      (e: unknown) => {
        if (!c.signal.aborted && !isAbort(e)) setError(e);
      },
    );
  };

  const timer = setInterval(() => {
    if (!document.hidden) refresh();
  }, everyMs);
  const visible = () => {
    if (!document.hidden && Date.now() - lastAt >= everyMs) refresh();
  };
  document.addEventListener("visibilitychange", visible);
  refresh();
  onCleanup(() => {
    clearInterval(timer);
    document.removeEventListener("visibilitychange", visible);
    controller?.abort();
  });
  return { data, error, refresh };
}
