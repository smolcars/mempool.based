import { useParams } from "@solidjs/router";
import { createMemo, Show } from "solid-js";
import { DataTable } from "../../components/DataTable";
import { Hash } from "../../components/Hash";
import { Link } from "../../components/Link";
import { ErrorState } from "../../components/States";
import { TimeAgo } from "../../components/Time";
import { createApi } from "../../lib/api";
import type { Block } from "../../lib/esplora";
import { groupDigits, NNBSP, truncateMiddle } from "../../lib/format";
import { paths } from "../../lib/network";
import { usePageTitle } from "../../lib/page";
import { fullness, parseHeight, shownParam } from "../block/chain";
import { failed, ready, Skel } from "../block/parts";
import "./BlocksPage.css";

/** The API answers ten blocks per page. */
const PAGE = 10;

/** Thousands with two decimals, for table cells whose unit is in the header: 1 607 007 → `1 607.01`. */
export function formatThousands(n: number): string {
  if (!Number.isFinite(n) || n < 0) return "–";
  const hundredths = Math.round(n / 10);
  return `${groupDigits(Math.floor(hundredths / 100))}.${String(hundredths % 100).padStart(2, "0")}`;
}

const columns = [
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
      b ? <Hash value={b.id} href={paths.block(b.id)} copyLabel="Copy block hash" /> : <Skel />,
  },
  {
    header: "Mined",
    cell: (b: Block | null) => (b ? <TimeAgo time={b.timestamp} /> : <Skel size="s" />),
  },
  {
    header: "Transactions",
    numeric: true,
    cell: (b: Block | null) => (b ? groupDigits(b.tx_count) : <Skel size="s" />),
  },
  {
    header: "Size, kB",
    numeric: true,
    optional: true,
    cell: (b: Block | null) => (b ? formatThousands(b.size) : <Skel size="s" />),
  },
  {
    header: "Weight, kWU",
    numeric: true,
    optional: true,
    cell: (b: Block | null) =>
      b ? (
        <span class="blocks-weight" title={`${Math.round(fullness(b.weight) * 100)}% of the limit`}>
          <span class="blocks-weight__bar" aria-hidden="true">
            <span class="blocks-weight__fill" style={{ "--fill": String(fullness(b.weight)) }} />
          </span>
          {formatThousands(b.weight)}
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

  const list = () => {
    const l = ready(blocks);
    return Array.isArray(l) ? l : undefined;
  };
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
              retry={() => void blocksCtl.refetch()}
            />
          }
        >
          <DataTable
            caption="Recent blocks, newest first"
            columns={columns}
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
