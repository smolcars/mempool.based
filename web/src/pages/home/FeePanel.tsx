import { createEffect, createMemo, createSignal, For, onSettled, Show } from "solid-js";
import { FeeRate } from "../../components/FeeRate";
import { SectionHeader } from "../../components/SectionHeader";
import { FEE_KEY_BOUNDS, feeColor, formatFeeRatePrecise } from "../../lib/format";
import type { FeeAnswer, LevelName } from "./fees";
import { LADDER_MAX, ladderPos, stackLabels } from "./mempool";

const TICKS = [0.1, 0.2, 0.5, 1, 2, 5, 10];

/** The four levels as the panel shows them, slowest first: left to right on the ladder. */
const PRIORITIES: readonly { level: LevelName; name: string; when: string }[] = [
  { level: "no_rush", name: "No rush", when: "within a day" },
  { level: "low", name: "Low", when: "within an hour" },
  { level: "medium", name: "Medium", when: "within 30 min" },
  { level: "high", name: "High", when: "next block" },
];

/**
 * "Fee to pay now": the server's recommendation for four waits as pins on a log scale, then as a
 * list, at the node's resolution. Labels stack into rows so close values never collide. The node's
 * estimates, served while the server cannot make its own, say so; no answer shows no number.
 */
export function FeePanel(props: { answer: () => FeeAnswer | null | undefined }) {
  const rates = createMemo(() => {
    const a = props.answer();
    return a ? PRIORITIES.map((p) => a.levels.find((l) => l.name === p.level)?.rate) : undefined;
  });
  let track: HTMLDivElement | undefined;
  const labels: (HTMLSpanElement | undefined)[] = [];
  const [width, setWidth] = createSignal(0);
  const [layout, setLayout] = createSignal<{ row: number; flip: boolean }[]>([]);

  onSettled(() => {
    const ro = new ResizeObserver(() => setWidth(track?.clientWidth ?? 0));
    if (track) ro.observe(track);
    return () => ro.disconnect();
  });
  createEffect(
    () => ({ r: rates(), w: width() }),
    ({ r, w }) => {
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
    },
  );

  return (
    <section class="home-fees" aria-labelledby="fees-title">
      <SectionHeader title="Fee to pay now" id="fees-title" />
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
                    {formatFeeRatePrecise(rates()?.[i] ?? Number.NaN)}
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
        when={props.answer() !== null}
        fallback={
          <p class="home-fees__none">
            No recommendation right now. Rather than show an old one, this panel waits until the
            server has a current answer.
          </p>
        }
      >
        <dl class="home-ladder__list" aria-busy={props.answer() ? "false" : "true"}>
          <For keyed={false} each={PRIORITIES}>
            {(p, i) => (
              <div class="home-ladder__item">
                <dt>
                  {p().name}
                  <span class="home-ladder__when">{p().when}</span>
                </dt>
                <dd>
                  <Show when={rates()?.[i]} fallback={<span class="muted">–</span>}>
                    {(r) => <FeeRate rate={r()} unit={false} precise />}
                  </Show>
                </dd>
              </div>
            )}
          </For>
        </dl>
        <p class="home-fees__note">
          {props.answer()?.source === "node"
            ? "The node's own estimates, in sat/vB: this server cannot make its own recommendation right now."
            : "What to pay, in sat/vB, to be mined within each wait, read from the projected blocks."}
        </p>
      </Show>
    </section>
  );
}

const rateColor = (rate: number | undefined) =>
  rate === undefined ? "var(--color-line-strong)" : feeColor(rate, formatFeeRatePrecise);
