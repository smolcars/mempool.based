// The 100 largest, best-connected or oldest Lightning nodes (docs/API.md "Rankings").
import { useSearchParams } from "@solidjs/router";
import { createMemo, For, Match, Show, Switch } from "solid-js";
import { formatDay } from "../../components/chart/scale";
import { type Column, DataTable } from "../../components/DataTable";
import { Link } from "../../components/Link";
import { ErrorState, Loading } from "../../components/States";
import { TimeAgo } from "../../components/Time";
import { createRequest } from "../../lib/api";
import { groupDigits } from "../../lib/format";
import { type Network, paths } from "../../lib/network";
import { useNetwork, usePageTitle } from "../../lib/page";
import { isRankings, parseRanking, RANKINGS, type RankedNode } from "./lightning";
import { ask, Gate, nodeColumns, VERIFIED_NOTE } from "./parts";

export default function RankingsPage() {
  const [search] = useSearchParams();
  const network = useNetwork();
  const by = createMemo(() => parseRanking(search["by"]));
  usePageTitle(() => `Lightning nodes ${by().words}`);
  return (
    <div class="wrap ln">
      <p class="ln-back">
        <Link href={paths.lightning()}>Lightning</Link>
      </p>
      <header class="ln-head">
        <h1>Lightning nodes</h1>
        <nav class="ln-picker" aria-label="Ranked by">
          <ul>
            <For each={RANKINGS}>
              {(r) => (
                <li>
                  <Link
                    class="ln-picker__item"
                    href={paths.lightningRankings(r.id)}
                    aria-current={r.id === by().id ? "page" : undefined}
                  >
                    {r.label}
                  </Link>
                </li>
              )}
            </For>
          </ul>
        </nav>
      </header>
      <Show when={network()} keyed>
        {(n) => (
          <Gate>
            <Ranked network={n} by={by().id} />
          </Gate>
        )}
      </Show>
    </div>
  );
}

function Ranked(props: { network: Network; by: string }) {
  const [ranked, { refetch }] = createRequest(
    () => props.by,
    (by, signal) => ask(props.network, `/rankings/${by}?limit=100`, isRankings, signal),
  );
  const columns: Column<RankedNode>[] = [
    { header: "Rank", numeric: true, optional: true, cell: (_, i) => groupDigits(i + 1) },
    ...nodeColumns(props.by === "channels" ? "channels" : "capacity"),
    {
      header: "First seen",
      optional: props.by !== "age",
      cell: (n) => (typeof n.first_seen === "number" ? formatDay(n.first_seen) : "–"),
    },
    {
      header: "Last announced",
      optional: true,
      cell: (n) => (typeof n.last_update === "number" ? <TimeAgo time={n.last_update} /> : "–"),
    },
  ];
  return (
    <Switch>
      <Match when={ranked.latest}>
        {(r) => (
          <section>
            <div class="ln-busy" aria-busy={r().by === props.by ? "false" : "true"}>
              <DataTable
                caption={`The 100 nodes ${parseRanking(r().by).words}`}
                columns={columns}
                rows={r().nodes}
              />
            </div>
            <p class="ln-aside">
              {VERIFIED_NOTE} First seen is the oldest funding block among a node's channels this
              server has seen.
            </p>
          </section>
        )}
      </Match>
      <Match when={ranked.state === "errored"}>
        <ErrorState title="Could not load the ranking" error={ranked.error} retry={refetch} />
      </Match>
      <Match when={true}>
        <Loading label="Loading the ranking" />
      </Match>
    </Switch>
  );
}
