import { createEffect, on, Show } from "solid-js";
import { ApiError } from "../../lib/api";
import type { FeeEstimates } from "../../lib/esplora";
import { useLiveStatus } from "../../lib/live";
import type { Network } from "../../lib/network";
import { useNetwork, usePageTitle } from "../../lib/page";
import { BlockStrip } from "./BlockStrip";
import { FeePanel } from "./FeePanel";
import { createBlockFeed, createPoll } from "./feeds";
import { createLiveMempool } from "./live-mempool";
import { MempoolPanel } from "./MempoolPanel";
import { isFeeEstimates } from "./mempool";
import { RecentTxs } from "./RecentTxs";
import { createSummaries } from "./summaries";
import { DifficultyPanel } from "./DifficultyPanel";
import "./HomePage.css";

/** How often the estimates are asked for. */
const FEES_MS = 30_000;

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
  const facts = createSummaries(props.network, blocks.data);
  const live = createLiveMempool(props.network, status);
  const fees = createPoll<FeeEstimates>(props.network, "/fee-estimates", FEES_MS, isFeeEstimates);

  // A new tip changes the estimates, and the mempool when it is polled: ask at once rather than at
  // the next tick. Only on a change of tip, not when the first list arrives.
  createEffect(
    on(
      () => blocks.data()?.[0]?.id,
      (tip, before) => {
        if (before === undefined || tip === undefined) return;
        fees.refresh();
        if (live.polling.mempool()) live.mempool.refresh();
        if (live.polling.recent()) live.recent.refresh();
      },
    ),
  );

  return (
    <div class="home">
      <BlockStrip
        blocks={blocks.data}
        error={blocks.error}
        retry={blocks.refresh}
        status={status}
        stale={() =>
          live.stale() || [blocks, live.mempool, live.recent].some((f) => nodeSilent(f.error()))
        }
        facts={facts}
        projection={live.projection}
        network={props.network}
      />
      <div class="home-grid wrap">
        <MempoolPanel feed={live.mempool} />
        <FeePanel feed={fees} />
        <RecentTxs feed={live.recent} />
        <DifficultyPanel network={props.network} tip={() => blocks.data()?.[0]?.id} />
      </div>
    </div>
  );
}
