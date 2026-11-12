// The Lightning network now and over time, and its newest opens and closes
// (docs/DESIGN.md "Lightning", docs/FRONTEND.md "Lightning pages").
import { useSearchParams } from "@solidjs/router";
import { createMemo, For, lazy, Loading as Pending, onSettled, Show } from "solid-js";
import { Amount } from "../../components/Amount";
import { formatMoment, formatSi } from "../../components/chart/scale";
import { type Column, DataTable } from "../../components/DataTable";
import { Link } from "../../components/Link";
import { SectionHeader } from "../../components/SectionHeader";
import { Empty, ErrorState, Loading } from "../../components/States";
import { TimeAgo } from "../../components/Time";
import { createRequest } from "../../lib/api";
import { formatBtc, groupDigits } from "../../lib/format";
import { type Network, paths } from "../../lib/network";
import { useNetwork, usePageTitle } from "../../lib/page";
import {
  coverageSentence,
  dataFrom,
  INTERVALS,
  type Interval,
  isOverview,
  isSeries,
  KIND_SHORT,
  type ListedChannel,
  msat,
  parseInterval,
  ppm,
  type RankedNode,
  type Series,
  type Stats,
} from "./lightning";
import {
  ask,
  BlockLink,
  ChannelLink,
  Ends,
  Gate,
  LightningSearch,
  NodeName,
  Stat,
  VERIFIED_NOTE,
} from "./parts";

const Chart = lazy(() => import("../../components/chart/Chart"));

export default function LightningPage() {
  usePageTitle(() => "Lightning");
  const network = useNetwork();
  return (
    <div class="wrap ln">
      <header class="ln-head">
        <h1>Lightning</h1>
        <LightningSearch />
      </header>
      {/* Everything belongs to one network: switching networks starts it afresh. */}
      <Show when={network()} keyed>
        {(n) => (
          <Gate>
            <Overview network={n} />
          </Gate>
        )}
      </Show>
    </div>
  );
}

function Overview(props: { network: Network }) {
  const [overview, { refetch }] = createRequest(
    () => props.network,
    (n, signal) => ask(n, "/overview", isOverview, signal),
  );
  onSettled(() => void Chart.preload());
  return (
    <>
      <Show
        when={overview.state !== "errored"}
        fallback={
          <ErrorState
            title="Could not load the Lightning network's figures"
            error={overview.error}
            retry={refetch}
          />
        }
      >
        <Show when={overview.latest} fallback={<Loading label="Loading the Lightning network" />}>
          {(o) => <Now stats={o().statistics} />}
        </Show>
      </Show>
      <History network={props.network} />
      <Show when={overview.latest}>
        {(o) => (
          <>
            <div class="ln-pair">
              <section aria-labelledby="ln-opened-title">
                <SectionHeader title="Newest channels" id="ln-opened-title">
                  By funding block
                </SectionHeader>
                <ChannelTable rows={o().opened} kind="opened" />
              </section>
              <section aria-labelledby="ln-closed-title">
                <SectionHeader title="Newest closes" id="ln-closed-title">
                  Seen on the chain
                </SectionHeader>
                <ChannelTable rows={o().closed} kind="closed" />
              </section>
            </div>
            <div class="ln-pair">
              <section aria-labelledby="ln-top-capacity-title">
                <SectionHeader title="Largest nodes" id="ln-top-capacity-title">
                  By capacity
                </SectionHeader>
                <TopTable rows={o().top_capacity} caption="The ten nodes with the most capacity" />
                <p class="ln-more">
                  <Link href={paths.lightningRankings("capacity")}>The top 100 by capacity</Link>
                </p>
              </section>
              <section aria-labelledby="ln-top-channels-title">
                <SectionHeader title="Most connected nodes" id="ln-top-channels-title">
                  By open channels
                </SectionHeader>
                <TopTable rows={o().top_channels} caption="The ten nodes with the most channels" />
                <p class="ln-more">
                  <Link href={paths.lightningRankings("channels")}>The top 100 by channels</Link>
                  {" · "}
                  <Link href={paths.lightningRankings("age")}>The oldest nodes</Link>
                </p>
              </section>
            </div>
          </>
        )}
      </Show>
    </>
  );
}

/** The newest snapshot's figures, or why there is none yet. */
function Now(props: { stats: Stats | null }) {
  return (
    <section aria-labelledby="ln-now-title">
      <SectionHeader title="The network now" id="ln-now-title">
        <Show when={props.stats}>{(s) => <>As of {formatMoment(s().time)}</>}</Show>
      </SectionHeader>
      <Show
        when={props.stats}
        fallback={
          <Empty title="No snapshot yet">
            This server stores the network's size every ten minutes once its channel graph is
            synced. The first appears here within ten minutes.
          </Empty>
        }
      >
        {(s) => (
          <>
            <dl class="ln-stats">
              <Stat
                label="Channels"
                sub={
                  <>
                    Median size <Amount sats={s().med_capacity} />
                  </>
                }
              >
                <span class="num">{groupDigits(s().channels)}</span>
              </Stat>
              <Stat label="Nodes" sub={nodeKinds(s())}>
                <span class="num">{groupDigits(s().nodes)}</span>
              </Stat>
              <Stat
                label="Capacity"
                sub={
                  <>
                    Average per channel <Amount sats={s().avg_capacity} />
                  </>
                }
              >
                <Amount sats={s().capacity} />
              </Stat>
              <Stat label="Median fee rate" sub={`Average ${ppm(s().avg_fee_rate)}`}>
                <span class="num">{ppm(s().med_fee_rate)}</span>
              </Stat>
              <Stat label="Median base fee" sub={`Average ${msat(s().avg_base_fee_msat)}`}>
                <span class="num">{msat(s().med_base_fee_msat)}</span>
              </Stat>
            </dl>
            <p class="ln-aside">
              {VERIFIED_NOTE} Fee figures are what nodes charge to forward a payment, over{" "}
              {groupDigits(s().policies)} policies; averages leave out the few over 1 000 000, which
              refuse to route.
            </p>
          </>
        )}
      </Show>
    </section>
  );
}

const nodeKinds = (s: Stats): string =>
  `${groupDigits(s.clearnet_nodes)} clearnet, ${groupDigits(s.tor_nodes)} Tor, ${groupDigits(s.clearnet_tor_nodes)} both, ${groupDigits(s.unannounced_nodes)} with no address`;

/** Whole BTC on an axis; three significant digits below 100. */
const btcAxis = (sats: number): string => {
  const btc = sats / 1e8;
  return `${btc >= 100 ? groupDigits(btc) : formatSi(btc, "")} BTC`;
};

function History(props: { network: Network }) {
  const [search] = useSearchParams();
  const interval = createMemo(() => parseInterval(search["interval"]));
  const [series, { refetch }] = createRequest(
    () => interval().id,
    (id, signal) => ask(props.network, `/statistics/${id}`, isSeries, signal),
  );
  const answer = () => series.latest;
  return (
    <section aria-labelledby="ln-history-title">
      <SectionHeader title="Over time" id="ln-history-title">
        Every ten minutes
      </SectionHeader>
      <div class="ln-section-head">
        <nav class="ln-picker" aria-label="Period">
          <ul>
            <For each={INTERVALS}>
              {(p) => (
                <li>
                  <Link
                    class="ln-picker__item"
                    href={paths.lightning(p.id)}
                    aria-current={p.id === interval().id ? "page" : undefined}
                  >
                    {p.label}
                    <span class="visually-hidden">: {p.words}</span>
                  </Link>
                </li>
              )}
            </For>
          </ul>
        </nav>
      </div>
      <Show
        when={series.state !== "errored"}
        fallback={
          <ErrorState
            title="Could not load the network's history"
            error={series.error}
            retry={refetch}
          />
        }
      >
        <Show
          when={answer()}
          fallback={
            <div class="ln-chart">
              <Loading label="Loading the network's history" />
            </div>
          }
        >
          {(s) => (
            <div
              class="ln-busy"
              aria-busy={s().interval === interval().id && !series.loading ? "false" : "true"}
            >
              <Show when={s().points.length > 0} fallback={<NothingRecorded s={s()} />}>
                <HistoryChart s={s()} interval={interval()} />
              </Show>
            </div>
          )}
        </Show>
      </Show>
    </section>
  );
}

function NothingRecorded(props: { s: Series }) {
  return (
    <Show
      when={props.s.first !== null}
      fallback={
        <Empty title="Nothing recorded yet">
          This server's history of the network starts when its Lightning peer first synced: it
          stores the network's size every ten minutes from then on. The first point appears within
          ten minutes.
        </Empty>
      }
    >
      <Empty title="Nothing recorded in this period">
        This server's records begin {formatMoment(props.s.first ?? 0)}.{" "}
        <Link href={paths.lightning("all")}>See all of them</Link>
      </Empty>
    </Show>
  );
}

function HistoryChart(props: { s: Series; interval: Interval }) {
  const x = () => props.s.points.map((p) => p.time);
  const col = (f: (p: Stats) => number) => props.s.points.map(f);
  const time = () =>
    props.s.step < 86_400 ? formatMoment : (t: number) => formatMoment(t).replace(/, .*$/, "");
  return (
    <>
      <Show when={coverageSentence(props.s, props.interval.words)}>
        {(text) => (
          <p class="ln-coverage">
            <Show when={dataFrom(props.s) !== undefined}>
              <span class="ln-coverage__swatch" aria-hidden="true" />
            </Show>
            {text()}
          </p>
        )}
      </Show>
      <div class="ln-chart">
        <Pending>
          <Chart
            label={`Lightning channels and capacity, ${props.interval.words}`}
            x={x()}
            domain={[props.s.from, props.s.to]}
            dataFrom={dataFrom(props.s)}
            gap={props.s.step}
            height={240}
            left={{ format: (v) => groupDigits(v) }}
            right={{ format: btcAxis }}
            series={[
              {
                name: "Channels",
                values: col((p) => p.channels),
                draw: "area",
                format: groupDigits,
              },
              {
                name: "Capacity",
                values: col((p) => p.capacity),
                draw: "line",
                axis: "right",
                tone: 2,
                format: (v) => `${formatBtc(v)} BTC`,
              },
            ]}
            notes={[
              { name: "Nodes", values: col((p) => p.nodes), format: groupDigits },
              { name: "Median fee rate", values: col((p) => p.med_fee_rate), format: ppm },
            ]}
            time={time()}
          />
        </Pending>
      </div>
    </>
  );
}

function ChannelTable(props: { rows: ListedChannel[]; kind: "opened" | "closed" }) {
  const columns = (): Column<ListedChannel>[] => [
    {
      header: "Channel",
      cell: (c) => (
        <span class="ln-ends">
          <ChannelLink id={c.id} />
          <Show when={props.kind === "closed" ? c.closed?.block : c.opened}>
            {(b) => <TimeAgo time={b().time} />}
          </Show>
        </span>
      ),
    },
    ...(props.kind === "closed"
      ? [
          {
            header: "Kind",
            cell: (c: ListedChannel) => (c.closed ? KIND_SHORT[c.closed.kind] : "–"),
          },
        ]
      : []),
    { header: "Between", cell: (c) => <Ends one={c.node_one} two={c.node_two} /> },
    {
      header: "Capacity, BTC",
      numeric: true,
      cell: (c) => <Amount sats={c.capacity} unit={false} />,
    },
    {
      header: "Block",
      numeric: true,
      optional: true,
      cell: (c) => (
        <Show when={props.kind === "closed" ? c.closed?.block : c.opened} fallback="–">
          {(b) => <BlockLink block={b()} />}
        </Show>
      ),
    },
  ];
  return (
    <DataTable
      caption={props.kind === "opened" ? "The newest channels" : "The newest closes"}
      columns={columns()}
      rows={props.rows}
      empty={
        props.kind === "opened" ? (
          <Empty title="No channel yet" />
        ) : (
          <Empty title="No close seen yet">
            This server records every close it sees on the chain from when its Lightning peer
            started.
          </Empty>
        )
      }
    />
  );
}

function TopTable(props: { rows: RankedNode[]; caption: string }) {
  const columns: Column<RankedNode>[] = [
    { header: "Node", cell: (n) => <NodeName node={n} /> },
    {
      header: "Capacity, BTC",
      numeric: true,
      cell: (n) => <Amount sats={n.capacity} unit={false} />,
    },
    { header: "Channels", numeric: true, cell: (n) => groupDigits(n.channels) },
  ];
  return <DataTable caption={props.caption} columns={columns} rows={props.rows} />;
}
