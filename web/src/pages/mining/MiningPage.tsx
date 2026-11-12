import { useSearchParams } from "@solidjs/router";
import { createMemo, createSignal, For, onMount, Show } from "solid-js";
import { Amount } from "../../components/Amount";
import { formatDay, formatHashrate, formatMoment, formatShare } from "../../components/chart/scale";
import { DataTable } from "../../components/DataTable";
import { DifficultyAdjustment } from "../../components/Difficulty";
import { Link } from "../../components/Link";
import { SectionHeader } from "../../components/SectionHeader";
import { groupDigits, MISSING } from "../../lib/format";
import { type Network, paths } from "../../lib/network";
import { useNetwork, usePageTitle } from "../../lib/page";
import { createMiningData, type MiningData } from "./data";
import {
  competitive,
  hashrateInterval,
  hashrateSeries,
  type Hashrates,
  isHashrates,
  isPoints,
  isPoolWeeks,
  isRanking,
  isRatePoints,
  isRewardStats,
  isSizes,
  networkHashrate,
  networkNote,
  periodDomain,
  PERIODS,
  type Period,
  parsePeriod,
  poolLayers,
  type Ranked,
  type Ranking,
  REWARD_MAX,
  rewardFigures,
  sharesOverTime,
} from "./mining";
import {
  btcFull,
  btcShort,
  bytesText,
  Chart,
  ChartSlot,
  chartHeight,
  CoverageNote,
  difficultyFull,
  difficultyShort,
  feeRateText,
  NothingStored,
  percent,
  Section,
  Stat,
} from "./parts";
import { RecentBlocks } from "./RecentBlocks";
import "./MiningPage.css";

export default function MiningPage() {
  usePageTitle(() => "Mining");
  const network = useNetwork();
  // Everything on the page belongs to one network: switching networks starts it afresh.
  return (
    <Show when={network()} keyed>
      {(n) => <Mining network={n} />}
    </Show>
  );
}

function Mining(props: { network: Network }) {
  const [search] = useSearchParams();
  const period = createMemo(() => parsePeriod(search["period"]));
  const net = () => props.network;
  const ranking = createMiningData(net, () => `/v1/mining/pools/${period().id}`, isRanking);
  const hashrates = createMiningData(
    net,
    () => `/v1/mining/hashrate/${hashrateInterval(period())}`,
    isHashrates,
  );
  onMount(() => void Chart.preload());

  return (
    <div class="wrap mining">
      <header class="mining-head">
        <h1>Mining</h1>
        <PeriodPicker period={period()} />
      </header>
      <Show when={networkNote(props.network)}>{(text) => <p class="mining-note">{text()}</p>}</Show>
      <NetworkNow network={props.network} hashrates={hashrates} />
      <HashrateChart period={period()} hashrates={hashrates} network={props.network} />
      <Pools period={period()} ranking={ranking} hashrates={hashrates} network={props.network} />
      <Rewards period={period()} ranking={ranking} network={props.network} />
      <OverTime period={period()} network={props.network} />
      <RecentBlocks />
    </div>
  );
}

function PeriodPicker(props: { period: Period }) {
  return (
    <nav class="mining-periods" aria-label="Period">
      <ul>
        <For each={PERIODS}>
          {(p) => (
            <li>
              <Link
                class="mining-periods__item"
                href={paths.mining(p.id)}
                aria-current={p.id === props.period.id ? "page" : undefined}
                aria-label={p.words}
              >
                {p.label}
              </Link>
            </li>
          )}
        </For>
      </ul>
    </nav>
  );
}

function NetworkNow(props: { network: Network; hashrates: MiningData<Hashrates> }) {
  const h = () => props.hashrates.answer()?.value;
  const meaningless = () => (competitive(props.network) ? undefined : "Not meaningful on regtest");
  return (
    <section class="mining-now" aria-labelledby="mining-now-title">
      <SectionHeader title="The network now" id="mining-now-title" />
      <div class="mining-now__grid">
        <dl class="mining-now__stats" aria-busy={!h()}>
          <Stat
            label="Hashrate"
            sub={
              meaningless() ??
              (typeof h()?.currentHashrate === "number" ? "Over the last 1 008 blocks" : undefined)
            }
          >
            <Show when={h()}>
              {(a) => (
                <span class="num">{formatHashrate(a().currentHashrate ?? Number.NaN, 4)}</span>
              )}
            </Show>
          </Stat>
          <Stat label="Difficulty" sub={h() ? "Times the minimum" : undefined}>
            <Show when={h()}>
              {(a) => <span class="num">{difficultyFull(a().currentDifficulty)}</span>}
            </Show>
          </Stat>
        </dl>
        <div class="mining-now__adjustment">
          <h3 class="mining-now__subtitle">Next difficulty adjustment</h3>
          <DifficultyAdjustment network={props.network} tip={() => undefined} />
        </div>
      </div>
    </section>
  );
}

function HashrateChart(props: {
  period: Period;
  hashrates: MiningData<Hashrates>;
  network: Network;
}) {
  const series = createMemo(() => {
    const a = props.hashrates.answer();
    return a ? hashrateSeries(a.value) : undefined;
  });
  const shown = () => (props.period.id === "24h" ? parsePeriod("1w") : props.period);
  return (
    <section class="mining-section" aria-labelledby="mining-hashrate-title">
      <SectionHeader title="Hashrate and difficulty" id="mining-hashrate-title">
        {props.period.id === "all"
          ? "One point a week"
          : "One point a day, each the average of the 7 days before it"}
      </SectionHeader>
      <Show when={props.period.id === "24h"}>
        <p class="mining-aside">
          Hashrate is estimated over a week of blocks, so this chart shows the last week.
        </p>
      </Show>
      <Section
        what="the hashrate"
        reserve={chartHeight(260)}
        ready={series() !== undefined}
        error={props.hashrates.error()}
        retry={props.hashrates.retry}
        busy={props.hashrates.loading()}
      >
        <CoverageNote coverage={props.hashrates.answer()?.coverage} period={shown()} />
        <Show
          when={(series()?.x.length ?? 0) > 0}
          fallback={<NothingStored what="blocks" period={shown()} />}
        >
          <ChartSlot
            label={`Hashrate and difficulty, ${shown().words}`}
            x={series()?.x ?? []}
            domain={periodDomain(shown(), series()?.x ?? [], Date.now() / 1000)}
            dataFrom={props.hashrates.answer()?.coverage?.fromTime}
            height={260}
            left={{ format: (v) => formatHashrate(v), log: "auto" }}
            right={{ format: difficultyShort, log: "auto" }}
            series={[
              {
                name: competitive(props.network) ? "Hashrate" : "Hashrate (not meaningful)",
                values: series()?.hashrate ?? [],
                draw: "area",
                format: (v) => formatHashrate(v, 4),
              },
              {
                name: "Difficulty",
                values: series()?.difficulty ?? [],
                draw: "step",
                axis: "right",
                tone: 2,
                format: difficultyFull,
              },
            ]}
          />
        </Show>
      </Section>
    </section>
  );
}

/** Rows shown before "Show all pools". */
const POOLS_SHOWN = 12;

function Pools(props: {
  period: Period;
  ranking: MiningData<Ranking>;
  hashrates: MiningData<Hashrates>;
  network: Network;
}) {
  const [all, setAll] = createSignal(false);
  const r = () => props.ranking.answer()?.value;
  const network = () => {
    const a = r();
    return a ? networkHashrate(props.period, a, props.hashrates.answer()?.value) : undefined;
  };
  const rows = () => {
    const list = r()?.pools ?? [];
    return all() ? list : list.slice(0, POOLS_SHOWN);
  };
  const share = (p: Ranked) => {
    const total = r()?.blockCount ?? 0;
    return total > 0 ? p.blockCount / total : 0;
  };
  const weeks = createMiningData(
    () => props.network,
    () =>
      sharesOverTime(props.period) ? `/v1/mining/hashrate/pools/${props.period.id}` : undefined,
    isPoolWeeks,
  );
  const layers = createMemo(() => {
    const w = weeks.answer()?.value;
    return w ? poolLayers(w) : undefined;
  });

  return (
    <section class="mining-section" aria-labelledby="mining-pools-title">
      <SectionHeader title="Pools" id="mining-pools-title">
        <Show when={r()}>
          {(a) =>
            `${groupDigits(a().blockCount)} ${a().blockCount === 1 ? "block" : "blocks"} in ${props.period.words}`
          }
        </Show>
      </SectionHeader>
      <Section
        what="the pools"
        reserve="38rem"
        hold={false}
        ready={r() !== undefined}
        error={props.ranking.error()}
        retry={props.ranking.retry}
        busy={props.ranking.loading()}
      >
        <CoverageNote coverage={props.ranking.answer()?.coverage} period={props.period} />
        <Show
          when={(r()?.blockCount ?? 0) > 0}
          fallback={<NothingStored what="blocks" period={props.period} />}
        >
          <div class="mining-pools">
            <div class="mining-pools__ranking">
              <DataTable
                caption={`Pools ranked by blocks found in ${props.period.words}`}
                rows={rows()}
                columns={[
                  {
                    header: "Rank",
                    cell: (p) => <span class="num mining-rank">{p.rank}</span>,
                  },
                  {
                    header: "Pool",
                    cell: (p) => (
                      <span class="mining-pool">
                        <Link href={paths.pool(p.slug)}>{p.name}</Link>
                        <Show when={p.poolId === 0}>
                          <span class="mining-pool__why">
                            named by no tag or address on the list
                          </span>
                        </Show>
                      </span>
                    ),
                  },
                  {
                    header: "Share",
                    cell: (p) => (
                      <span class="mining-share">
                        <span class="mining-share__bar" aria-hidden="true">
                          <span class="mining-share__fill" style={{ "--p": String(share(p)) }} />
                        </span>
                        <span class="num">{formatShare(share(p))}</span>
                      </span>
                    ),
                  },
                  { header: "Blocks", numeric: true, cell: (p) => groupDigits(p.blockCount) },
                  {
                    header: "Hashrate, est.",
                    numeric: true,
                    optional: true,
                    cell: (p) => {
                      const n = network();
                      return n === undefined || !competitive(props.network)
                        ? MISSING
                        : formatHashrate(n * share(p));
                    },
                  },
                  {
                    header: "Empty blocks",
                    numeric: true,
                    optional: true,
                    cell: (p) => groupDigits(p.emptyBlocks),
                  },
                ]}
              />
              <Show when={(r()?.pools.length ?? 0) > POOLS_SHOWN}>
                <button class="btn mining-more" type="button" onClick={() => setAll(!all())}>
                  {all() ? "Show the top pools only" : `Show all ${r()?.pools.length ?? 0} pools`}
                </button>
              </Show>
              <p class="mining-aside">
                A pool's hashrate is its share of the blocks times the network's hashrate over the
                same blocks. Blocks are credited by the payout address first, then by the coinbase
                tag; ones that name no pool on the list are counted as Unknown.
              </p>
            </div>
            <div class="mining-pools__shares">
              <h3 class="mining-subtitle">Share of blocks, week by week</h3>
              <Show
                when={sharesOverTime(props.period)}
                fallback={
                  <p class="mining-aside">
                    Shares are counted by the week. Choose 1 m or longer to see how they change.
                  </p>
                }
              >
                <Section
                  what="the weekly shares"
                  reserve={chartHeight(260)}
                  ready={layers() !== undefined}
                  error={weeks.error()}
                  retry={weeks.retry}
                  busy={weeks.loading()}
                >
                  <Show
                    when={(layers()?.x.length ?? 0) > 0}
                    fallback={<NothingStored what="weeks" period={props.period} />}
                  >
                    <ChartSlot
                      label={`Share of blocks by pool, week by week, ${props.period.words}`}
                      x={layers()?.x ?? []}
                      domain={
                        weeks.answer()?.coverage?.complete === false
                          ? periodDomain(props.period, layers()?.x ?? [], Date.now() / 1000)
                          : undefined
                      }
                      dataFrom={weeks.answer()?.coverage?.fromTime}
                      height={260}
                      left={{ format: percent, domain: [0, 1] }}
                      layers={layers()?.layers ?? []}
                      layerFormat={formatShare}
                      time={(t) => `Week of ${formatDay(t)}`}
                      summary={shareSummary(layers()?.layers ?? [])}
                    />
                  </Show>
                </Section>
              </Show>
            </div>
          </div>
        </Show>
      </Section>
    </section>
  );
}

/** The weekly shares in words: each layer's average share over the weeks shown. */
function shareSummary(layers: readonly { name: string; values: readonly number[] }[]): string {
  const parts = layers.map((l) => {
    const avg = l.values.reduce((s, v) => s + v, 0) / Math.max(1, l.values.length);
    return `${l.name} ${formatShare(avg)}`;
  });
  return `Average weekly share of blocks: ${parts.join(", ")}.`;
}

function Rewards(props: { period: Period; ranking: MiningData<Ranking>; network: Network }) {
  const count = () => Math.min(REWARD_MAX, props.ranking.answer()?.value.blockCount ?? 0);
  const stats = createMiningData(
    () => props.network,
    () => (count() > 0 ? `/v1/mining/reward-stats/${count()}` : undefined),
    isRewardStats,
  );
  const f = () => {
    const a = stats.answer()?.value;
    return a ? rewardFigures(a) : undefined;
  };
  return (
    <Show when={props.ranking.answer() === undefined || count() > 0}>
      <section class="mining-section" aria-labelledby="mining-rewards-title">
        <SectionHeader title="Rewards" id="mining-rewards-title">
          <Show when={stats.answer()}>
            {(a) =>
              `Blocks ${groupDigits(a().value.startBlock)} to ${groupDigits(a().value.endBlock)}`
            }
          </Show>
        </SectionHeader>
        <Show when={(props.ranking.answer()?.value.blockCount ?? 0) > REWARD_MAX}>
          <p class="mining-aside">
            Reward totals cover the last {groupDigits(REWARD_MAX)} blocks, about a year.
          </p>
        </Show>
        <Section
          what="the rewards"
          reserve="6rem"
          ready={f() !== undefined}
          error={stats.error()}
          retry={stats.retry}
          busy={stats.loading()}
        >
          <dl class="mining-rewards">
            <Stat
              label="Paid to miners"
              sub={f() ? `${groupDigits(f()?.blocks ?? 0)} blocks` : undefined}
            >
              <Amount sats={f()?.reward ?? 0} />
            </Stat>
            <Stat label="New coins" sub="The block subsidy">
              <Amount sats={f()?.subsidy ?? 0} />
            </Stat>
            <Stat label="Fees" sub={`${formatShare(f()?.feeShare ?? 0)} of the reward`}>
              <Amount sats={f()?.fees ?? 0} />
            </Stat>
            <Stat label="Per block" sub={`${groupDigits(f()?.txs ?? 0)} transactions in all`}>
              <Amount sats={Math.round(f()?.perBlock ?? 0)} />
            </Stat>
          </dl>
        </Section>
      </section>
    </Show>
  );
}

const SERIES = [
  { id: "fees", label: "Fees" },
  { id: "rewards", label: "Rewards" },
  { id: "rates", label: "Fee rates" },
  { id: "sizes", label: "Size" },
] as const;
type SeriesId = (typeof SERIES)[number]["id"];

function OverTime(props: { period: Period; network: Network }) {
  const [shown, setShown] = createSignal<SeriesId>("fees");
  const net = () => props.network;
  const path = (id: SeriesId, route: string) => () =>
    shown() === id ? `/v1/mining/blocks/${route}/${props.period.id}` : undefined;
  const fees = createMiningData(net, path("fees", "fees"), isPoints("avgFees"));
  const rewards = createMiningData(net, path("rewards", "rewards"), isPoints("avgRewards"));
  const rates = createMiningData(
    net,
    () =>
      shown() === "rates" ? `/v1/mining/blocks/fee-rates/${props.period.id}/precise` : undefined,
    isRatePoints,
  );
  const sizes = createMiningData(net, path("sizes", "sizes-weights"), isSizes);
  const data = (): MiningData<unknown> =>
    ({ fees, rewards, rates, sizes } as Record<SeriesId, MiningData<unknown>>)[shown()];
  const time = () => (props.period.id === "24h" ? formatMoment : formatDay);
  const common = (x: readonly number[], what: string) => ({
    label: `${what}, ${props.period.words}`,
    x,
    domain: periodDomain(props.period, x, Date.now() / 1000),
    dataFrom: data().answer()?.coverage?.fromTime,
    time: time(),
  });
  const pos = (v: number) => (v > 0 ? v : null);

  return (
    <section class="mining-section" aria-labelledby="mining-blocks-title">
      <SectionHeader title="Blocks over time" id="mining-blocks-title">
        <span>
          {props.period.id === "24h" ? "One point a block" : "Each point averages a run of blocks"}
        </span>
      </SectionHeader>
      <div class="mining-tabs" role="group" aria-label="Show">
        <For each={SERIES}>
          {(s) => (
            <button
              type="button"
              class="btn"
              aria-pressed={shown() === s.id}
              onClick={() => setShown(s.id)}
            >
              {s.label}
            </button>
          )}
        </For>
      </div>
      <Section
        what="the block figures"
        reserve={chartHeight(240)}
        ready={data().answer() !== undefined}
        error={data().error()}
        retry={data().retry}
        busy={data().loading()}
      >
        <CoverageNote coverage={data().answer()?.coverage} period={props.period} />
        <Show when={shown() === "fees" && fees.answer()}>
          {(a) => (
            <Show
              when={a().value.length > 0}
              fallback={<NothingStored what="blocks" period={props.period} />}
            >
              <ChartSlot
                {...common(
                  a().value.map((p) => p.timestamp),
                  "Average fees per block",
                )}
                left={{ format: btcShort, zero: true }}
                series={[
                  {
                    name: "Fees per block",
                    draw: "area",
                    values: a().value.map((p) => Number(p["avgFees"])),
                    format: btcFull,
                  },
                ]}
              />
            </Show>
          )}
        </Show>
        <Show when={shown() === "rewards" && rewards.answer()}>
          {(a) => (
            <Show
              when={a().value.length > 0}
              fallback={<NothingStored what="blocks" period={props.period} />}
            >
              <ChartSlot
                {...common(
                  a().value.map((p) => p.timestamp),
                  "Average reward per block",
                )}
                left={{ format: btcShort, zero: true, log: "auto" }}
                series={[
                  {
                    name: "Reward per block",
                    draw: "area",
                    values: a().value.map((p) => Number(p["avgRewards"])),
                    format: btcFull,
                  },
                ]}
              />
            </Show>
          )}
        </Show>
        <Show when={shown() === "rates" && rates.answer()}>
          {(a) => (
            <Show
              when={a().value.length > 0}
              fallback={<NothingStored what="fee rates" period={props.period} />}
            >
              <ChartSlot
                {...common(
                  a().value.map((p) => p.timestamp),
                  "Fee rates paid in blocks",
                )}
                left={{ format: (v) => formatFeeAxis(v), log: true, fee: true }}
                bands={[
                  {
                    name: "10th to 90th percentile",
                    tone: 1,
                    lo: a().value.map((p) => pos(p.avgFee_10)),
                    hi: a().value.map((p) => pos(p.avgFee_90)),
                    format: feeRateText,
                  },
                  {
                    name: "25th to 75th percentile",
                    tone: 2,
                    lo: a().value.map((p) => pos(p.avgFee_25)),
                    hi: a().value.map((p) => pos(p.avgFee_75)),
                    format: feeRateText,
                  },
                ]}
                series={[
                  {
                    name: "Median",
                    draw: "line",
                    values: a().value.map((p) => pos(p.avgFee_50)),
                    format: feeRateText,
                  },
                ]}
              />
              <p class="mining-aside">
                Percentiles are weighted by size: the median is the rate at which half a block's
                bytes paid less. Fee rates are stored for the last year of blocks.
              </p>
            </Show>
          )}
        </Show>
        <Show when={shown() === "sizes" && sizes.answer()}>
          {(a) => (
            <Show
              when={a().value.sizes.length > 0}
              fallback={<NothingStored what="blocks" period={props.period} />}
            >
              <ChartSlot
                {...common(
                  a().value.sizes.map((p) => p.timestamp),
                  "Average block size and weight",
                )}
                left={{ format: bytesText, zero: true }}
                right={{ format: percent, domain: [0, 1] }}
                series={[
                  {
                    name: "Size",
                    draw: "area",
                    values: a().value.sizes.map((p) => p.avgSize),
                    format: bytesText,
                  },
                  {
                    name: "Weight, of the limit",
                    draw: "line",
                    axis: "right",
                    tone: 2,
                    values: a().value.weights.map((p) => p.avgWeight / 4_000_000),
                    format: percent,
                  },
                ]}
              />
            </Show>
          )}
        </Show>
      </Section>
    </section>
  );
}

/** Fee-rate axis ticks, which are powers of ten and their 2s and 5s: `0.2 sat/vB`, `10 sat/vB`. */
const formatFeeAxis = (v: number): string =>
  `${v >= 1 ? String(v) : v.toFixed(v >= 0.1 ? 1 : 2)} sat/vB`;
