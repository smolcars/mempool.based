// The mempool over time: its size by fee band and what flowed through it, from the server's
// history (docs/FRONTEND.md "Mempool history", docs/DESIGN.md "Mempool history").
import { useSearchParams } from "@solidjs/router";
import { createMemo, For, lazy, Loading as Pending, onSettled, Show } from "solid-js";
import type { JSX } from "@solidjs/web";
import { formatMoment, formatSi } from "../../components/chart/scale";
import { Link } from "../../components/Link";
import { SectionHeader } from "../../components/SectionHeader";
import { Empty, ErrorState, Loading } from "../../components/States";
import { ApiError, createRequest, fetchApp } from "../../lib/api";
import { FEE_KEY_BOUNDS, formatFeeRate, formatSize, groupDigits } from "../../lib/format";
import { type Network, paths } from "../../lib/network";
import { useNetwork, usePageTitle } from "../../lib/page";
import {
  coverageSentence,
  dataFrom,
  flowSentence,
  type History,
  INTERVALS,
  type Interval,
  isHistory,
  parseInterval,
  pointTime,
  rateText,
  seriesOf,
  sizeSummary,
} from "./history";
import "./MempoolPage.css";

/** The chart loads only on pages that draw one. */
const Chart = lazy(() => import("../../components/chart/Chart"));

/** A new sample lands about a second after each minute turns (docs/FRONTEND.md); ask just after. */
const REFRESH_OFFSET_MS = 3000;

/** The server runs without a data directory, so it keeps no history: no retry changes that. */
export const isNoStore = (e: unknown): boolean =>
  e instanceof ApiError && e.status === 503 && /data directory/i.test(e.message);

/** The ten bands, cheapest first, as the readout shows them beside their colour: `0.2–0.3`, `10+`. */
const BAND_SHORT = FEE_KEY_BOUNDS.map((from, k) => {
  const to = FEE_KEY_BOUNDS[k + 1];
  if (k === 0) return `under ${to ?? ""}`;
  return to === undefined ? `${from}+` : `${from}–${to}`;
});

export default function MempoolPage() {
  usePageTitle(() => "Mempool history");
  const network = useNetwork();
  // Everything on the page belongs to one network: switching networks starts it afresh.
  return (
    <Show when={network()} keyed>
      {(n) => <MempoolHistory network={n} />}
    </Show>
  );
}

function MempoolHistory(props: { network: Network }) {
  const [search] = useSearchParams();
  const interval = createMemo(() => parseInterval(search["interval"]));
  const [request, { refetch }] = createRequest(
    () => interval().id,
    async (id, signal) => {
      const v = await fetchApp<unknown>(props.network, `/mempool/history/${id}`, { signal });
      if (!isHistory(v)) throw new ApiError(200, "The server sent a reply that could not be read.");
      return v;
    },
  );
  // The last answer stays while the next loads, so a new interval never empties the page.
  const answer = () => request.latest;
  const current = () => answer()?.interval === interval().id;

  onSettled(() => {
    void Chart.preload();
    // Live edge: ask again just after each new minute is recorded, so the newest point is never stale.
    let timer: ReturnType<typeof setTimeout> | undefined;
    const schedule = () => {
      const ms = 60_000 - (Date.now() % 60_000) + REFRESH_OFFSET_MS;
      timer = setTimeout(() => {
        // Never cut short an ask still in flight (or retrying a busy server): it would never finish.
        if (request.state !== "errored" && !request.loading) refetch();
        schedule();
      }, ms);
    };
    schedule();
    return () => clearTimeout(timer);
  });

  return (
    <div class="wrap mempool">
      <header class="mempool-head">
        <h1>Mempool history</h1>
        <IntervalPicker
          interval={interval()}
          fitted={
            search["interval"] === undefined &&
            answer() !== undefined &&
            dataFrom(answer() as History) !== undefined
          }
        />
      </header>
      <Show
        when={request.state !== "errored"}
        fallback={<Failure error={request.error} retry={refetch} />}
      >
        <Show
          when={answer()}
          fallback={
            <div class="mempool-slot">
              <Loading label="Loading the mempool's history" />
            </div>
          }
        >
          {(h) => (
            <div class="mempool-busy" aria-busy={current() && !request.loading ? "false" : "true"}>
              <Show when={h().points.length > 0} fallback={<NothingRecorded h={h()} />}>
                <Charts h={h()} interval={interval()} fit={search["interval"] === undefined} />
              </Show>
            </div>
          )}
        </Show>
      </Show>
    </div>
  );
}

/**
 * The periods. A view fitted to what is recorded has its own choice, "Recorded", and is the one
 * marked current: a period's name always means its whole width, never a narrower view.
 */
function IntervalPicker(props: { interval: Interval; fitted: boolean }) {
  return (
    <nav class="mempool-intervals" aria-label="Period">
      <ul>
        <Show when={props.fitted}>
          <li class="mempool-intervals__recorded">
            <Link class="mempool-intervals__item" href={paths.mempool()} aria-current="page">
              Recorded
              <span class="visually-hidden">: since this server began recording</span>
            </Link>
          </li>
        </Show>
        <For each={INTERVALS}>
          {(p) => (
            <li>
              <Link
                class="mempool-intervals__item"
                href={paths.mempool(p.id)}
                aria-current={!props.fitted && p.id === props.interval.id ? "page" : undefined}
              >
                {p.label}
                <span class="visually-hidden">: {p.words}</span>
              </Link>
            </li>
          )}
        </For>
      </ul>
    </nav>
  );
}

function Failure(props: { error: unknown; retry: () => void }) {
  return (
    <Show
      when={!isNoStore(props.error)}
      fallback={
        <Empty title="This server keeps no mempool history">
          It records the mempool once a minute only when it is given somewhere to store it (a data
          directory), and this one was started without one. The live mempool is on the{" "}
          <Link href={paths.home()}>home page</Link>.
        </Empty>
      }
    >
      <ErrorState
        title="Could not load the mempool's history"
        error={props.error}
        retry={props.retry}
      />
    </Show>
  );
}

/** Nothing in the period: say whether anything was ever recorded, and offer all of it. */
function NothingRecorded(props: { h: History }) {
  return (
    <Show
      when={props.h.first !== null}
      fallback={
        <Empty title="Nothing recorded yet">
          This server records the mempool once a minute while it follows it. The first minute
          appears here within two minutes of the server starting, without a reload.
        </Empty>
      }
    >
      <Empty title="Nothing recorded in this period">
        The server was not recording then. Its records begin {pointTime(60)(props.h.first ?? 0)}.{" "}
        <Link href={paths.mempool("all")}>See all of them</Link>
      </Empty>
    </Show>
  );
}

function ChartSlot(props: { class: string; children: JSX.Element }) {
  return (
    <div class={`mempool-chart ${props.class}`}>
      <Pending>{props.children}</Pending>
    </div>
  );
}

/**
 * `fit`: the reader chose no period, so a view whose records begin inside it starts where they do
 * rather than hatching most of its width; the coverage sentence says so. A period the reader chose
 * keeps its whole width, the part before the records hatched.
 */
/** The longest run of missing steps drawn across rather than as a break. */
const BRIDGE_STEPS = 5;

function Charts(props: { h: History; interval: Interval; fit: boolean }) {
  const fitted = () => (props.fit ? dataFrom(props.h) : undefined);
  // A missing minute or few is bridged, not drawn as a stripe that looks like a fault; a longer
  // pause in recording still breaks the line, and the table lists only what was stored.
  const bridged = () => props.h.step * BRIDGE_STEPS;
  const s = createMemo(() => seriesOf(props.h));
  const domain = () => [fitted() ?? props.h.from, props.h.to] as const;
  const hatchFrom = () => (fitted() === undefined ? dataFrom(props.h) : undefined);
  const time = () => pointTime(props.h.step);
  const per = () => props.h.step / 60;
  const notes = () => [
    { name: "Transactions", values: s().count, format: groupDigits },
    {
      name: "Lowest accepted",
      values: s().floor,
      format: (r: number) => `${formatFeeRate(r)} sat/vB`,
    },
    // A point stands for several minutes past two hours: say how many were seen.
    ...(per() > 1
      ? [
          {
            name: "Minutes recorded",
            values: s().samples,
            format: (n: number) => `${n} of ${per()}`,
          },
        ]
      : []),
  ];
  return (
    <>
      <Show when={coverageSentence(props.h, props.interval.words)}>
        {(text) => (
          <p class="mempool-coverage">
            <Show when={hatchFrom() !== undefined}>
              <span class="mempool-coverage__swatch" aria-hidden="true" />
            </Show>
            {/* Fitted to the records, one line says from when; the period's own account is a
                choice away. */}
            {fitted() === undefined
              ? text()
              : `Everything recorded, since ${formatMoment(props.h.first ?? 0)}.`}
          </p>
        )}
      </Show>
      <section class="mempool-section" aria-labelledby="mempool-size-title">
        <SectionHeader title="Waiting, by fee rate" id="mempool-size-title">
          {per() === 1 ? "One point a minute" : `Each point the mean of ${stepWords(props.h.step)}`}
        </SectionHeader>
        <ChartSlot class="mempool-chart--size">
          <Chart
            label={`The mempool's size by fee rate, ${props.interval.words}`}
            x={s().x}
            domain={domain()}
            dataFrom={hatchFrom()}
            gap={bridged()}
            height={300}
            left={{ format: (v) => formatSi(v, "vB"), zero: true }}
            series={[{ name: "Total", values: s().vsize, draw: "line", format: formatSize }]}
            notes={notes()}
            layers={s().bands.map((values, k) => ({
              name: `${BAND_SHORT[k] ?? ""} sat/vB`,
              short: BAND_SHORT[k] ?? "",
              values,
              fee: k + 1,
            }))}
            layerFormat={formatSize}
            time={time()}
            summary={sizeSummary(props.h)}
          />
        </ChartSlot>
      </section>
      <section class="mempool-section" aria-labelledby="mempool-flow-title">
        <SectionHeader title="Flow" id="mempool-flow-title">
          vB a second
        </SectionHeader>
        <Show when={flowSentence(props.h, props.interval.words)}>
          {(text) => <p class="mempool-flow">{text()}</p>}
        </Show>
        <ChartSlot class="mempool-chart--flow">
          <Chart
            label={`What entered and left the mempool, ${props.interval.words}`}
            x={s().x}
            domain={domain()}
            dataFrom={hatchFrom()}
            gap={bridged()}
            height={200}
            left={{ format: (v) => `${formatSi(v, "vB")}/s`, zero: true }}
            series={[{ name: "Arrived", values: s().entered, draw: "line", format: rateText }]}
            layers={[
              { name: "Mined", values: s().mined },
              { name: "Left otherwise", values: s().otherwise },
            ]}
            layerFormat={rateText}
            time={time()}
            summary={flowSentence(props.h, props.interval.words) ?? "No data."}
          />
        </ChartSlot>
        <p class="mempool-aside">
          The line is what arrived; the layers are what blocks mined and what left otherwise
          (replaced, expired or evicted). Line above the layers: the mempool is filling. Below: it
          is draining.
        </p>
      </section>
    </>
  );
}

/** A step in words: `10 minutes`, `an hour`, `a day`, `3 days`. */
function stepWords(step: number): string {
  if (step < 3600) return `${step / 60} minutes`;
  if (step === 3600) return "an hour";
  if (step < 86_400) return `${step / 3600} hours`;
  return step === 86_400 ? "a day" : `${step / 86_400} days`;
}
