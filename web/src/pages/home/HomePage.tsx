import { createEffect, on, Show } from "solid-js";
import { ApiError } from "../../lib/api";
import type { FeeEstimates, MempoolSummary, RecentTx } from "../../lib/esplora";
import { useLiveStatus } from "../../lib/live";
import type { Network } from "../../lib/network";
import { useNetwork, usePageTitle } from "../../lib/page";
import { BlockStrip } from "./BlockStrip";
import { FeePanel } from "./FeePanel";
import { createBlockFeed, createPoll } from "./feeds";
import { MempoolPanel } from "./MempoolPanel";
import { isFeeEstimates, isMempoolSummary, isRecentList } from "./mempool";
import { RecentTxs } from "./RecentTxs";
import "./HomePage.css";

/** How often each panel asks again. Tip and mempool answers are cached for a second on the server. */
const MEMPOOL_MS = 10_000;
const FEES_MS = 30_000;
const RECENT_MS = 5_000;

/** A failure that means the node, not the request, is the problem. */
const nodeSilent = (e: unknown) => e instanceof ApiError && (e.status === 0 || e.status >= 500);

export default function HomePage() {
  usePageTitle(() => "Blocks and mempool");
  const network = useNetwork();
  // Everything on the page belongs to one network: switching networks starts it afresh.
  return (
    <Show when={network()} keyed>
      {(n) => <Home network={n} />}
    </Show>
  );
}

function Home(props: { network: Network }) {
  const status = useLiveStatus();
  const blocks = createBlockFeed(props.network, status);
  const mempool = createPoll<MempoolSummary>(
    props.network,
    "/mempool",
    MEMPOOL_MS,
    isMempoolSummary,
  );
  const fees = createPoll<FeeEstimates>(props.network, "/fee-estimates", FEES_MS, isFeeEstimates);
  const recent = createPoll<RecentTx[]>(props.network, "/mempool/recent", RECENT_MS, isRecentList);

  // A new tip changes the mempool and the estimates: ask at once rather than at the next tick.
  createEffect(
    on(
      () => blocks.data()?.[0]?.id,
      () => {
        mempool.refresh();
        fees.refresh();
        recent.refresh();
      },
      { defer: true },
    ),
  );

  return (
    <div class="home">
      <BlockStrip
        blocks={blocks.data}
        error={blocks.error}
        retry={blocks.refresh}
        status={status}
        stale={() => [blocks, mempool, recent].some((f) => nodeSilent(f.error()))}
      />
      <div class="home-grid wrap">
        <MempoolPanel feed={mempool} />
        <FeePanel feed={fees} />
        <RecentTxs feed={recent} />
      </div>
    </div>
  );
}
