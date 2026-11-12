import { type Accessor, createEffect, createSignal, on, onCleanup, Show } from "solid-js";
import { SectionHeader } from "../../components/SectionHeader";
import { ErrorState } from "../../components/States";
import { fetchStamped, withRetry } from "../../lib/api";
import { useNow } from "../../lib/clock";
import { groupDigits } from "../../lib/format";
import { NETWORK_NAMES, type Network } from "../../lib/network";
import {
  change,
  type DifficultyAdjustment,
  isDifficultyAdjustment,
  PERIOD,
  paceText,
  periodText,
  retargets,
  retargetUtc,
  until,
} from "./difficulty";
import { Stat } from "./MempoolPanel";

/** The answer can trail a new tip by a moment; ask again this often, this many times. */
const BEHIND_RETRY_MS = 1_500;
const BEHIND_TRIES = 4;

/**
 * The difficulty adjustment: how far through its period the chain is, and what the next retarget
 * is expected to do. Fetched once, then again whenever the tip moves.
 */
export function DifficultyPanel(props: { network: Network; tip: Accessor<string | undefined> }) {
  return (
    <section class="home-difficulty" aria-labelledby="difficulty-title">
      <SectionHeader title="Difficulty adjustment" id="difficulty-title" />
      <Show
        when={retargets(props.network)}
        fallback={
          <p class="home-difficulty__note">
            {NETWORK_NAMES[props.network]} never adjusts its difficulty: every block is mined at the
            minimum, so there is no next adjustment to show.
          </p>
        }
      >
        <Adjustment network={props.network} tip={props.tip} />
      </Show>
    </section>
  );
}

function Adjustment(props: { network: Network; tip: Accessor<string | undefined> }) {
  const now = useNow();
  const [data, setData] = createSignal<DifficultyAdjustment>();
  const [error, setError] = createSignal<unknown>();
  let controller: AbortController | undefined;
  let behind: ReturnType<typeof setTimeout> | undefined;
  let behindTries = 0;
  /** The tip the shown answer is as of. */
  let answeredAt: string | undefined;

  const load = () => {
    controller?.abort();
    clearTimeout(behind);
    const c = new AbortController();
    controller = c;
    // A 503 before the server's first answer is retried as it says, without showing an error.
    withRetry(
      () => fetchStamped<unknown>(props.network, "/v1/difficulty-adjustment", { signal: c.signal }),
      c.signal,
    ).then(
      ({ value, tip }) => {
        if (c.signal.aborted) return;
        if (isDifficultyAdjustment(value)) {
          setData(value);
          setError(undefined);
        } else setError(new Error("The server sent a reply that could not be read."));
        answeredAt = tip?.hash;
        // The answer is as of an older tip than the strip shows: the server is still recomputing.
        const want = props.tip();
        if (want && tip && tip.hash !== want && behindTries < BEHIND_TRIES) {
          behindTries++;
          behind = setTimeout(load, BEHIND_RETRY_MS);
        } else behindTries = 0;
      },
      (e: unknown) => {
        if (!c.signal.aborted) setError(e);
      },
    );
  };
  load();
  // A new tip changes the answer. The first tip only arrives with the strip, after `load` above.
  createEffect(
    on(
      props.tip,
      (tip, previous) => {
        // The strip's first tip: ask again only if the answer already shown is as of another one.
        if (previous === undefined && (answeredAt === undefined || answeredAt === tip)) return;
        behindTries = 0;
        load();
      },
      { defer: true },
    ),
  );
  onCleanup(() => {
    controller?.abort();
    clearTimeout(behind);
  });

  const d = () => data();
  const expected = () => change(d()?.difficultyChange ?? 0);
  const previous = () => change(d()?.previousRetarget ?? 0);
  const progress = () => Math.min(1, Math.max(0, (d()?.progressPercent ?? 0) / 100));
  const pace = () => Math.min(1, Math.max(0, (d()?.expectedBlocks ?? 0) / PERIOD));

  return (
    <Show
      when={d() ?? error() === undefined}
      fallback={
        <ErrorState title="Could not load the difficulty adjustment" error={error()} retry={load} />
      }
    >
      <figure class="home-difficulty__period">
        <div class="home-difficulty__bar">
          <span
            class="home-difficulty__done"
            classList={{ "home-difficulty__done--loading": !d() }}
            style={{ "--p": String(progress()) }}
          />
          <Show when={d()}>
            <span class="home-difficulty__pace" style={{ "--p": String(pace()) }} />
          </Show>
        </div>
        <figcaption class="home-difficulty__caption">
          <span class="num home-difficulty__count">
            <Show when={d()}>{(a) => periodText(a())}</Show>
          </span>
          <span class="home-difficulty__pace-text">
            <Show when={d()}>{(a) => paceText(a())}</Show>
          </span>
        </figcaption>
      </figure>
      <dl class="home-stats home-difficulty__stats" aria-busy={!d()}>
        <Stat label="Expected change">
          <Show when={d()}>
            {(a) => (
              <>
                <span class="num">
                  <span class="home-difficulty__arrow" aria-hidden="true">
                    {expected().arrow}
                  </span>
                  {expected().text}
                </span>
                <span class="home-stat__sub num">
                  at block {groupDigits(a().nextRetargetHeight)}
                </span>
              </>
            )}
          </Show>
        </Stat>
        <Stat label="Next adjustment">
          <Show when={d()}>
            {(a) => (
              <>
                <span class="num">{until(a().estimatedRetargetDate, now() * 1000)}</span>
                <span class="home-stat__sub num">{retargetUtc(a().estimatedRetargetDate)}</span>
              </>
            )}
          </Show>
        </Stat>
        <Stat label="Previous change">
          <Show when={d()}>
            <span class="num">
              <span class="home-difficulty__arrow" aria-hidden="true">
                {previous().arrow}
              </span>
              {previous().text}
            </span>
          </Show>
        </Stat>
      </dl>
    </Show>
  );
}
