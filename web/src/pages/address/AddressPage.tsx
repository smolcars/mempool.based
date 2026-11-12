import { useParams } from "@solidjs/router";
import { createMemo, createResource, For, Match, onCleanup, Show, Switch } from "solid-js";
import { HashFull } from "../../components/Hash";
import { createApi } from "../../lib/api";
import { fetchConfig } from "../../lib/config";
import type { AddressStats, Transaction } from "../../lib/esplora";
import { truncateMiddle } from "../../lib/format";
import { NETWORK_NAMES, type Network, onNetwork, paths } from "../../lib/network";
import { useNetwork, usePageTitle } from "../../lib/page";
import { type Recognised, recognise, refusal, TYPE_NAMES } from "./address";
import { History } from "./History";
import { Notice } from "./parts";
import { Totals } from "./Totals";
import "./AddressPage.css";

/** How addresses start on each network, for the malformed-address message. */
const PREFIXES: Record<Network, string> = {
  mainnet: "bc1, 1 or 3",
  signet: "tb1, m, n or 2",
  testnet4: "tb1, m, n or 2",
  regtest: "bcrt1, m, n or 2",
};

/** Minimum time between two refreshes on focus, so flicking between tabs is not a burst. */
const REFRESH_GAP_MS = 5000;

const decode = (s: string) => {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
};

export default function AddressPage() {
  const params = useParams<{ address: string }>();
  const network = useNetwork();
  const input = () => decode(params.address);
  const found = createMemo(() => recognise(input()));
  // Recreates the body for another address or network, so nothing from the last one lingers.
  const key = () => {
    const f = found();
    return f?.networks.includes(network()) ? `${network()} ${f.address}` : undefined;
  };
  usePageTitle(() => `Address ${truncateMiddle(found()?.address ?? input())}`);

  return (
    <div class="wrap address">
      <header class="address-head">
        <div class="address-head__row">
          <h1>Address</h1>
          <Show when={found()}>
            {(f) => (
              <span class="address-type" title={TYPE_NAMES[f().type].long}>
                {TYPE_NAMES[f().type].short}
              </span>
            )}
          </Show>
        </div>
        <Show when={found()} fallback={<p class="address-head__raw">{input()}</p>}>
          {(f) => <HashFull value={f().address} copyLabel="Copy address" />}
        </Show>
      </header>
      <Switch>
        <Match when={!found()}>
          <Malformed network={network()} />
        </Match>
        <Match when={key() === undefined}>
          <WrongNetwork found={found() as Recognised} network={network()} />
        </Match>
        <Match when={key()} keyed>
          {(k) => <AddressBody address={k.slice(k.indexOf(" ") + 1)} found={found()} />}
        </Match>
      </Switch>
    </div>
  );
}

function AddressBody(props: { address: string; found: Recognised | null }) {
  const network = useNetwork();
  const path = `/address/${encodeURIComponent(props.address)}`;
  const [stats, { refetch: refetchStats }] = createApi<AddressStats>(() => path);
  const [first, { refetch: refetchFirst }] = createApi<Transaction[]>(() => `${path}/txs`);
  let refreshHistory = () => {};

  // A refusal that holds for the whole address, from whichever request answered first.
  const pageRefusal = () => {
    for (const r of [stats, first]) {
      if (r.state !== "errored") continue;
      const kind = refusal(r.error);
      if (kind === "no-index" || kind === "invalid" || kind === "wrong-network") return kind;
    }
    return undefined;
  };
  const retryAll = () => {
    refetchStats();
    if (first.state === "errored") refetchFirst();
  };

  // shortcut: refreshes totals and the first page of history when the tab regains focus;
  // replace with an address subscription on the WebSocket (/app/ws) once it exists.
  let lastRefresh = Date.now();
  const refresh = () => {
    if (document.visibilityState !== "visible" || Date.now() - lastRefresh < REFRESH_GAP_MS) return;
    lastRefresh = Date.now();
    if (stats.state === "ready") refetchStats();
    if (first.state === "ready") refetchFirst();
    refreshHistory();
  };
  window.addEventListener("focus", refresh);
  document.addEventListener("visibilitychange", refresh);
  onCleanup(() => {
    window.removeEventListener("focus", refresh);
    document.removeEventListener("visibilitychange", refresh);
  });

  return (
    <Switch>
      <Match when={pageRefusal() === "no-index"}>
        <Notice title="This server does not index addresses" tone="caution">
          Its node runs without an address index, so it cannot show the balance or history of any
          address. This says nothing about this address or its funds: look it up with another
          explorer or your own node.
        </Notice>
      </Match>
      <Match when={pageRefusal() === "invalid"}>
        <Malformed network={network()} />
      </Match>
      <Match when={pageRefusal() === "wrong-network" && props.found}>
        {(f) => <WrongNetwork found={f()} network={network()} />}
      </Match>
      <Match when={true}>
        <Totals stats={stats} retry={retryAll} />
        {/* The balance-over-time chart goes here: give it a fixed height so nothing below moves. */}
        <History
          address={props.address}
          stats={stats}
          first={first}
          retryFirst={() => refetchFirst()}
          onRefresh={(fn) => {
            refreshHistory = fn;
          }}
        />
      </Match>
    </Switch>
  );
}

function Malformed(props: { network: Network }) {
  return (
    <Notice title="This is not a valid address" tone="caution">
      Check it for a missing or mistyped character. {NETWORK_NAMES[props.network]} addresses start
      with {PREFIXES[props.network]}.
    </Notice>
  );
}

/** An address of another network's form: name its network, and link there when it is served. */
function WrongNetwork(props: { found: Recognised; network: Network }) {
  const [config] = createResource(fetchConfig);
  const theirs = () => props.found.networks.filter((n) => n !== props.network);
  const served = () => theirs().filter((n) => config()?.networks.includes(n));
  const names = (list: readonly Network[]) => {
    const all = list.map((n) => NETWORK_NAMES[n]);
    return all.length > 1 ? `${all.slice(0, -1).join(", ")} or ${all.at(-1)}` : (all[0] ?? "");
  };
  return (
    <Notice title={`This is a ${names(theirs())} address`} tone="caution">
      <p>Addresses belong to one network, and this page shows {NETWORK_NAMES[props.network]}.</p>
      <Show
        when={served().length > 0}
        fallback={
          <Show when={config.state === "ready"}>
            <p>This server does not serve {names(theirs())}.</p>
          </Show>
        }
      >
        <ul class="address-elsewhere">
          <For each={served()}>
            {(n) => (
              <li>
                <a href={onNetwork(n, paths.address(props.found.address))}>
                  Open it on {NETWORK_NAMES[n]}
                </a>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </Notice>
  );
}
