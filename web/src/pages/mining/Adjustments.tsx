// Past difficulty adjustments (docs/API.md "Mining", `difficulty-adjustments`), newest first, under
// the one the current period is heading for.
import { createSignal, Show } from "solid-js";
import { DataTable } from "../../components/DataTable";
import { Link } from "../../components/Link";
import { SectionHeader } from "../../components/SectionHeader";
import { TimeAgo } from "../../components/Time";
import { useNow } from "../../lib/clock";
import { change, isDifficultyAdjustment, PERIOD, retargets, until } from "../../lib/difficulty";
import { groupDigits } from "../../lib/format";
import { NETWORK_NAMES, type Network, paths } from "../../lib/network";
import { createMiningData } from "./data";
import { type Adjustment, adjustmentPercent, isAdjustments, parsePeriod } from "./mining";
import { CoverageNote, difficultyFull, Section } from "./parts";

/** Rows shown before "Show all": about five months. */
const SHOWN = 10;

/** A change as an arrow and words, never by colour: `↑ Up 5.54%`. */
function Change(props: { percent: number }) {
  const c = () => change(props.percent);
  return (
    <span class="num">
      <span class="mining-arrow" aria-hidden="true">
        {c().arrow}
      </span>
      {c().text}
    </span>
  );
}

export function Adjustments(props: { network: Network }) {
  const [all, setAll] = createSignal(false);
  const now = useNow();
  const net = () => props.network;
  const past = createMiningData(net, () => "/v1/mining/difficulty-adjustments", isAdjustments);
  const next = createMiningData(
    net,
    () => (retargets(props.network) ? "/v1/difficulty-adjustment" : undefined),
    isDifficultyAdjustment,
  );
  const list = () => past.answer()?.value ?? [];
  const rows = () => (all() ? list() : list().slice(0, SHOWN));

  return (
    <section class="mining-section" aria-labelledby="mining-adjustments-title">
      <SectionHeader title="Difficulty adjustments" id="mining-adjustments-title">
        Every {groupDigits(PERIOD)} blocks
      </SectionHeader>
      <Show when={next.answer()?.value}>
        {(d) => (
          <p class="mining-next">
            <span class="mining-next__label">Next, expected</span>{" "}
            <strong>
              <Change percent={d().difficultyChange} />
            </strong>{" "}
            <span class="mining-next__when">
              at block <span class="num">{groupDigits(d().nextRetargetHeight)}</span>,{" "}
              {until(d().estimatedRetargetDate, now() * 1000)}
            </span>
          </p>
        )}
      </Show>
      <Section
        what="the past adjustments"
        reserve="22rem"
        hold={false}
        ready={past.answer() !== undefined}
        error={past.error()}
        retry={past.retry}
        busy={past.loading()}
      >
        <CoverageNote coverage={past.answer()?.coverage} period={parsePeriod("all")} />
        <Show
          when={list().length > 0}
          fallback={
            <p class="mining-aside">
              {retargets(props.network)
                ? "No adjustments are stored yet."
                : `${NETWORK_NAMES[props.network]} never adjusts its difficulty, so there are none to list.`}
            </p>
          }
        >
          <DataTable
            caption="Difficulty adjustments, newest first"
            rows={rows()}
            rowHref={(a: Adjustment) => paths.block(a[1])}
            columns={[
              {
                header: "Block",
                cell: (a: Adjustment) => (
                  <span class="mining-retarget">
                    <Link class="num" href={paths.block(a[1])}>
                      {groupDigits(a[1])}
                    </Link>
                    {/* Phones drop the When column, so the age moves under the height. */}
                    <span class="mining-retarget__age">
                      <TimeAgo time={a[0]} />
                    </span>
                  </span>
                ),
              },
              {
                header: "When",
                optional: true,
                cell: (a: Adjustment) => <TimeAgo time={a[0]} />,
              },
              {
                header: "Change",
                numeric: true,
                cell: (a: Adjustment) => <Change percent={adjustmentPercent(a[3])} />,
              },
              {
                header: "Difficulty",
                numeric: true,
                cell: (a: Adjustment) => difficultyFull(a[2]),
              },
            ]}
          />
          <Show when={list().length > SHOWN}>
            <button class="btn mining-more" type="button" onClick={() => setAll(!all())}>
              {all()
                ? "Show the latest only"
                : `Show all ${groupDigits(list().length)} adjustments`}
            </button>
          </Show>
        </Show>
      </Section>
    </section>
  );
}
