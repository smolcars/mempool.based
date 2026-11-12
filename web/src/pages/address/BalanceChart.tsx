import {
  createEffect,
  createMemo,
  createSignal,
  lazy,
  Loading as Pending,
  Match,
  onCleanup,
  Switch,
  untrack,
} from "solid-js";
import "../../components/SectionHeader.css";
import { Loading, ErrorState } from "../../components/States";
import { type ApiResource, fetchStamped, type Stamped } from "../../lib/api";
import type { AddressStats } from "../../lib/esplora";
import { formatBtc, formatUtc, groupDigits } from "../../lib/format";
import { useNetwork } from "../../lib/page";
import { refusal } from "./address";
import { advance, type History, loadHistory, points, readRows } from "./balance";
import { settled } from "./parts";

const Chart = lazy(() => import("../../components/chart/Chart"));

/** Plot height in px. `.address-balance__slot` holds it with the readout and caption: keep both. */
const HEIGHT = 200;

/** BTC without the zeros a round tick does not need: `0.5`, `1 200`. */
const btcTick = (sats: number) => formatBtc(Math.round(sats)).replace(/\.?0+$/, "");
const btc = (sats: number) => `${formatBtc(sats)} BTC`;
const signedBtc = (sats: number) => `${sats > 0 ? "+" : ""}${btc(sats)}`;
const plural = (n: number, one: string, many: string) =>
  `${groupDigits(n)} ${n === 1 ? one : many}`;

type Shown =
  | { kind: "loading" }
  | { kind: "ready"; history: History; now: number }
  | { kind: "failed"; error: unknown };

/**
 * The confirmed balance after each block, as a step line. Its requests start once the page's own
 * have answered, and its code with them; addresses with no history, or too much for the node to
 * add up, never load either. docs/FRONTEND.md "Address balance chart".
 */
export function BalanceChart(props: {
  address: string;
  stats: ApiResource<AddressStats>;
  /** The page's main content has answered, so the chart's requests come after it. */
  after: boolean;
  /** Hands the page a function that brings the chart up to date after a change. */
  onRefresh: (refresh: () => void) => void;
}) {
  const network = useNetwork();
  const base = `/address/${encodeURIComponent(untrack(() => props.address))}`;
  const [shown, setShown] = createSignal<Shown>({ kind: "loading" });
  const controller = new AbortController();
  onCleanup(() => controller.abort());
  const fetch = (path: string): Promise<Stamped<unknown>> =>
    fetchStamped<unknown>(network(), path, { signal: controller.signal });

  let history: History | undefined;
  let started = false;
  let busy = false;
  let again = false;
  /** The newest page joined to what is loaded, or everything again when they do not meet. */
  const update = async () => {
    const old = history;
    if (old === undefined) return loadHistory(base, fetch);
    const page = readRows((await fetch(`${base}/txs/summary`)).value);
    return advance(old, page) ?? loadHistory(base, fetch);
  };
  // One job at a time; a change heard meanwhile runs one update after it, never a second chain.
  const work = async () => {
    busy = true;
    try {
      history = await update();
      setShown({ kind: "ready", history, now: Math.floor(Date.now() / 1000) });
    } catch (error) {
      if (controller.signal.aborted) return;
      setShown({ kind: "failed", error });
    }
    busy = false;
    if (again) {
      again = false;
      void work();
    }
  };
  const refresh = () => {
    if (!started) return;
    if (busy) again = true;
    else void work();
  };
  props.onRefresh(refresh);

  const failure = () => (props.stats.state === "errored" ? refusal(props.stats.error) : undefined);
  const count = () => settled(props.stats)?.chain_stats.tx_count;
  const statsLoading = () => props.stats.state === "unresolved" || props.stats.state === "pending";
  createEffect(
    () => props.after && (count() ?? 0) > 0,
    (go) => {
      if (!go || started) return;
      started = true;
      void Chart.preload();
      void work();
    },
  );

  const view = createMemo(() => {
    const s = shown();
    if (s.kind !== "ready") return undefined;
    const pts = points(s.history);
    const first = pts[0];
    const last = pts.at(-1);
    if (first === undefined || last === undefined) return undefined;
    return { h: s.history, pts, end: Math.max(s.now, last.time), from: first.time };
  });
  const meta = () => {
    const v = view();
    if (!v) return "";
    const n = v.h.rows.length;
    return v.h.complete
      ? plural(n, "confirmed transaction", "confirmed transactions")
      : `The last ${groupDigits(n)} of ${plural(v.h.total, "confirmed transaction", "confirmed transactions")}`;
  };

  return (
    <Switch>
      <Match when={failure() === "too-large"}>
        <p class="address-balance-none">
          There is no balance chart for this address: it has more history than this server's node
          will add up.
        </p>
      </Match>
      <Match when={statsLoading() || (count() ?? 0) > 0}>
        <section class="address-balance" aria-labelledby="address-balance-title">
          <div class="section__head">
            <h2 id="address-balance-title">Balance over time</h2>
            <div class="section__meta">{meta()}</div>
          </div>
          <div class="address-balance__slot">
            <Switch>
              <Match when={view()}>
                {(v) => (
                  <Pending>
                    <Chart
                      label={
                        v().h.complete
                          ? "Confirmed balance after each block"
                          : `Confirmed balance after each block, the last ${groupDigits(v().h.rows.length)} transactions`
                      }
                      x={v().pts.map((p) => p.time)}
                      domain={[v().from, v().end]}
                      gap={Number.POSITIVE_INFINITY}
                      height={HEIGHT}
                      // A whole history rose from nothing; a partial one starts where it stood.
                      left={{ format: btcTick, zero: v().h.complete }}
                      series={[
                        {
                          name: "Balance",
                          values: v().pts.map((p) => p.balance),
                          draw: "step",
                          until: v().end,
                          format: btc,
                        },
                      ]}
                      notes={[
                        {
                          name: "Block",
                          values: v().pts.map((p) => p.height),
                          format: groupDigits,
                        },
                        { name: "Change", values: v().pts.map((p) => p.change), format: signedBtc },
                      ]}
                      time={formatUtc}
                    />
                  </Pending>
                )}
              </Match>
              <Match when={shown().kind === "failed"}>
                <ErrorState
                  title="Could not load the balance history"
                  error={(shown() as { error: unknown }).error}
                  retry={() => {
                    setShown({ kind: "loading" });
                    refresh();
                  }}
                />
              </Match>
              <Match when={shown().kind === "ready"}>
                <p class="muted">No confirmed transactions yet.</p>
              </Match>
              <Match when={true}>
                <Loading label="Loading the balance history" />
              </Match>
            </Switch>
          </div>
        </section>
      </Match>
    </Switch>
  );
}
