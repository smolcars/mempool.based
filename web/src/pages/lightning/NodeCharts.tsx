// A node's past (its channels and capacity since this server's record began, stepping at each
// open and close) and how it prices its channels, out and in (docs/DESIGN.md "Lightning").
import { createSignal, For, lazy, Loading as Pending, onSettled, Show } from "solid-js";
import { Amount } from "../../components/Amount";
import { formatDay, formatMoment } from "../../components/chart/scale";
import "../../components/chart/Chart.css";
import { type Column, DataTable } from "../../components/DataTable";
import { More } from "../../components/More";
import { SectionHeader } from "../../components/SectionHeader";
import { Empty, ErrorState, Loading } from "../../components/States";
import { createRequest } from "../../lib/api";
import { formatBtc, groupDigits } from "../../lib/format";
import type { Network } from "../../lib/network";
import {
  btcAxis,
  count,
  feeRows,
  feeSentence,
  type FeeRow,
  isNodeHistory,
  type NodeDetail,
} from "./lightning";
import { ask } from "./parts";

const Chart = lazy(() => import("../../components/chart/Chart"));

const DAY = 86_400;

export function NodeHistory(props: { network: Network; id: string }) {
  const [history, { refetch }] = createRequest(
    () => props.id,
    (id, signal) => ask(props.network, `/nodes/${id}/history`, isNodeHistory, signal),
  );
  onSettled(() => void Chart.preload());
  const points = () => history.latest?.points ?? [];
  // From zero, so a node's size reads at a glance; whole numbers for a node with a few channels.
  const channelAxis = () =>
    Math.max(...points().map((p) => p.channels)) <= 4
      ? { domain: [0, 4] as const, format: groupDigits }
      : { zero: true, format: groupDigits };
  const col = (f: (p: { channels: number; capacity: number }) => number) =>
    points().map((p) => f(p));
  return (
    <section aria-labelledby="ln-node-history-title">
      <SectionHeader title="Over time" id="ln-node-history-title">
        Each change and midnight UTC
      </SectionHeader>
      <Show
        when={history.state !== "errored"}
        fallback={
          <ErrorState title="Could not load its history" error={history.error} retry={refetch} />
        }
      >
        <Show
          when={history.latest}
          fallback={
            <div class="ln-chart ln-chart--node">
              <Loading label="Loading its history" />
            </div>
          }
        >
          {(h) => (
            <Show
              when={h().from !== null && points().length > 1}
              fallback={
                <Empty title="Nothing to draw yet">
                  {h().from === null
                    ? "This server's history starts when its Lightning peer first syncs and stores the network's size. A node's past is drawn from then on, a point each midnight UTC."
                    : `This server's record begins ${formatMoment(h().from ?? 0)}. A node's past is drawn from the first midnight UTC after that.`}
                </Empty>
              }
            >
              {/* Our record of closes starts with the server: before it, a zero would be a guess. */}
              <p class="ln-coverage">Recorded since {formatMoment(h().from ?? 0)}.</p>
              <div class="ln-chart ln-chart--node">
                <Pending>
                  <Chart
                    label="This node's channels and capacity, since this server's record began"
                    x={points().map((p) => p.time)}
                    gap={DAY}
                    height={200}
                    left={channelAxis()}
                    right={{ zero: true, format: btcAxis }}
                    // Steps: a channel opens or closes in a block, at a moment the record has, and
                    // the count holds until the next change. A slope would invent the time between.
                    series={[
                      {
                        name: "Channels",
                        values: col((p) => p.channels),
                        draw: "step-area",
                        format: groupDigits,
                      },
                      {
                        name: "Capacity",
                        values: col((p) => p.capacity),
                        draw: "step",
                        axis: "right",
                        tone: 2,
                        format: formatBtc,
                        unit: "BTC",
                      },
                    ]}
                    // Midnights are days; a change and the last point (now) are moments.
                    time={(t) => (t % DAY === 0 ? formatDay(t) : formatMoment(t))}
                  />
                </Pending>
              </div>
            </Show>
          )}
        </Show>
      </Show>
    </section>
  );
}

export function NodeFees(props: { node: NodeDetail }) {
  const rows = () => feeRows(props.node.fees);
  const [figures, setFigures] = createSignal(false);
  // One scale for both ways, so a bar out and a bar in compare by length.
  const most = () => Math.max(1, ...rows().flatMap((r) => [r.outgoing.count, r.incoming.count]));
  // One table each way, under the chart's own words: three columns fit a 320 px phone whole, where
  // five scrolled, and the capacities are only here.
  const columns = (way: "outgoing" | "incoming"): Column<FeeRow>[] => [
    { header: "Fee rate, ppm", cell: (r) => r.label },
    { header: "Channels", numeric: true, cell: (r) => groupDigits(r[way].count) },
    {
      header: "Their capacity, BTC",
      short: "Capacity, BTC",
      numeric: true,
      cell: (r) => <Amount sats={r[way].capacity} unit={false} short />,
    },
  ];
  return (
    <section aria-labelledby="ln-node-fees-title">
      <SectionHeader title="Fee rates" id="ln-node-fees-title">
        {count(props.node.channels.open, "open channel")}
      </SectionHeader>
      <Show
        when={rows().length > 0}
        fallback={<Empty title="No fee to show">It has no open channel this server knows.</Empty>}
      >
        <p class="ln-fees__answer">{feeSentence(rows())}</p>
        <figure class="ln-fees">
          {/* The table under "Show the figures" says the same to a screen reader. */}
          <div class="ln-fees__plot" aria-hidden="true">
            <span class="ln-fees__unit">ppm</span>
            {/* The key heads the bars, in their order: stacked on a phone, side by side if wide. */}
            <span class="ln-fees__bars ln-fees__key">
              <span class="ln-fees__way ln-fees__way--out">It charges</span>
              <span class="ln-fees__way ln-fees__way--in">Its peers charge</span>
            </span>
            <For each={rows()}>
              {(r) => (
                <>
                  <span class="ln-fees__label num">{r.label}</span>
                  <span class="ln-fees__bars">
                    <span class="ln-fees__bar ln-fees__bar--out">
                      <Count n={r.outgoing.count} most={most()} />
                    </span>
                    <span class="ln-fees__bar ln-fees__bar--in">
                      <Count n={r.incoming.count} most={most()} />
                    </span>
                  </span>
                </>
              )}
            </For>
          </div>
          <figcaption class="chart__caption">
            <details class="chart__figures" onToggle={(e) => setFigures(e.currentTarget.open)}>
              <summary>Show the figures</summary>
              <Show when={figures()}>
                <div
                  class="chart__table ln-fees__figures"
                  // oxlint-disable-next-line jsx-a11y/no-noninteractive-tabindex -- a scrolling region must take focus to be scrolled by keyboard
                  tabindex="0"
                  role="region"
                  aria-label="Its channels by fee rate, figures"
                >
                  <div class="ln-fees__table ln-fees__table--out">
                    <DataTable
                      caption="It charges"
                      showCaption
                      columns={columns("outgoing")}
                      rows={rows()}
                    />
                  </div>
                  <div class="ln-fees__table ln-fees__table--in">
                    <DataTable
                      caption="Its peers charge"
                      showCaption
                      columns={columns("incoming")}
                      rows={rows()}
                    />
                  </div>
                </div>
              </Show>
            </details>
          </figcaption>
        </figure>
        <More>
          <p>
            A channel has a fee rate each way: what this node charges to forward a payment out over
            it, and what the peer at the other end charges to send one towards it. A rate is in
            parts per million of the amount forwarded: 1 000 ppm is 0.1%. Each bar counts channels;
            the figures give their capacity too. "No policy" counts the ends that have announced no
            fee, or whose announcement is over two weeks old. The fixed fee per payment (base fee)
            is not included.
          </p>
        </More>
      </Show>
    </section>
  );
}

/** One bar and its count; nothing for none, so a row shows only what it holds. */
function Count(props: { n: number; most: number }) {
  return (
    <Show when={props.n > 0}>
      <span class="ln-fees__fill" style={{ "--w": String(props.n / props.most) }} />
      <span class="ln-fees__n num">{groupDigits(props.n)}</span>
    </Show>
  );
}
