import { createMemo, Show } from "solid-js";
import { Amount } from "../../components/Amount";
import { type Column, DataTable } from "../../components/DataTable";
import { FeeRate } from "../../components/FeeRate";
import { Hash } from "../../components/Hash";
import { SectionHeader } from "../../components/SectionHeader";
import { Empty, ErrorState } from "../../components/States";
import type { RecentTx } from "../../lib/esplora";
import { feeRate, groupDigits } from "../../lib/format";
import { paths } from "../../lib/network";
import type { Feed } from "./feeds";

/** Rows shown while the first answer is on its way, so the section keeps its height. */
const PLACEHOLDER_ROWS = 10;

const COLUMNS: Column<RecentTx | null>[] = [
  {
    header: "Transaction",
    cell: (t) =>
      t ? <Hash value={t.txid} href={paths.tx(t.txid)} copy={false} /> : <span class="home-bone" />,
  },
  {
    header: "Fee rate, sat/vB",
    cell: (t) =>
      t ? (
        <FeeRate rate={feeRate(t.fee, t.vsize) ?? Number.NaN} unit={false} />
      ) : (
        <span class="home-bone home-bone--short" />
      ),
  },
  {
    header: "Size, vB",
    numeric: true,
    optional: true,
    cell: (t) => (t ? groupDigits(t.vsize) : ""),
  },
  {
    header: "Value, BTC",
    numeric: true,
    cell: (t) => (t ? <Amount sats={t.value} unit={false} /> : ""),
  },
];

/**
 * The newest transactions to enter the mempool. Rows already on screen keep their element, so only
 * the ones that are new flash.
 */
export function RecentTxs(props: { feed: Feed<RecentTx[]> }) {
  let known = new Map<string, RecentTx>();
  const fresh = new WeakSet<RecentTx>();
  let first = true;
  const rows = createMemo(() => {
    const list = props.feed.data();
    if (!list) return undefined;
    const next = new Map<string, RecentTx>();
    const out = list.map((t) => {
      const kept = known.get(t.txid);
      if (kept) return kept;
      if (!first) fresh.add(t);
      return t;
    });
    for (const t of out) next.set(t.txid, t);
    known = next;
    first = false;
    return out;
  });

  return (
    <section class="home-recent" aria-labelledby="recent-title">
      <SectionHeader title="Latest transactions" id="recent-title" />
      <Show
        when={rows() ?? props.feed.error() === undefined}
        fallback={
          <ErrorState
            title="Could not load the latest transactions"
            error={props.feed.error()}
            retry={props.feed.refresh}
          />
        }
      >
        <DataTable<RecentTx | null>
          caption="The newest transactions waiting in the mempool"
          columns={COLUMNS}
          rows={rows() ?? Array.from({ length: PLACEHOLDER_ROWS }, () => null)}
          rowClass={(t) => (t && fresh.has(t) ? "home-recent__new" : undefined)}
          empty={
            <Empty title="No transactions are waiting">
              New transactions appear here as the node accepts them.
            </Empty>
          }
        />
      </Show>
    </section>
  );
}
