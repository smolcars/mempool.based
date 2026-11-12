// The fee facts of the strip's tiles: one batch request on first paint, then the WebSocket's
// `summary` message for each new block (docs/WS.md), with HTTP as the fallback.
import { type Accessor, createEffect, createSignal, on, onCleanup } from "solid-js";
import { blocksTopic } from "../../lib/blocks-topic";
import type { Block } from "../../lib/esplora";
import { useTopic } from "../../lib/live";
import type { Network } from "../../lib/network";
import {
  type BlockSummary,
  fetchSummaries,
  knownSummary,
  rememberSummary,
} from "../../lib/summary";

/** A new block's summary is pushed well under a second after it; fetch it only if it is this late. */
export const PUSH_GRACE_MS = 4_000;

/** A tile's facts: here, not yet, or given up on after the retries. */
export type TileFacts = BlockSummary | "failed" | undefined;

export function createSummaries(
  network: Network,
  blocks: Accessor<Block[] | undefined>,
): (id: string) => TileFacts {
  const [facts, setFacts] = createSignal<ReadonlyMap<string, BlockSummary | "failed">>(new Map());
  const pending = new Set<string>();
  let controller: AbortController | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;

  const put = (entries: [string, BlockSummary | "failed"][]) => {
    if (entries.length > 0) setFacts((m) => new Map([...m, ...entries]));
  };

  /** Fetches the shown blocks' summaries that are neither here nor already asked for. */
  const fetchMissing = () => {
    const wanted = (blocks() ?? [])
      .map((b) => b.id)
      .filter((id) => !pending.has(id) && !knownSummary(network, id));
    if (wanted.length === 0) return;
    for (const id of wanted) pending.add(id);
    controller ??= new AbortController();
    const { signal } = controller;
    fetchSummaries(network, wanted, signal).then(
      (found) => {
        for (const id of wanted) pending.delete(id);
        put(wanted.map((id) => [id, found.get(id) ?? "failed"]));
      },
      () => {
        for (const id of wanted) pending.delete(id);
        // Aborted: the page is gone. Otherwise the tiles keep their neutral gauge.
        if (!signal.aborted) put(wanted.map((id) => [id, "failed"]));
      },
    );
  };

  useTopic(blocksTopic, {
    message: (m) => {
      if (m.type !== "summary") return;
      rememberSummary(network, m.summary);
      if (blocks()?.some((b) => b.id === m.summary.id)) put([[m.summary.id, m.summary]]);
    },
    // Pushes may have been missed while the socket was down.
    resync: () => fetchMissing(),
  });

  let first = true;
  createEffect(
    on(blocks, (list) => {
      if (!list) return;
      clearTimeout(timer);
      // First paint asks at once; a new block waits for its push, then asks for what is missing.
      if (first) fetchMissing();
      else timer = setTimeout(fetchMissing, PUSH_GRACE_MS);
      first = false;
    }),
  );
  onCleanup(() => {
    clearTimeout(timer);
    controller?.abort();
  });
  // A summary known on this page wins over "failed": a push can land while its HTTP fallback fails.
  // Known before the tile was drawn (an earlier visit, or a push that beat its block): shown at once.
  return (id) => {
    const f = facts().get(id);
    return typeof f === "object" ? f : (knownSummary(network, id) ?? f);
  };
}
