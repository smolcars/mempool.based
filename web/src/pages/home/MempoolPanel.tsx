import { createMemo, createSignal, Show, For } from "solid-js";
import type { JSX } from "@solidjs/web";
import { Amount } from "../../components/Amount";
import { FeeKey, FeeRate } from "../../components/FeeRate";
import { SectionHeader } from "../../components/SectionHeader";
import { ErrorState } from "../../components/States";
import { formatFeeRate, formatSize, groupDigits } from "../../lib/format";
import type { Feed } from "./feeds";
import { type FeeBandShare, type MempoolFigures, sharesOf } from "./mempool";

const BLOCK_VSIZE = 1_000_000;

/** How many blocks the mempool would fill: `≈ 31 blocks`, `≈ 1 block`. */
export function blocksOf(vsize: number): string {
  if (vsize <= 0) return "empty";
  const n = Math.ceil(vsize / BLOCK_VSIZE);
  return `≈ ${groupDigits(n)} ${n === 1 ? "block" : "blocks"}`;
}

function rangeText(b: FeeBandShare): string {
  if (b.to === null) return `${formatFeeRate(b.from)} sat/vB and up`;
  if (b.band === 1) return `under ${formatFeeRate(b.to)} sat/vB`;
  return `${formatFeeRate(b.from)} to ${formatFeeRate(b.to)} sat/vB`;
}

const percent = (share: number) =>
  share > 0 && share < 0.01 ? "under 1%" : `${Math.round(share * 100)}%`;

/** Mempool: four figures and the bar of waiting transactions by fee band. */
export function MempoolPanel(props: { feed: Feed<MempoolFigures> }) {
  const m = () => props.feed.data();
  const bands = createMemo(() => m()?.bands ?? sharesOf([]));
  // Index, not object: bands are recomputed on every poll, and the open detail follows its band.
  const [tipAt, setTipAt] = createSignal<number>();
  const [focused, setFocused] = createSignal(false);
  const tip = () => {
    const i = tipAt();
    return i === undefined ? undefined : bands()[i];
  };

  return (
    <section class="home-mempool" aria-labelledby="mempool-title">
      <SectionHeader title="Mempool" id="mempool-title" />
      <Show
        when={m() ?? props.feed.error() === undefined}
        fallback={
          <ErrorState
            title="Could not load the mempool"
            error={props.feed.error()}
            retry={props.feed.refresh}
          />
        }
      >
        <dl class="home-stats" aria-busy={m() ? "false" : "true"}>
          <Stat label="Transactions">
            <Show when={m()}>{(s) => <span class="num">{groupDigits(s().count)}</span>}</Show>
          </Stat>
          <Stat label="Size">
            <Show when={m()}>
              {(s) => (
                <>
                  <span class="num">{formatSize(s().vsize)}</span>
                  <span class="home-stat__sub num">{blocksOf(s().vsize)}</span>
                </>
              )}
            </Show>
          </Stat>
          <Stat label="Fees waiting">
            <Show when={m()}>{(s) => <Amount sats={s().fees} />}</Show>
          </Stat>
          <Stat label="Lowest accepted">
            <Show when={m()}>
              {(s) => (
                <Show when={s().lowest} fallback={<span class="muted">–</span>}>
                  {(r) => (
                    <>
                      <FeeRate rate={r()} />
                      <span class="home-stat__sub">{s().lowestNote}</span>
                    </>
                  )}
                </Show>
              )}
            </Show>
          </Stat>
        </dl>
        <figure class="home-feebar">
          <div class="home-feebar__frame">
            <ul
              class="home-feebar__bar"
              aria-label="Waiting transactions by fee rate, cheapest first"
            >
              <For keyed={false} each={bands()}>
                {(b, i) => (
                  // oxlint-disable-next-line jsx-a11y/no-noninteractive-element-interactions -- each bar takes focus and hover to show its tooltip
                  <li
                    class="home-feebar__seg"
                    style={{
                      "--x": String(b().x),
                      "--w": String(b().share),
                      "--c": `var(--fee-${b().band})`,
                    }}
                    // oxlint-disable-next-line jsx-a11y/no-noninteractive-tabindex -- focus shows the bar's tooltip
                    tabindex={b().share > 0 ? 0 : undefined}
                    aria-hidden={b().share > 0 ? undefined : "true"}
                    aria-label={`${rangeText(b())}: ${formatSize(b().vsize)}, ${percent(b().share)}`}
                    onPointerEnter={() => {
                      setFocused(false);
                      setTipAt(i);
                    }}
                    onPointerLeave={() => setTipAt(undefined)}
                    onFocus={() => {
                      setFocused(true);
                      setTipAt(i);
                    }}
                    onBlur={() => setTipAt(undefined)}
                  />
                )}
              </For>
            </ul>
            <Show when={m() && m()?.vsize === 0}>
              <p class="home-feebar__empty">
                Nothing is waiting. The next block would hold only its coinbase.
              </p>
            </Show>
            <Show when={tip()}>
              {(b) => (
                <>
                  <span
                    class={["home-feebar__ring", { "home-feebar__ring--focus": focused() }]}

                    style={{ "--x": String(b().x), "--w": String(b().share) }}
                  />
                  <p
                    class="home-feebar__tip"
                    aria-hidden="true"
                    ref={(el) => {
                      // Centered on the band, kept inside the bar so it never widens the page.
                      queueMicrotask(() => {
                        const frame = el.parentElement?.clientWidth ?? 0;
                        const left = (b().x + b().share / 2) * frame - el.offsetWidth / 2;
                        el.style.setProperty(
                          "--left",
                          `${Math.max(0, Math.min(frame - el.offsetWidth, left))}px`,
                        );
                      });
                    }}
                  >
                    <strong>{rangeText(b())}</strong>
                    <span class="num">
                      {formatSize(b().vsize)}, {percent(b().share)} of the mempool
                    </span>
                  </p>
                </>
              )}
            </Show>
          </div>
          <figcaption class="home-feebar__caption">
            Waiting transactions by fee rate, cheapest on the left. Hover or focus a band for
            detail.
          </figcaption>
        </figure>
        <FeeKey />
      </Show>
    </section>
  );
}

export function Stat(props: { label: string; children: JSX.Element }) {
  return (
    <div class="home-stat">
      <dt class="home-stat__label">{props.label}</dt>
      <dd class="home-stat__value">{props.children}</dd>
    </div>
  );
}
