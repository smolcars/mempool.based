import { createMemo, Show, For } from "solid-js";
import type { JSX } from "@solidjs/web";
import { Amount } from "../../components/Amount";
import { FeeRate } from "../../components/FeeRate";
import { Link } from "../../components/Link";
import { SectionHeader } from "../../components/SectionHeader";
import { ErrorState } from "../../components/States";
import { formatSize, groupDigits } from "../../lib/format";
import { paths } from "../../lib/network";
import type { Feed } from "./feeds";
import { type FeeBandShare, type MempoolFigures, sharesOf } from "./mempool";

const BLOCK_VSIZE = 1_000_000;

/** How many blocks the mempool would fill: `≈ 31 blocks`, `≈ 1 block`. */
export function blocksOf(vsize: number): string {
  if (vsize <= 0) return "empty";
  const n = Math.ceil(vsize / BLOCK_VSIZE);
  return `≈ ${groupDigits(n)} ${n === 1 ? "block" : "blocks"}`;
}

/** A band's range in sat/vB, as short as a row allows: `10+`, `0.5–1`, `< 0.2`. */
export function rangeLabel(b: FeeBandShare): string {
  const r = (x: number) => String(x);
  if (b.to === null) return `${r(b.from)}+`;
  if (b.band === 1) return `< ${r(b.to)}`;
  return `${r(b.from)}–${r(b.to)}`;
}

const percent = (share: number) =>
  share > 0 && share < 0.01 ? "under 1%" : `${Math.round(share * 100)}%`;

/**
 * Mempool: four figures, then what waits in each fee band, dearest first, as a row of figures with a
 * bar of its share. The numbers carry today's structure even when one cheap band holds nearly all.
 */
export function MempoolPanel(props: { feed: Feed<MempoolFigures> }) {
  const m = () => props.feed.data();
  const bands = createMemo(() => m()?.bands ?? sharesOf([]));

  return (
    <section class="home-mempool" aria-labelledby="mempool-title">
      <SectionHeader title="Mempool" id="mempool-title">
        <Link href={paths.mempool()}>Mempool history</Link>
      </SectionHeader>
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
        <figure class="home-depth">
          <figcaption class="home-depth__caption">
            Waiting, by fee rate: the dearest are mined first.
          </figcaption>
          <ol class="home-depth__list" aria-busy={m() ? "false" : "true"}>
            <For each={[...bands()].reverse()}>
              {(b) => (
                <li
                  class={["home-depth__row", { "home-depth__row--none": b.vsize === 0 }]}
                  style={{ "--w": String(b.share), "--c": `var(--fee-${b.band})` }}
                >
                  <span class="home-depth__range num">{rangeLabel(b)}</span>
                  <span class="home-depth__bar" aria-hidden="true" />
                  <span class="home-depth__size num">
                    {m() ? (b.vsize > 0 ? formatSize(b.vsize) : "–") : ""}
                    <span class="visually-hidden">, {percent(b.share)} of the mempool</span>
                  </span>
                </li>
              )}
            </For>
          </ol>
          <Show when={m() && m()?.vsize === 0}>
            <p class="home-depth__empty">
              Nothing is waiting. The next block would hold only its coinbase.
            </p>
          </Show>
        </figure>
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
