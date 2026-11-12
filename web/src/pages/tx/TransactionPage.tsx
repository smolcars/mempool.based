import { useParams } from "@solidjs/router";
import {
  createEffect,
  createMemo,
  createSignal,
  For,
  type JSX,
  Match,
  on,
  onCleanup,
  Show,
  Switch,
} from "solid-js";
import { Amount, Sats } from "../../components/Amount";
import { FeeRate } from "../../components/FeeRate";
import { HashFull } from "../../components/Hash";
import { Link } from "../../components/Link";
import { SectionHeader } from "../../components/SectionHeader";
import { Empty, ErrorState } from "../../components/States";
import { TimeAgo, TimeUtc } from "../../components/Time";
import { createApi, fetchApi, isNotFound } from "../../lib/api";
import type { Outspend, Transaction, TxStatus } from "../../lib/esplora";
import { formatSize, groupDigits, MISSING, truncateMiddle } from "../../lib/format";
import { NETWORK_NAMES, paths } from "../../lib/network";
import { useNetwork, usePageTitle } from "../../lib/page";
import { Code } from "./Code";
import { Flow, type Selection } from "./Flow";
import { Inspector } from "./Inspector";
import { confirmations, features, isCoinbase, isTxid, lockTimeMeaning, vsize } from "./tx";
import "./TransactionPage.css";

/** How often an unconfirmed transaction, or a young confirmed one, is checked while the tab is visible. */
export const POLL_MS = 10_000;
/** Confirmations after which most services treat a payment as final, and a coinbase's maturity. */
const SETTLED = 6;
const MATURE = 100;

/** "Removed": it was unconfirmed and the node no longer knows it (replaced or dropped). */
type Live = TxStatus | "removed";

export default function TransactionPage() {
  const params = useParams<{ txid: string }>();
  const network = useNetwork();
  const id = () => (isTxid(params.txid) ? params.txid.toLowerCase() : null);
  usePageTitle(() => (id() ? `Transaction ${truncateMiddle(id() ?? "")}` : "Transaction"));
  return (
    <div class="wrap tx-page">
      <Show
        when={id() ? `${network()}/${id()}` : null}
        keyed
        fallback={<BadInput value={params.txid} />}
      >
        {(key) => <TransactionView txid={key.slice(key.indexOf("/") + 1)} />}
      </Show>
    </div>
  );
}

/** A path segment as the reader typed it; left encoded if it does not decode. */
function decoded(segment: string): string {
  try {
    return decodeURIComponent(segment);
  } catch {
    return segment;
  }
}

function BadInput(props: { value: string }) {
  return (
    <>
      <header class="tx-head">
        <h1>Transaction</h1>
      </header>
      <Empty title="That is not a transaction ID">
        <p>
          “{truncateMiddle(decoded(props.value), 24, 8)}” is not one. A transaction ID is 64
          characters, each a digit or a letter from a to f. Check the link, or search for it again.
        </p>
      </Empty>
    </>
  );
}

function TransactionView(props: { txid: string }) {
  const network = useNetwork();
  const [tx, { refetch }] = createApi<Transaction>(() => `/tx/${props.txid}`);
  // Fetched alongside the transaction rather than after it, so neither waits on the other.
  const [outspends] = createApi<Outspend[]>(() => `/tx/${props.txid}/outspends`);
  const [tipText] = createApi<string>(() => "/blocks/tip/height", { as: "text" });

  const ready = () => (tx.state === "ready" ? tx() : undefined);
  const spends = () => (outspends.state === "ready" ? outspends() : undefined);

  const [live, setLive] = createSignal<Live>();
  const [polledTip, setPolledTip] = createSignal<number>();
  const [announcement, setAnnouncement] = createSignal("");
  const [justConfirmed, setJustConfirmed] = createSignal(false);
  createEffect(on(ready, (t) => t && setLive(t.status)));

  const tip = () => {
    const polled = polledTip();
    if (polled !== undefined) return polled;
    const t = tipText.state === "ready" ? Number(tipText()) : Number.NaN;
    return Number.isSafeInteger(t) ? t : undefined;
  };
  const status = () => {
    const l = live();
    return l && l !== "removed" ? l : undefined;
  };
  const confs = () => confirmations(status()?.block_height, tip());

  // shortcut: polls /tx/:txid/status and the tip every POLL_MS; replace with the /app/ws WebSocket when it lands.
  // A memo, so the effect below restarts only when watching starts or stops, not on every tip.
  const watching = createMemo(() => {
    const l = live();
    if (!l || l === "removed") return false;
    return (
      !l.confirmed ||
      (confs() ?? 0) < (ready() && isCoinbase(ready() as Transaction) ? MATURE : SETTLED)
    );
  });

  createEffect(() => {
    if (!watching()) return;
    const controller = new AbortController();
    let busy = false;
    const tick = async () => {
      if (busy || document.visibilityState !== "visible") return;
      busy = true;
      try {
        const now = live();
        if (now && now !== "removed" && !now.confirmed) {
          try {
            const next = await fetchApi<TxStatus>(network(), `/tx/${props.txid}/status`, {
              signal: controller.signal,
            });
            if (next.confirmed) {
              setJustConfirmed(true);
              setLive(next);
              setAnnouncement(
                `Confirmed in block ${groupDigits(next.block_height ?? Number.NaN)}.`,
              );
            }
          } catch (e) {
            if (!isNotFound(e)) throw e;
            setLive("removed");
            setAnnouncement("This transaction is no longer in the mempool.");
            return;
          }
        }
        const height = await fetchApi<string>(network(), "/blocks/tip/height", {
          as: "text",
          signal: controller.signal,
        });
        if (Number.isSafeInteger(Number(height))) setPolledTip(Number(height));
      } catch {
        // A failed check is tried again on the next tick; the page keeps what it shows.
      } finally {
        busy = false;
      }
    };
    const timer = setInterval(tick, POLL_MS);
    const onVisible = () => void tick();
    document.addEventListener("visibilitychange", onVisible);
    onCleanup(() => {
      clearInterval(timer);
      controller.abort();
      document.removeEventListener("visibilitychange", onVisible);
    });
  });

  return (
    <>
      <header class="tx-head">
        <div class="tx-head__row">
          <h1>Transaction</h1>
          <Show
            when={live()}
            fallback={!tx.error && <span class="tx-status tx-status--pending" aria-hidden="true" />}
          >
            {(l) => <StatusBadge live={l()} />}
          </Show>
        </div>
        <HashFull value={props.txid} copyLabel="Copy transaction ID" />
      </header>
      <p class="visually-hidden" aria-live="polite">
        {announcement()}
      </p>
      <Switch fallback={<Skeleton />}>
        <Match when={tx.error}>
          <Show
            when={isNotFound(tx.error)}
            fallback={
              <ErrorState
                title="Could not load this transaction"
                error={tx.error}
                retry={refetch}
              />
            }
          >
            <Empty title="Transaction not found">
              <p>
                The {NETWORK_NAMES[network()]} node does not know this transaction. It may not have
                been broadcast yet, it may have left the mempool after being replaced or dropped, or
                it may belong to another network.
              </p>
            </Empty>
          </Show>
        </Match>
        <Match when={ready()}>
          {(t) => (
            <Body
              tx={t()}
              live={live() ?? t().status}
              tip={tip()}
              confs={confs()}
              spends={spends}
              justConfirmed={justConfirmed()}
            />
          )}
        </Match>
      </Switch>
    </>
  );
}

const Icon = (props: { tone: "positive" | "caution" | "negative" }) => (
  <svg class="tx-icon" viewBox="0 0 16 16" aria-hidden="true">
    {props.tone === "positive" ? (
      <path d="M4.5 8.5l2.4 2.4L11.8 5.6" />
    ) : props.tone === "caution" ? (
      <circle cx="8" cy="8" r="5.5" stroke-dasharray="2.6 2" />
    ) : (
      <path d="M5 5l6 6M11 5l-6 6" />
    )}
  </svg>
);

function StatusBadge(props: { live: Live }) {
  const tone = () =>
    props.live === "removed" ? "negative" : props.live.confirmed ? "positive" : "caution";
  const word = () =>
    props.live === "removed" ? "Removed" : props.live.confirmed ? "Confirmed" : "Unconfirmed";
  return (
    <span class={`tx-status tx-status--${tone()}`}>
      <Icon tone={tone()} />
      {word()}
    </span>
  );
}

interface Step {
  title: string;
  detail: JSX.Element;
  state: "done" | "current" | "todo" | "dead";
  tone?: "positive" | "caution" | "negative";
}

function Body(props: {
  tx: Transaction;
  live: Live;
  tip: number | undefined;
  confs: number | undefined;
  spends: () => Outspend[] | undefined;
  justConfirmed: boolean;
}) {
  const network = useNetwork();
  const coinbase = () => isCoinbase(props.tx);
  const status = () => (props.live === "removed" ? undefined : props.live);
  const confirmed = () => status()?.confirmed === true;
  const [selected, setSelected] = createSignal<Selection | null>(null);
  let inspectorHeading: HTMLHeadingElement | undefined;

  const select = (s: Selection) => {
    const cur = selected();
    if (cur && cur.side === s.side && cur.index === s.index) {
      setSelected(null);
      return;
    }
    setSelected(s);
    const reduce = matchMedia("(prefers-reduced-motion: reduce)").matches;
    requestAnimationFrame(() => {
      inspectorHeading?.scrollIntoView({ block: "nearest", behavior: reduce ? "auto" : "smooth" });
      inspectorHeading?.focus({ preventScroll: true });
    });
  };

  const steps = (): Step[] => {
    const s = status();
    const block = (st: TxStatus) => (
      <>
        <span>
          In{" "}
          <Link href={paths.block(st.block_hash ?? st.block_height ?? "")}>
            block {groupDigits(st.block_height ?? Number.NaN)}
          </Link>
        </span>
        <Show when={st.block_time}>
          {(t) => (
            <>
              <TimeUtc time={t()} />
              <TimeAgo time={t()} />
            </>
          )}
        </Show>
      </>
    );
    const target = coinbase() ? MATURE : SETTLED;
    const third: Step = (() => {
      const title = coinbase() ? "Spendable" : "Six confirmations";
      if (props.live === "removed") return { title, detail: "", state: "dead" };
      if (!s?.confirmed)
        return { title, detail: "Most services treat six confirmations as final.", state: "todo" };
      const c = props.confs;
      if (c === undefined) return { title, detail: "", state: "current" };
      const count = `${groupDigits(c)} confirmation${c === 1 ? "" : "s"}`;
      if (c >= target)
        return {
          title,
          detail: coinbase() ? `${count}: its outputs can be spent.` : `${count}.`,
          state: "done",
          tone: "positive",
        };
      return {
        title,
        detail: coinbase()
          ? `${count} of the ${MATURE} a miner's reward needs before it can be spent.`
          : `${count} of ${SETTLED} so far.`,
        state: "current",
      };
    })();
    const first: Step = coinbase()
      ? {
          title: "Created",
          detail: "A miner's reward, made with the block that holds it.",
          state: "done",
        }
      : {
          title: "Broadcast",
          detail:
            s && !s.confirmed
              ? "Relayed to the network; waiting in the mempool."
              : "Relayed to the network.",
          state: "done",
        };
    const second: Step =
      props.live === "removed"
        ? {
            title: "No longer in the mempool",
            detail:
              "Replaced by a transaction spending the same coins, or dropped. It will not confirm unless it is sent again.",
            state: "dead",
            tone: "negative",
          }
        : s?.confirmed
          ? { title: "Confirmed", detail: block(s), state: "done", tone: "positive" }
          : {
              title: "Waiting for a block",
              detail: "This page updates when it confirms.",
              state: "current",
              tone: "caution",
            };
    return [first, second, third];
  };

  const v = () => vsize(props.tx);
  const rate = () => {
    const size = v();
    return !coinbase() && props.tx.fee !== undefined && size ? props.tx.fee / size : undefined;
  };
  const lock = () => lockTimeMeaning(props.tx.locktime, props.tx.vin);
  const [raw, setRaw] = createSignal<{ hex?: string; error?: unknown; loading?: boolean }>({});
  const loadRaw = async () => {
    setRaw({ loading: true });
    try {
      setRaw({
        hex: await fetchApi<string>(network(), `/tx/${props.tx.txid}/hex`, { as: "text" }),
      });
    } catch (e) {
      setRaw({ error: e });
    }
  };

  return (
    <>
      <ol class="tx-timeline" aria-label="Status">
        <For each={steps()}>
          {(step, i) => (
            <li
              class={`tx-step tx-step--${step.state}${step.tone ? ` tx-step--${step.tone}` : ""}`}
              classList={{
                "tx-step--landed": props.justConfirmed && i() === 1 && step.tone === "positive",
              }}
              aria-current={step.state === "current" ? "step" : undefined}
            >
              <span class="tx-step__marker num" aria-hidden="true">
                {i() + 1}
              </span>
              <span class="tx-step__title">{step.title}</span>
              <span class="tx-step__detail">{step.detail}</span>
            </li>
          )}
        </For>
      </ol>

      <div class="tx-layout">
        <section class="tx-layout__flow" aria-labelledby="tx-flow-title">
          <SectionHeader title="Inputs and outputs" id="tx-flow-title">
            <span class="num">
              {groupDigits(props.tx.vin?.length ?? 0)} input{props.tx.vin?.length === 1 ? "" : "s"},{" "}
              {groupDigits(props.tx.vout.length)} output{props.tx.vout.length === 1 ? "" : "s"}
            </span>
          </SectionHeader>
          <Show when={props.tx.pruned}>
            <p class="tx-note">
              This node has pruned the inputs of transactions this old, so some are missing.
            </p>
          </Show>
          <Flow tx={props.tx} outspends={props.spends} selected={selected()} onSelect={select} />
          <Show when={selected()}>
            {(s) => (
              <Inspector
                tx={props.tx}
                selected={s()}
                outspends={props.spends}
                onClose={() => setSelected(null)}
                ref={(el) => {
                  inspectorHeading = el;
                }}
              />
            )}
          </Show>
        </section>

        <section class="tx-layout__fee" aria-labelledby="tx-fee-title">
          <SectionHeader title="Fee" id="tx-fee-title" />
          <dl class="tx-facts tx-facts--single">
            <div class="tx-fact">
              <dt>Fee rate</dt>
              <dd>
                <Show
                  when={rate() !== undefined}
                  fallback={coinbase() ? "None: a coinbase pays no fee" : MISSING}
                >
                  <FeeRate rate={rate() ?? 0} />
                </Show>
              </dd>
            </div>
            <div class="tx-fact">
              <dt>Fee</dt>
              <dd>
                <Show
                  when={!coinbase() && props.tx.fee !== undefined}
                  fallback={coinbase() ? "None" : MISSING}
                >
                  <Sats sats={props.tx.fee ?? 0} />
                  <span class="tx-fact__sub">
                    <Amount sats={props.tx.fee ?? 0} />
                  </span>
                </Show>
              </dd>
            </div>
            <div class="tx-fact">
              <dt>Virtual size</dt>
              <dd class="num">{v() === undefined ? MISSING : formatSize(v() ?? 0)}</dd>
            </div>
            <div class="tx-fact">
              <dt>Weight</dt>
              <dd class="num">
                {props.tx.weight === undefined ? MISSING : formatSize(props.tx.weight, "WU")}
              </dd>
            </div>
            <div class="tx-fact">
              <dt>Size</dt>
              <dd class="num">
                {props.tx.size === undefined ? MISSING : formatSize(props.tx.size, "B")}
              </dd>
            </div>
            <div class="tx-fact">
              <dt>Replaceable</dt>
              <dd>
                <Switch>
                  <Match when={props.live === "removed"}>No: it is no longer in the mempool</Match>
                  <Match when={confirmed()}>No: it is confirmed</Match>
                  <Match when={true}>
                    Yes, until it confirms
                    <span class="tx-fact__sub">
                      Nodes relay a replacement that pays more, whatever this transaction's sequence
                      numbers say.
                    </span>
                  </Match>
                </Switch>
              </dd>
            </div>
          </dl>
          {/* Reserved for the projected position in the mempool, which needs the mempool engine's data. */}
          <Show when={!props.tx.status.confirmed}>
            <div class="tx-position-slot" aria-hidden="true" />
          </Show>
        </section>

        <section class="tx-layout__details" aria-labelledby="tx-details-title">
          <SectionHeader title="Details" id="tx-details-title" />
          <dl class="tx-facts tx-facts--two">
            <div class="tx-fact">
              <dt>Version</dt>
              <dd class="num">{props.tx.version}</dd>
            </div>
            <div class="tx-fact">
              <dt>Witness</dt>
              <dd>{features(props.tx)}</dd>
            </div>
            <div class="tx-fact tx-fact--wide">
              <dt>Lock time</dt>
              <dd>
                <span class="num">{lock().value}</span>
                <span class="tx-fact__sub">{lock().detail}</span>
              </dd>
            </div>
            <div class="tx-fact tx-fact--wide">
              <dt>Raw transaction</dt>
              <dd>
                <Switch>
                  <Match when={raw().hex}>
                    {(hex) => <Code label="Raw hex" value={hex()} bytes={hex().length / 2} />}
                  </Match>
                  <Match when={raw().error}>
                    <ErrorState
                      title="Could not load the raw transaction"
                      error={raw().error}
                      retry={loadRaw}
                    />
                  </Match>
                  <Match when={true}>
                    <button class="btn" type="button" onClick={loadRaw} disabled={raw().loading}>
                      {raw().loading ? "Loading raw hex…" : "Show raw hex"}
                    </button>
                    <span class="tx-fact__sub num">
                      {props.tx.size === undefined ? "" : formatSize(props.tx.size, "B")}
                    </span>
                  </Match>
                </Switch>
              </dd>
            </div>
          </dl>
        </section>
      </div>
    </>
  );
}

/** Placeholders shaped like the page, so nothing moves when the transaction arrives. */
function Skeleton() {
  return (
    <div class="tx-skeleton" role="status">
      <span class="visually-hidden">Loading transaction</span>
      <ol class="tx-timeline" aria-hidden="true">
        <For each={[0, 1, 2]}>
          {() => (
            <li class="tx-step tx-step--todo">
              <span class="tx-step__marker" />
              <span class="tx-step__title">
                <span class="tx-bar tx-bar--title" />
              </span>
              <span class="tx-step__detail">
                <span class="tx-bar" />
              </span>
            </li>
          )}
        </For>
      </ol>
      <div class="tx-layout" aria-hidden="true">
        <div class="tx-layout__flow">
          <div class="section__head">
            <span class="tx-bar tx-bar--head" />
          </div>
          <div class="tx-flow">
            <ul class="tx-flow__col">
              <li class="tx-flow__item tx-flow__item--ghost" />
            </ul>
            <span />
            <ul class="tx-flow__col">
              <li class="tx-flow__item tx-flow__item--ghost" />
              <li class="tx-flow__item tx-flow__item--ghost" />
            </ul>
          </div>
        </div>
        <div class="tx-layout__fee">
          <div class="section__head">
            <span class="tx-bar tx-bar--head" />
          </div>
          <div class="tx-facts tx-facts--single">
            <For each={[0, 1, 2, 3, 4, 5]}>
              {() => (
                <div class="tx-fact">
                  <span class="tx-bar" />
                </div>
              )}
            </For>
          </div>
        </div>
      </div>
    </div>
  );
}
