import { createSignal, For, lazy, Loading, Show } from "solid-js";
import { Amount } from "../Amount";
import { DataTable } from "../DataTable";
import { SectionHeader } from "../SectionHeader";
import { Loading as Waiting } from "../States";
import { formatSize, groupDigits } from "../../lib/format";
import { type AuditTotals, CATEGORIES, formatMatchRate, type Kind } from "./model";
import "../Button.css";
import "../More.css";
import "./Audit.css";

// The list of transactions loads only when the reader asks for it.
const AuditList = lazy(() => import("./AuditList"));

const EXPECTED_ONLY: readonly Kind[] = ["displaced", "fresh", "conflicted", "missing"];
const MINED_ONLY: readonly Kind[] = ["added", "unseen"];

const txCount = (n: number) => `${groupDigits(n)} ${n === 1 ? "transaction" : "transactions"}`;

/**
 * A block's audit on its page: the match rate, the block we expected against the one mined, and
 * what fell in each category. `audit` is undefined while the block's facts load and null when
 * there is no audit.
 */
export function AuditSection(props: {
  hash: string | undefined;
  audit: AuditTotals | null | undefined;
  error?: boolean;
  retry?: () => void;
}) {
  return (
    <section class="audit" aria-labelledby="audit-title">
      <SectionHeader title="Audit" id="audit-title" />
      <Show
        when={props.audit}
        fallback={
          <p class="audit__none">
            <Show
              when={props.audit === null}
              fallback={
                <Show when={props.error} fallback={<span class="audit__wait" aria-hidden="true" />}>
                  The audit could not load.{" "}
                  <button type="button" class="audit__retry" onClick={() => props.retry?.()}>
                    Try again
                  </button>
                </Show>
              }
            >
              Not audited: we were not watching when this block arrived, so there is no block we
              expected to compare it with.
            </Show>
          </p>
        }
      >
        {(a) => <Body audit={a()} hash={props.hash} />}
      </Show>
    </section>
  );
}

function Body(props: { audit: AuditTotals; hash: string | undefined }) {
  const a = () => props.audit;
  const listed = () => [...EXPECTED_ONLY, ...MINED_ONLY].some((k) => a()[k].txs > 0);
  // On a phone the kinds' meanings wait behind one button: their counts stay in sight.
  const [says, setSays] = createSignal(false);
  const rows = () => [
    {
      label: "Transactions",
      expected: groupDigits(a().expected.txs),
      mined: groupDigits(a().actual.txs),
    },
    {
      label: "Virtual size",
      expected: formatSize(a().expected.vsize),
      mined: formatSize(a().actual.vsize),
    },
    {
      label: "Fees, BTC",
      expected: <Amount sats={a().expected.fees} unit={false} />,
      mined: <Amount sats={a().actual.fees} unit={false} />,
    },
  ];
  type Row = ReturnType<typeof rows>[number];
  const columns = [
    { header: "Measure", hidden: true, cell: (r: Row) => r.label },
    { header: "Expected", numeric: true, cell: (r: Row) => r.expected },
    { header: "Mined", numeric: true, cell: (r: Row) => r.mined },
  ];
  return (
    <div class="audit__body">
      <p class="audit__intro">
        When this block was found, we compared it with the block we expected next: the best-paying
        transactions in our own mempool that would fit.
      </p>
      <div class="audit__top">
        <div class="audit__score">
          <span class="audit__rate num">
            {formatMatchRate(a().match_rate)}
            <span class="audit__pct">%</span>
          </span>
          <span class="audit__rate-label">Match rate</span>
          <span class="audit__rate-says">
            Of the block we expected, by size, the share the miner mined or had no room for, leaving
            aside transactions too new or in conflict.
          </span>
        </div>
        <div class="audit__compare">
          <DataTable
            caption="The block we expected and the block mined"
            columns={columns}
            rows={rows()}
          />
          <p class="audit__note">Both leave out the coinbase.</p>
        </div>
      </div>
      <Bars audit={a()} />
      <div id="audit-kinds" class="audit__kinds" data-says={says() ? "" : undefined}>
        <Kinds audit={a()} kinds={["matched"]} />
        <div class="audit__side">
          <h3 class="audit__side-title">Expected, not mined</h3>
          <Kinds audit={a()} kinds={EXPECTED_ONLY} />
        </div>
        <div class="audit__side">
          <h3 class="audit__side-title">Mined, not expected</h3>
          <Kinds audit={a()} kinds={MINED_ONLY} />
        </div>
      </div>
      <button
        type="button"
        class="more__summary audit__says"
        aria-expanded={says() ? "true" : "false"}
        aria-controls="audit-kinds"
        onClick={() => setSays(!says())}
      >
        {says() ? "Hide what each kind means" : "What each kind means"}
      </button>
      <Show when={props.hash && listed()}>
        <Details hash={props.hash ?? ""} audit={a()} />
      </Show>
    </div>
  );
}

/** The two blocks laid side by side, their matched parts lined up: what only one side had sticks out. */
function Bars(props: { audit: AuditTotals }) {
  const sum = (kinds: readonly Kind[]) => kinds.reduce((s, k) => s + props.audit[k].vsize, 0);
  const total = () => sum(EXPECTED_ONLY) + props.audit.matched.vsize + sum(MINED_ONLY);
  const share = (v: number) => String(total() > 0 ? v / total() : 0);
  // Each piece is placed at its own share, so a sliver held at its minimum width overlaps its
  // neighbour instead of pushing the matched piece out of line with the other lane's.
  const Row = (p: { label: string; kinds: readonly Kind[]; offset?: number }) => {
    const pieces = () => {
      let x = p.offset ?? 0;
      return p.kinds
        .filter((k) => props.audit[k].vsize > 0)
        .map((k) => {
          const piece = { kind: k, x };
          x += props.audit[k].vsize;
          return piece;
        });
    };
    return (
      <div class="audit-bars__row">
        <span class="audit-bars__label">{p.label}</span>
        <span class="audit-bars__track">
          <span class="audit-bars__lane">
            <For each={pieces()}>
              {(piece) => (
                <span
                  class={`audit-bars__seg audit-kind--${piece.kind}`}
                  style={{
                    "--x": share(piece.x),
                    "--w": share(props.audit[piece.kind].vsize),
                  }}
                />
              )}
            </For>
          </span>
        </span>
      </div>
    );
  };
  return (
    <Show when={total() > 0}>
      <div class="audit-bars" aria-hidden="true">
        <Row label="Expected" kinds={["missing", "conflicted", "fresh", "displaced", "matched"]} />
        <Row label="Mined" kinds={["matched", ...MINED_ONLY]} offset={sum(EXPECTED_ONLY)} />
      </div>
    </Show>
  );
}

function Kinds(props: { audit: AuditTotals; kinds: readonly Kind[] }) {
  return (
    <ul class="audit-kinds">
      <For each={CATEGORIES.filter((c) => props.kinds.includes(c.kind))}>
        {(c) => {
          const part = () => props.audit[c.kind];
          return (
            <li class={["audit-kinds__item", { "audit-kinds__item--none": part().txs === 0 }]}>
              <span class={`audit-kinds__swatch audit-kind--${c.kind}`} aria-hidden="true" />
              <span class="audit-kinds__name">{c.name}</span>
              <span class="audit-kinds__figures num">
                <Show when={part().txs > 0} fallback="None">
                  {/* A phone may break the line between the count and the size, never inside one. */}
                  <span class="audit-kinds__part">{txCount(part().txs)},</span>{" "}
                  <span class="audit-kinds__part">{formatSize(part().vsize)}</span>
                </Show>
              </span>
              <span class="audit-kinds__says">{c.says}</span>
            </li>
          );
        }}
      </For>
    </ul>
  );
}

function Details(props: { hash: string; audit: AuditTotals }) {
  const [open, setOpen] = createSignal(false);
  return (
    <div class="audit__details">
      <button
        type="button"
        class="btn"
        aria-expanded={open() ? "true" : "false"}
        aria-controls="audit-list"
        onClick={() => setOpen((o) => !o)}
      >
        {open() ? "Hide the transactions" : "Show the transactions that differ"}
      </button>
      <div id="audit-list">
        <Show when={open()}>
          <Loading fallback={<Waiting label="Loading the list" />}>
            <AuditList hash={props.hash} audit={props.audit} />
          </Loading>
        </Show>
      </div>
    </div>
  );
}
