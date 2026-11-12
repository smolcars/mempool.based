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
import {
  depthRows,
  type FeeBandShare,
  type MempoolFigures,
  nextBlockReach,
  sharesOf,
} from "./mempool";

const BLOCK_VSIZE = 1_000_000;

/** How many blocks the mempool would fill: `≈ 31 blocks`, `≈ 1 block`. */
export function blocksOf(vsize: number): string {
  if (vsize <= 0) return "empty";
  const n = Math.ceil(vsize / BLOCK_VSIZE);
  return `≈ ${groupDigits(n)} ${n === 1 ? "block" : "blocks"}`;
}

/** A band's range in sat/vB, as short as a row allows: `10+`, `0.5–1`, `< 0.2`. */
export function rangeLabel(b: FeeBandShare): string {
  if (b.to === null) return `${b.from}+`;
  if (b.band === 1) return `< ${b.to}`;
  return `${b.from}–${b.to}`;
}

/** How deep the queue is, in blocks: `< 0.1`, `0.4`, `31`. */
function deepText(deep: number): string {
  if (deep <= 0) return "–";
  if (deep < 0.1) return "< 0.1";
  return deep < 10 ? deep.toFixed(1) : groupDigits(deep);
}

/**
 * Mempool: four figures, the fee rate the next block's worth reaches down to, then each fee band,
 * dearest first: its size against one block (a band past a block fills its bar) and how many
 * blocks deep the queue is by its end.
 */
export function MempoolPanel(props: { feed: Feed<MempoolFigures> }) {
  const m = () => props.feed.data();
  const rows = createMemo(() => depthRows(m()?.bands ?? sharesOf([])));
  const wide = typeof matchMedia === "function" && matchMedia("(min-width: 641px)").matches;

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
            <Show when={m()}>{(s) => <Amount sats={s().fees} short />}</Show>
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
        <div class="home-depth" aria-busy={m() ? "false" : "true"}>
          <p class="home-depth__answer">
            <Show when={m()} fallback={<span class="home-bone" />}>
              {(s) => (
                <Show
                  when={nextBlockReach(rows())}
                  fallback={
                    s().vsize === 0
                      ? "Nothing is waiting. The next block would hold only its coinbase."
                      : "All of it would fit in the next block."
                  }
                >
                  {(reach) => (
                    <>
                      The next block's worth reaches down to{" "}
                      <strong class="num">{rangeLabel(reach())} sat/vB</strong>.
                    </>
                  )}
                </Show>
              )}
            </Show>
          </p>
          {/* Open on wide screens; a phone shows the answer and keeps the rows one tap away. */}
          <details class="home-depth__more" open={wide}>
            <summary class="home-depth__summary">By fee rate</summary>
            <table class="home-depth__table">
              <caption class="visually-hidden">
                Waiting by fee rate, dearest first, each band's size and how many blocks deep the
                queue is by its end
              </caption>
              <thead>
                <tr>
                  <th scope="col">sat/vB</th>
                  <th scope="col">
                    <span class="visually-hidden">Size against one block, log scale</span>
                  </th>
                  <th scope="col" class="home-depth__num">
                    Size
                  </th>
                  <th scope="col" class="home-depth__num">
                    Blocks deep
                  </th>
                </tr>
              </thead>
              <tbody>
                <For each={rows()}>
                  {(r) => (
                    <tr
                      class={[
                        "home-depth__row",
                        {
                          "home-depth__row--none": r.vsize === 0,
                          "home-depth__row--full": r.blocks > 1,
                        },
                      ]}
                      style={{ "--w": String(r.bar), "--c": `var(--fee-${r.band})` }}
                    >
                      <td class="home-depth__range num">{rangeLabel(r)}</td>
                      <td class="home-depth__barcell" aria-hidden="true">
                        <span class="home-depth__bar" />
                      </td>
                      <td class="home-depth__num num">
                        {m() ? (r.vsize > 0 ? formatSize(r.vsize) : "–") : ""}
                      </td>
                      <td class="home-depth__num num">{m() ? deepText(r.deep) : ""}</td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
            <p class="home-depth__note">
              Bars on a log scale, from 100 vB to one block's worth, which fills a bar.
            </p>
          </details>
        </div>
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
