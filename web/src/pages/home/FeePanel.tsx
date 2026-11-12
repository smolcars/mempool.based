import { createEffect, createMemo, createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { FeeRate } from "../../components/FeeRate";
import { SectionHeader } from "../../components/SectionHeader";
import { ErrorState } from "../../components/States";
import type { FeeEstimates } from "../../lib/esplora";
import { FEE_KEY_BOUNDS, feeColor, formatFeeRate } from "../../lib/format";
import type { Feed } from "./feeds";
import { estimateFor, LADDER_MAX, ladderPos, PRIORITIES, stackLabels } from "./mempool";

const TICKS = [0.1, 0.2, 0.5, 1, 2, 5, 10];

/**
 * "Fee to pay now": the node's estimates for four confirmation targets as pins on a log scale,
 * then as a list. Labels stack into rows so close values never collide.
 */
export function FeePanel(props: { feed: Feed<FeeEstimates> }) {
  const rates = createMemo(() => {
    const est = props.feed.data();
    return est ? PRIORITIES.map((p) => estimateFor(est, p.target)) : undefined;
  });
  const hasAny = () => rates()?.some((r) => r !== undefined) ?? false;
  let track: HTMLDivElement | undefined;
  const labels: (HTMLSpanElement | undefined)[] = [];
  const [width, setWidth] = createSignal(0);
  const [layout, setLayout] = createSignal<{ row: number; flip: boolean }[]>([]);

  onMount(() => {
    const ro = new ResizeObserver(() => setWidth(track?.clientWidth ?? 0));
    if (track) ro.observe(track);
    onCleanup(() => ro.disconnect());
  });
  createEffect(() => {
    const r = rates();
    const w = width();
    if (!r || w === 0) return;
    // Label widths are read after the values render, so a new value is measured at its new size.
    queueMicrotask(() =>
      setLayout(
        stackLabels(
          r.map((rate, i) => ({
            x: ladderPos(rate ?? 0) * w,
            width: rate === undefined ? 0 : (labels[i]?.offsetWidth ?? 0),
          })),
          w,
        ),
      ),
    );
  });

  return (
    <section class="home-fees" aria-labelledby="fees-title">
      <SectionHeader title="Fee to pay now" id="fees-title" />
      <Show
        when={props.feed.data() ?? props.feed.error() === undefined}
        fallback={
          <ErrorState
            title="Could not load fee estimates"
            error={props.feed.error()}
            retry={props.feed.refresh}
          />
        }
      >
        <div class="home-ladder" aria-hidden="true">
          <div class="home-ladder__track" ref={track}>
            <div class="home-ladder__scale">
              <For each={FEE_KEY_BOUNDS}>
                {(bound, i) => (
                  <span
                    class="home-ladder__band"
                    style={{
                      "--x": String(ladderPos(bound)),
                      "--w": String(
                        ladderPos(FEE_KEY_BOUNDS[i() + 1] ?? LADDER_MAX) - ladderPos(bound),
                      ),
                      "--c": `var(--fee-${i() + 1})`,
                    }}
                  />
                )}
              </For>
            </div>
            <For keyed={false} each={PRIORITIES}>
              {(p, i) => (
                <div
                  class={[
                    "home-ladder__pin",
                    {
                      "home-ladder__pin--flip": layout()[i]?.flip ?? false,
                      "home-ladder__pin--none": rates()?.[i] === undefined,
                    },
                  ]}

                  style={{
                    "--px": `${ladderPos(rates()?.[i] ?? 0) * width()}px`,
                    "--row": String(layout()[i]?.row ?? 0),
                    "--c": rateColor(rates()?.[i]),
                  }}
                >
                  <span class="home-ladder__pin-label" ref={(el) => (labels[i] = el)}>
                    <span class="home-ladder__pin-name">{p().name}</span>
                    <span class="home-ladder__pin-value num">
                      {formatFeeRate(rates()?.[i] ?? Number.NaN)}
                    </span>
                  </span>
                </div>
              )}
            </For>
            <div class="home-ladder__ticks">
              <For each={TICKS}>
                {(t) => (
                  <span class="home-ladder__tick num" style={{ "--x": String(ladderPos(t)) }}>
                    {String(t)}
                  </span>
                )}
              </For>
            </div>
          </div>
        </div>
        <Show
          when={!props.feed.data() || hasAny()}
          fallback={
            <p class="home-fees__none">
              The node has no fee estimates yet. It makes them once it has seen enough blocks
              confirm transactions.
            </p>
          }
        >
          <dl class="home-ladder__list" aria-busy={!props.feed.data() ? "true" : "false"}>
            <For keyed={false} each={PRIORITIES}>
              {(p, i) => (
                <div class="home-ladder__item">
                  <dt>
                    {p().name}
                    <span class="home-ladder__when">{p().when}</span>
                  </dt>
                  <dd>
                    <Show when={rates()?.[i]} fallback={<span class="muted">–</span>}>
                      {(r) => <FeeRate rate={r()} unit={false} />}
                    </Show>
                  </dd>
                </div>
              )}
            </For>
          </dl>
        </Show>
        <p class="home-fees__note">
          The node's own estimates, in sat/vB, of what confirms within each wait.
        </p>
      </Show>
    </section>
  );
}

const rateColor = (rate: number | undefined) =>
  rate === undefined ? "var(--color-line-strong)" : feeColor(rate);
