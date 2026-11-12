import { useParams } from "@solidjs/router";
import { createEffect, createMemo, createSignal, Show } from "solid-js";
import { Amount } from "../../components/Amount";
import { formatMatchRate, readAudit } from "../../components/audit/model";
import { DataTable } from "../../components/DataTable";
import { FeeRate } from "../../components/FeeRate";
import { Hash } from "../../components/Hash";
import { Link } from "../../components/Link";
import { ErrorState } from "../../components/States";
import { TimeAgo } from "../../components/Time";
import { createApi } from "../../lib/api";
import type { Block } from "../../lib/esplora";
import { formatSize, groupDigits, MISSING, NNBSP, truncateMiddle } from "../../lib/format";
import { paths } from "../../lib/network";
import { useNetwork, usePageTitle } from "../../lib/page";
import { type BlockSummary, fetchSummaries, knownSummary } from "../../lib/summary";
import { fullness, parseHeight, shownParam } from "../block/chain";
import { failed, ready, Skel } from "../block/parts";
import "./BlocksPage.css";

/** The API answers ten blocks per page. */
const PAGE = 10;
/** Summary requests in flight at once on this page. */
const IN_FLIGHT = 2;

/** A row's fee facts: here, still loading, or given up on. */
type RowFacts = BlockSummary | "failed" | undefined;

const columns = (factsOf: (id: string) => RowFacts) => [
  {
    header: "Height",
    cell: (b: Block | null) =>
      b ? (
        <Link class="blocks-height num" href={paths.block(b.id)}>
          {groupDigits(b.height)}
        </Link>
      ) : (
        <Skel size="s" />
      ),
  },
  {
    header: "Hash",
    optional: true,
    cell: (b: Block | null) =>
      b ? (
        <Hash value={b.id} href={paths.block(b.id)} copyLabel="Copy block hash" />
      ) : (
        <span class="blocks-hash-skel">
          <Skel />
        </span>
      ),
  },
  {
    header: "Mined",
    cell: (b: Block | null) => (b ? <TimeAgo time={b.timestamp} /> : <Skel size="s" />),
  },
  {
    header: "Transactions",
    numeric: true,
    optional: true,
    cell: (b: Block | null) => (b ? groupDigits(b.tx_count) : <Skel size="s" />),
  },
  {
    header: "Median fee rate, sat/vB",
    numeric: true,
    cell: (b: Block | null) => {
      const f = b ? factsOf(b.id) : undefined;
      if (f === undefined) return <Skel size="s" />;
      if (f === "failed") return MISSING;
      return f.fee_rate ? <FeeRate rate={f.fee_rate.median} unit={false} /> : "None";
    },
  },
  // Blank for a block without an audit: most old blocks, and never an error.
  {
    header: "Match rate, %",
    numeric: true,
    cell: (b: Block | null) => {
      const f = b ? factsOf(b.id) : undefined;
      if (f === undefined) return <Skel size="s" />;
      const a = f === "failed" ? null : readAudit(f.audit);
      return a ? (
        <>
          {formatMatchRate(a.match_rate)}
          <span class="visually-hidden">%</span>
        </>
      ) : (
        ""
      );
    },
  },
  {
    header: "Total fees, BTC",
    numeric: true,
    optional: true,
    cell: (b: Block | null) => {
      const f = b ? factsOf(b.id) : undefined;
      if (f === undefined) return <Skel size="s" />;
      return f === "failed" ? MISSING : <Amount sats={f.fees} unit={false} />;
    },
  },
  // Sizes carry their unit in the cell, as on the block page: they span B to MB (docs/DESIGN.md).
  {
    header: "Size",
    numeric: true,
    optional: true,
    cell: (b: Block | null) => (b ? formatSize(b.size, "B") : <Skel size="s" />),
  },
  {
    header: "Weight",
    numeric: true,
    optional: true,
    cell: (b: Block | null) =>
      b ? (
        <span class="blocks-weight" title={`${Math.round(fullness(b.weight) * 100)}% of the limit`}>
          <span class="blocks-weight__bar" aria-hidden="true">
            <span class="blocks-weight__fill" style={{ "--fill": String(fullness(b.weight)) }} />
          </span>
          {formatSize(b.weight, "WU")}
        </span>
      ) : (
        <Skel size="s" />
      ),
  },
];

/** `/blocks` and `/blocks/:from`: recent blocks, newest first, ten to a page. */
export default function BlocksPage() {
  const params = useParams<{ from?: string }>();
  const from = createMemo(() => (params.from === undefined ? undefined : parseHeight(params.from)));
  const [blocks, blocksCtl] = createApi<Block[]>(() => {
    const f = from();
    return f !== null && (f === undefined ? "/blocks" : `/blocks/${f}`);
  });
  const [tip] = createApi<string>(() => "/blocks/tip/height", { as: "text" });
  usePageTitle(() => "Blocks");
  const network = useNetwork();

  // Each row's fee facts on its own request, two at a time, so an old page, whose blocks the server
  // must compute one by one, fills in row by row without spending the client's whole allowance at
  // once; busy replies (503, 429) are retried as they say.
  const [facts, setFacts] = createSignal<ReadonlyMap<string, BlockSummary | "failed">>(new Map());
  const put = (id: string, f: BlockSummary | "failed") => setFacts((m) => new Map([...m, [id, f]]));
  const factsOf = (id: string): RowFacts => facts().get(id) ?? knownSummary(network(), id);
  const table = columns(factsOf);

  const list = () => {
    const l = ready(blocks);
    return Array.isArray(l) ? l : undefined;
  };
  createEffect(list, (rows) => {
    const c = new AbortController();
    const n = network();
    const queue = (rows ?? [])
      .map((b) => b.id)
      .filter((id) => !knownSummary(n, id) && !(facts().get(id) instanceof Object));
    const worker = async () => {
      for (let id = queue.shift(); id !== undefined && !c.signal.aborted; id = queue.shift()) {
        try {
          // oxlint-disable-next-line no-await-in-loop -- one request at a time per worker
          const found = await fetchSummaries(n, [id], c.signal);
          put(id, found.get(id) ?? "failed");
        } catch {
          if (!c.signal.aborted) put(id, "failed");
        }
      }
    };
    for (let i = 0; i < IN_FLIGHT; i++) void worker();
    return () => c.abort();
  });
  const tipHeight = () => {
    const t = Number(ready(tip));
    return Number.isSafeInteger(t) ? t : undefined;
  };
  const newest = () => list()?.[0]?.height;
  const oldest = () => list()?.at(-1)?.height;
  const atTip = () => {
    const n = newest();
    const t = tipHeight();
    return from() === undefined || n === undefined || (t !== undefined && n >= t);
  };
  const newerHref = () => {
    const n = newest();
    const t = tipHeight();
    if (n === undefined || t === undefined || n + PAGE >= t) return paths.blocks();
    return paths.blocks(n + PAGE);
  };

  return (
    <div class="wrap blocks-page">
      <header class="blocks-head">
        <h1>Blocks</h1>
        <p class="blocks-head__range num" aria-live="polite">
          <Show when={list()?.length} fallback={<Skel size="l" />}>
            Newest first: {groupDigits(newest() ?? 0)} to {groupDigits(oldest() ?? 0)}
          </Show>
        </p>
      </header>
      <Show
        when={from() !== null}
        fallback={
          <p class="blocks-message">
            “{truncateMiddle(shownParam(params.from ?? ""), 40, 20)}” is not a block height. Show
            the <Link href={paths.blocks()}>newest blocks</Link>.
          </p>
        }
      >
        <Show
          when={failed(blocks) === undefined}
          fallback={
            <ErrorState
              title="Could not load the blocks"
              error={failed(blocks)}
              retry={() => blocksCtl.refetch()}
            />
          }
        >
          <DataTable
            caption="Recent blocks, newest first"
            columns={table}
            rows={list() ?? Array.from({ length: PAGE }, () => null)}
          />
          <nav class="blocks-pager" aria-label="Block pages">
            <Show
              when={!atTip()}
              fallback={
                <span class="btn blocks-pager__off" aria-disabled="true">
                  ← Newer
                </span>
              }
            >
              <Link class="btn" href={newerHref()} rel="prev">
                ← Newer
              </Link>
            </Show>
            <Show when={!atTip()}>
              <Link class="blocks-pager__latest" href={paths.blocks()}>
                Newest blocks
              </Link>
            </Show>
            <Show
              when={(oldest() ?? 0) > 0}
              fallback={
                <span class="btn blocks-pager__off" aria-disabled="true">
                  Older →
                </span>
              }
            >
              <Link class="btn" href={paths.blocks((oldest() ?? 1) - 1)} rel="next">
                Older{NNBSP}→
              </Link>
            </Show>
          </nav>
        </Show>
      </Show>
    </div>
  );
}
