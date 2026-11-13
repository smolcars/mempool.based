// Block health: how closely each block this server watched arrive matched the block it expected
// (docs/API.md "Block audits", `predictions`), over the page's period.
import { createMemo, Show } from "solid-js";
import { formatMatchRate } from "../../components/audit/model";
import { formatMoment } from "../../components/chart/scale";
import { More } from "../../components/More";
import { SectionHeader } from "../../components/SectionHeader";
import { groupDigits } from "../../lib/format";
import { type Network, paths } from "../../lib/network";
import { createMiningData, type MiningData } from "./data";
import {
  answeredPeriod,
  healthCoverage,
  healthSeries,
  isPredictions,
  type Period,
  periodBlocks,
  periodDomain,
  type Ranking,
} from "./mining";
import { ChartSlot, chartHeight, NothingStored, Section } from "./parts";

const HEIGHT = 200;
const rate = (v: number) => `${formatMatchRate(v)}%`;

export function BlockHealth(props: {
  period: Period;
  network: Network;
  ranking: MiningData<Ranking>;
}) {
  const data = createMiningData(
    () => props.network,
    () => `/v1/mining/blocks/predictions/${props.period.id}`,
    isPredictions,
  );
  const health = createMemo(() => {
    const a = data.answer();
    return a ? healthSeries(a.value) : undefined;
  });
  // While the next period loads the last answer stays, so every word follows the answer shown.
  const period = (): Period => answeredPeriod(data.answer()) ?? props.period;
  const blocks = () => periodBlocks(props.ranking.answer(), period());
  const coverage = () => {
    const h = health();
    return h ? healthCoverage(h, blocks()) : undefined;
  };
  const count = () => health()?.count ?? 0;

  return (
    <section class="mining-section" aria-labelledby="mining-health-title">
      <SectionHeader title="Block health" id="mining-health-title">
        {/* An expression, not a Show callback: a callback's string is made once and goes stale. */}
        {health() &&
          `${groupDigits(count())} audited ${count() === 1 ? "block" : "blocks"} in ${period().words}`}
      </SectionHeader>
      <Section
        what="the block audits"
        reserve={chartHeight(HEIGHT)}
        hold={count() > 0}
        ready={health() !== undefined}
        error={data.error()}
        retry={data.retry}
        busy={data.loading()}
      >
        <Show when={coverage()}>
          {(text) => (
            <p class="mining-coverage">
              <span class="mining-coverage__swatch" aria-hidden="true" />
              {text()}
            </p>
          )}
        </Show>
        <Show when={count() > 0} fallback={<NothingStored what="block audits" period={period()} />}>
          <ChartSlot
            label={`Match rate of each audited block, ${period().words}`}
            x={health()?.x ?? []}
            domain={periodDomain(period(), health()?.x ?? [], Date.now() / 1000)}
            // Before the first audit nothing is stored, unless the period's every block has one.
            dataFrom={coverage() === undefined ? undefined : health()?.first}
            height={HEIGHT}
            left={{ format: (v) => `${v}%`, domain: [0, 100] }}
            series={[
              {
                name: (health()?.per ?? 1) > 1 ? "Lowest match rate" : "Match rate",
                values: health()?.rate ?? [],
                draw: "area",
                format: rate,
              },
            ]}
            notes={[
              { name: "Block", values: health()?.height ?? [], format: groupDigits },
              ...((health()?.per ?? 1) > 1
                ? [
                    {
                      name: "Audited",
                      values: health()?.audited ?? [],
                      format: (n: number) =>
                        `${groupDigits(n)} of ${groupDigits(health()?.per ?? 1)}`,
                    },
                  ]
                : []),
            ]}
            // Only a block with no audit breaks the line, however long miners took between two.
            gap={Number.POSITIVE_INFINITY}
            time={formatMoment}
            href={(i) => {
              const h = health()?.height[i];
              return typeof h === "number" ? paths.block(h) : undefined;
            }}
          />
          <p class="mining-aside">
            <Show when={(health()?.per ?? 1) > 1}>
              Each point is the lowest of {groupDigits(health()?.per ?? 1)} blocks in a row.{" "}
            </Show>
            A point opens its block: click it, or tap it twice.
          </p>
        </Show>
      </Section>
      <More>
        <p>
          The match rate is how much of the block this server expected on a block's parent the miner
          mined, by virtual size, leaving out transactions that arrived too late to count or that
          conflicted with the block. A low one means the miner chose differently, or this node's
          mempool did not hold what the miner's did. Only blocks this server watched arrive with its
          mempool engine on are audited, so the record begins when it first did, and a gap is a
          stretch it did not watch.
        </p>
      </More>
    </section>
  );
}
