// One Lightning node: its name (a claim), its key (its identity), what it announces and its
// channels, paged.
import { useParams } from "@solidjs/router";
import { createSignal, For, Match, Show, Switch } from "solid-js";
import { Amount } from "../../components/Amount";
import { type Column, DataTable } from "../../components/DataTable";
import { HashFull } from "../../components/Hash";
import { Pagination } from "../../components/Pagination";
import { More } from "../../components/More";
import { SectionHeader } from "../../components/SectionHeader";
import { Empty, ErrorState, Loading } from "../../components/States";
import { TimeAgo } from "../../components/Time";
import { createRequest, isNotFound } from "../../lib/api";
import { formatDay } from "../../components/chart/scale";
import { groupDigits } from "../../lib/format";
import { type Network, paths } from "../../lib/network";
import { useNetwork, usePageTitle } from "../../lib/page";
import {
  cleanAlias,
  cut,
  isNodeChannels,
  isNodeDetail,
  isPublicKey,
  KIND_SHORT,
  NAME_IN_TITLE,
  type NodeChannel,
  type NodeChannels,
  type NodeDetail,
  STATE_WORDS,
} from "./lightning";
import {
  ask,
  BlockLink,
  ChannelLink,
  closesInWords,
  Gate,
  NodeName,
  Stat,
  LightningNav,
} from "./parts";

export default function NodePage() {
  const params = useParams<{ key: string }>();
  const network = useNetwork();
  const key = () => (params.key ?? "").toLowerCase();
  return (
    <div class="wrap ln">
      <LightningNav here="detail" />
      <Show
        when={isPublicKey(key())}
        fallback={<NotFound title="Not a node's public key" text={BAD_KEY} />}
      >
        <Show when={{ network: network(), id: key() }} keyed>
          {(k) => (
            <Gate>
              <Node network={k.network} id={k.id} />
            </Gate>
          )}
        </Show>
      </Show>
    </div>
  );
}

const BAD_KEY =
  "A Lightning node is named by its public key: 66 hexadecimal characters, starting with 02 or 03.";

function NotFound(props: { title: string; text: string }) {
  usePageTitle(() => "Lightning node");
  return (
    <>
      <h1>Lightning node</h1>
      <Empty title={props.title}>{props.text}</Empty>
    </>
  );
}

function Node(props: { network: Network; id: string }) {
  const [node, { refetch }] = createRequest(
    () => props.id,
    (id, signal) => ask(props.network, `/nodes/${id}`, isNodeDetail, signal),
  );
  return (
    <Switch>
      <Match when={node.latest}>{(n) => <Body node={n()} network={props.network} />}</Match>
      <Match when={node.state === "errored" && isNotFound(node.error)}>
        <NotFound
          title="This server knows no such node"
          text="It holds a node while the node has an open channel it checked against the chain. A node with only private or closed channels, or one that never announced a channel, is not here."
        />
      </Match>
      <Match when={node.state === "errored"}>
        <ErrorState title="Could not load this node" error={node.error} retry={refetch} />
      </Match>
      <Match when={true}>
        <Loading label="Loading the node" />
      </Match>
    </Switch>
  );
}

function Body(props: { node: NodeDetail; network: Network }) {
  const name = () => cleanAlias(props.node.alias);
  const title = () => (name() === null ? null : cut(name() ?? "", NAME_IN_TITLE));
  const [whole, setWhole] = createSignal(false);
  usePageTitle(() => `Lightning node ${title()?.text ?? props.node.id.slice(0, 20)}`);
  const c = () => props.node.channels;
  return (
    <>
      <header class="ln-identity">
        <h1>
          <Show when={title()} fallback={<span class="ln-alias--none">Unnamed node</span>}>
            {(t) => <bdi class="ln-alias">{t().text}</bdi>}
          </Show>
        </h1>
        <Show when={title()?.more}>
          <div>
            <button
              class="btn ln-name-more"
              type="button"
              aria-expanded={whole() ? "true" : "false"}
              onClick={() => setWhole((w) => !w)}
            >
              {whole() ? "Hide the whole name" : "Show the whole name"}
            </button>
            <Show when={whole()}>
              <p class="ln-name-full">
                <bdi>{name()}</bdi>
              </p>
            </Show>
          </div>
        </Show>
        <HashFull value={props.node.id} copyLabel="Copy public key" />
        <p class="ln-aside">
          The public key is the node's identity: any node can announce any name.
        </p>
      </header>

      <section aria-labelledby="ln-node-now-title">
        <SectionHeader title="Channels and capacity" id="ln-node-now-title" />
        <dl class="ln-stats">
          <Stat label="Capacity">
            <Amount sats={props.node.capacity} short />
          </Stat>
          <Stat label="Open channels" sub={`${groupDigits(c().active)} active`}>
            <span class="num">{groupDigits(c().open)}</span>
          </Stat>
          <Stat
            label="Closed since this server started"
            sub={c().closed > 0 ? closesInWords(c()) : undefined}
          >
            <span class="num">{groupDigits(c().closed)}</span>
          </Stat>
          <Stat
            label="First seen"
            sub="The oldest funding block among its channels this server has seen"
          >
            <span class="num">
              {props.node.first_seen === null ? "–" : formatDay(props.node.first_seen)}
            </span>
          </Stat>
        </dl>
      </section>

      <section aria-labelledby="ln-node-announced-title">
        <SectionHeader title="What it announces" id="ln-node-announced-title">
          <Show when={props.node.last_update} fallback="It has sent no announcement">
            {(t) => (
              <>
                Last announced <TimeAgo time={t()} />
              </>
            )}
          </Show>
        </SectionHeader>
        <dl class="ln-facts">
          <div class="ln-fact">
            <dt>Addresses</dt>
            <dd>
              <Show when={(props.node.addresses ?? []).length > 0} fallback="None announced">
                <ul class="ln-chips ln-chips--mono">
                  <For each={props.node.addresses ?? []}>
                    {(a) => <li>{cut(cleanAlias(a) ?? "", 128).text}</li>}
                  </For>
                </ul>
              </Show>
            </dd>
          </div>
          <div class="ln-fact">
            <dt>Features</dt>
            <dd>
              <Show when={props.node.features.length > 0} fallback="None announced">
                <ul class="ln-chips">
                  <For each={props.node.features}>
                    {(f) => (
                      <li>
                        {f.name ?? `Bit ${f.bit}`}
                        <span class="ln-chips__req">{f.required ? " (required)" : ""}</span>
                      </li>
                    )}
                  </For>
                </ul>
              </Show>
            </dd>
          </div>
        </dl>
        <More label="About these">
          <p>
            Addresses are where the node says it can be reached; nothing here checks them, and they
            are not links. The name above is whatever the node's operator announced, another node's
            included.
          </p>
        </More>
      </section>

      <Channels node={props.node} network={props.network} />
    </>
  );
}

type Tab = "open" | "closed";

function Channels(props: { node: NodeDetail; network: Network }) {
  const [tab, setTab] = createSignal<Tab>("open");
  const [page, setPage] = createSignal(0);
  const first = () => props.node.open_channels;
  // The node's own answer holds the first page of open channels; any other page is asked for.
  const [list, { refetch }] = createRequest(
    () => (tab() === "open" && page() === 0 ? false : `${tab()} ${page()}`),
    (_, signal) =>
      ask(
        props.network,
        `/nodes/${props.node.id}/channels?status=${tab()}&page=${page()}`,
        isNodeChannels,
        signal,
      ),
  );
  const shown = (): NodeChannels | undefined =>
    tab() === "open" && page() === 0 ? first() : list.latest;
  const total = () => (tab() === "open" ? props.node.channels.open : props.node.channels.closed);
  const pages = () => Math.max(1, Math.ceil(total() / (first().per_page || 25)));
  const choose = (t: Tab) => {
    setTab(t);
    setPage(0);
  };
  return (
    <section aria-labelledby="ln-node-channels-title">
      <SectionHeader title="Channels" id="ln-node-channels-title" />
      <div class="ln-tabs" role="group" aria-label="Which channels">
        <button
          class="btn"
          type="button"
          aria-pressed={tab() === "open" ? "true" : "false"}
          onClick={() => choose("open")}
        >
          Open ({groupDigits(props.node.channels.open)})
        </button>
        <button
          class="btn"
          type="button"
          aria-pressed={tab() === "closed" ? "true" : "false"}
          onClick={() => choose("closed")}
        >
          Closed ({groupDigits(props.node.channels.closed)})
        </button>
      </div>
      <Switch>
        {/* A failed page says so, never the last page under the new one's number. */}
        <Match when={list.state === "errored"}>
          <ErrorState title="Could not load these channels" error={list.error} retry={refetch} />
        </Match>
        <Match when={shown()}>
          {(s) => (
            <div
              class="ln-busy"
              aria-busy={s().status === tab() && s().page === page() ? "false" : "true"}
            >
              <ChannelList rows={s().channels} closed={tab() === "closed"} />
            </div>
          )}
        </Match>
        <Match when={true}>
          <Loading label="Loading channels" />
        </Match>
      </Switch>
      <Show when={pages() > 1}>
        <Pagination
          page={page() + 1}
          pageCount={pages()}
          onPage={(p) => setPage(p - 1)}
          label="Channel pages"
        />
      </Show>
    </section>
  );
}

function ChannelList(props: { rows: NodeChannel[]; closed: boolean }) {
  const columns = (): Column<NodeChannel>[] => [
    // The channel and who it is with share a cell, so the table fits a phone.
    {
      header: "Channel and peer",
      cell: (c) => (
        <span class="ln-ends">
          <ChannelLink id={c.id} />
          <NodeName node={c.peer} />
        </span>
      ),
    },
    {
      header: "Capacity, BTC",
      numeric: true,
      cell: (c) => <Amount sats={c.capacity} unit={false} />,
    },
    ...(props.closed
      ? [
          {
            header: "Closed",
            cell: (c: NodeChannel) => (
              <Show when={c.closed} fallback="–">
                {(x) => (
                  <span class="ln-ends">
                    <span>{KIND_SHORT[x().kind]}</span>
                    <span>
                      Block <BlockLink block={x().block} />
                    </span>
                  </span>
                )}
              </Show>
            ),
          },
        ]
      : [
          {
            header: "State",
            optional: true,
            cell: (c: NodeChannel) => STATE_WORDS[c.state] ?? c.state,
          },
          {
            header: "Its fee rate, ppm",
            numeric: true,
            optional: true,
            cell: (c: NodeChannel) => (c.fee_rate === null ? "–" : groupDigits(c.fee_rate)),
          },
        ]),
  ];
  return (
    <DataTable
      caption={
        props.closed
          ? "Its closed channels, newest close first"
          : "Its open channels, largest first"
      }
      columns={columns()}
      rowHref={(c) => paths.lightningChannel(c.id)}
      rows={props.rows}
      empty={
        <Empty title={props.closed ? "No close seen" : "No open channel"}>
          {props.closed
            ? "This server has seen none of its channels close since it started watching."
            : "Nothing on this page."}
        </Empty>
      }
    />
  );
}
