import { createMemo, For, Show } from "solid-js";
import { SectionHeader } from "../../components/SectionHeader";
import { formatFeeRateUp } from "../../lib/format";
import { type FeeAnswer, type FeeGroup, groupLevels } from "./fees";

/**
 * "Fee to pay now": four plainly labelled rates, slowest first, each with what it promises, rounded
 * up so the figure is never less than the rate it stands for. Neighbours that print the same rate
 * read as one answer. The node's estimates, served while the server cannot make its own, say so;
 * no answer shows no number.
 */
/**
 * Where an answer sits in the panel's two rows of two: one cell, a whole row, or both rows. A lone
 * answer beside a group of three takes its row too, so there are always exactly two rows.
 */
function placement(span: number, groups: readonly FeeGroup[]): "cell" | "row" | "all" {
  if (span === 4) return "all";
  return span > 1 || groups.some((g) => g.span === 3) ? "row" : "cell";
}

export function FeePanel(props: { answer: () => FeeAnswer | null | undefined }) {
  const groups = createMemo(() => groupLevels(props.answer() ?? undefined, formatFeeRateUp));
  return (
    <section class="home-fees" aria-labelledby="fees-title">
      <SectionHeader title="Fee to pay now" id="fees-title" />
      <Show
        when={props.answer() !== null}
        fallback={
          <p class="home-fees__none">
            No recommendation right now. Rather than show an old one, this panel waits until the
            server has a current answer.
          </p>
        }
      >
        <dl class="home-fees__list" aria-busy={props.answer() ? "false" : "true"}>
          <For each={groups()}>
            {(g) => (
              <div class={["home-fees__item", `home-fees__item--${placement(g.span, groups())}`]}>
                <dt>
                  <span class="home-fees__name">{g.name}</span>
                  <span class="home-fees__when">{g.when}</span>
                </dt>
                <dd class="home-fees__rate">
                  <Show when={g.rate} fallback={<span class="muted">–</span>}>
                    {(r) => (
                      <>
                        <span class="num">{formatFeeRateUp(r())}</span>
                        <span class="unit"> sat/vB</span>
                        <Show when={g.span === 4 && props.answer()?.source === "projected"}>
                          <span class="home-fees__same">
                            Every wait pays the same: the next block has room at this rate.
                          </span>
                        </Show>
                      </>
                    )}
                  </Show>
                </dd>
              </div>
            )}
          </For>
        </dl>
        <p class="home-fees__note">
          {props.answer()?.source === "node"
            ? "The node's own estimates: this server cannot make its own recommendation right now."
            : "Read from the projected blocks, rounded up."}
        </p>
      </Show>
    </section>
  );
}
