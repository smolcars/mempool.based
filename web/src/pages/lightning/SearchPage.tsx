// Lightning nodes by name or key, and a channel by id: the server's index answers
// (docs/API.md "Search"). A name proves nothing, so every node shows its key.
import { useSearchParams } from "@solidjs/router";
import { createMemo, Match, Show, Switch } from "solid-js";
import { Amount } from "../../components/Amount";
import { DataTable } from "../../components/DataTable";
import { SectionHeader } from "../../components/SectionHeader";
import { Empty, ErrorState, Loading } from "../../components/States";
import { createRequest } from "../../lib/api";
import { type Network, paths } from "../../lib/network";
import { useNetwork, usePageTitle } from "../../lib/page";
import { isSearchResult, type SearchResult, searchProblem, STATE_WORDS } from "./lightning";
import { ask, ChannelLink, Gate, LightningSearch, nodeColumns, LightningNav } from "./parts";

export default function SearchPage() {
  const [search] = useSearchParams();
  const network = useNetwork();
  const q = createMemo(() => {
    const v: unknown = search["q"];
    return typeof v === "string" ? v.trim() : "";
  });
  usePageTitle(() => "Lightning search");
  return (
    <div class="wrap ln">
      <LightningNav here="search" />
      <header class="ln-head">
        <h1>Search Lightning</h1>
        <LightningSearch value={q()} />
      </header>
      <Show
        when={searchProblem(q()) === undefined}
        fallback={<Empty title="Nothing searched for">{searchProblem(q())}</Empty>}
      >
        <Show when={{ network: network(), q: q() }} keyed>
          {(k) => (
            <Gate>
              <Results network={k.network} q={k.q} />
            </Gate>
          )}
        </Show>
      </Show>
    </div>
  );
}

function Results(props: { network: Network; q: string }) {
  const [found, { refetch }] = createRequest(
    () => props.q,
    (q, signal) => ask(props.network, `/search?q=${encodeURIComponent(q)}`, isSearchResult, signal),
  );
  return (
    <Switch>
      <Match when={found.latest}>{(r) => <Found r={r()} q={props.q} />}</Match>
      <Match when={found.state === "errored"}>
        <ErrorState title="Could not search" error={found.error} retry={refetch} />
      </Match>
      <Match when={true}>
        <Loading label="Searching" />
      </Match>
    </Switch>
  );
}

function Found(props: { r: SearchResult; q: string }) {
  return (
    <Show
      when={props.r.nodes.length > 0 || props.r.channels.length > 0}
      fallback={
        <Empty title="Nothing matches">
          No node's name or public key holds this, and it is no channel this server knows. Names are
          matched anywhere in them, keys from their start.
        </Empty>
      }
    >
      <Show when={props.r.channels.length > 0}>
        <section aria-labelledby="ln-found-channels-title">
          <SectionHeader title="Channel" id="ln-found-channels-title" />
          <DataTable
            caption="The channel with this id"
            columns={[
              { header: "Channel", cell: (c) => <ChannelLink id={c.id} /> },
              {
                header: "Capacity, BTC",
                numeric: true,
                cell: (c) => <Amount sats={c.capacity} unit={false} short />,
              },
              { header: "State", cell: (c) => STATE_WORDS[c.state] ?? c.state },
            ]}
            rows={props.r.channels}
          />
        </section>
      </Show>
      <Show when={props.r.nodes.length > 0}>
        <section aria-labelledby="ln-found-nodes-title">
          <SectionHeader title="Nodes" id="ln-found-nodes-title">
            {props.r.nodes.length === 10 ? "The 10 largest matches" : "Largest first"}
          </SectionHeader>
          <DataTable
            caption="Nodes that match"
            columns={nodeColumns("capacity")}
            rows={props.r.nodes}
            rowHref={(n) => paths.lightningNode(n.id)}
          />
          <p class="ln-aside">
            Any node can announce any name, so a famous name can appear more than once: the public
            key under each is what tells them apart.
          </p>
        </section>
      </Show>
    </Show>
  );
}
