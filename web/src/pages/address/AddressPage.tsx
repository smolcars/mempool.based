import { useParams } from "@solidjs/router";
import { createMemo, createSignal, For, Match, onCleanup, Show, Switch, untrack } from "solid-js";
import { HashFull } from "../../components/Hash";
import { createApi, createRequest } from "../../lib/api";
import { fetchConfig } from "../../lib/config";
import { type LiveStatus, useLiveStatus, useTopic } from "../../lib/live";
import type { AddressStats, Transaction } from "../../lib/esplora";
import { truncateMiddle } from "../../lib/format";
import { NETWORK_NAMES, type Network, onNetwork, paths } from "../../lib/network";
import { useNetwork, usePageTitle } from "../../lib/page";
import { type Recognised, recognise, TYPE_NAMES } from "../../lib/address";
import { type AddressMessage, addressTopic } from "../../lib/track-topics";
import { refusal } from "./address";
import { History } from "./History";
import { Notice, settled } from "./parts";
import { Totals } from "./Totals";
import "./AddressPage.css";

/** How addresses start on each network, for the malformed-address message. */
const PREFIXES: Record<Network, string> = {
  mainnet: "bc1, 1 or 3",
  signet: "tb1, m, n or 2",
  testnet4: "tb1, m, n or 2",
  regtest: "bcrt1, m, n or 2",
};

/** A history change the address index has not shown yet is asked again after 1, 2 and 4 s. */
const CATCH_UP_MS = 1000;
const CATCH_UP_TRIES = 3;

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
  // The page is made afresh for each address (AddressPage keys it).
  const address = untrack(() => props.address);
  const path = `/address/${encodeURIComponent(address)}`;
  const [stats, { refetch: refetchStats }] = createApi<AddressStats>(() => path);
  const [first, { refetch: refetchFirst }] = createApi<Transaction[]>(() => `${path}/txs`);
  let refreshHistory = () => {
    // Replaced by History once it mounts.
  };

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

  // The address topic says when its history changes; the page then refetches what it shows.
  // A request still in flight is asked again, so its answer is as new as the change.
  const asked = (state: string) =>
    state === "ready" || state === "refreshing" || state === "pending";
  const refresh = () => {
    if (asked(stats.state)) refetchStats();
    if (asked(first.state)) refetchFirst();
    refreshHistory();
  };
  // The node's status can change a moment before its address index answers with the change
  // (seen on regtest), so a refetch that brings nothing new is tried again, a few times.
  const fingerprint = () =>
    JSON.stringify([settled(stats), settled(first)?.map((t) => [t.txid, t.status.block_hash])]);
  let catchUpTimer: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(catchUpTimer));
  const catchUp = (before: string, attempt = 0) => {
    clearTimeout(catchUpTimer);
    refresh();
    if (attempt >= CATCH_UP_TRIES) return;
    const check = () => {
      if (stats.state === "refreshing" || first.state === "refreshing") {
        catchUpTimer = setTimeout(check, CATCH_UP_MS);
      } else if (fingerprint() === before) {
        catchUp(before, attempt + 1);
      }
    };
    catchUpTimer = setTimeout(check, CATCH_UP_MS * 2 ** attempt);
  };
  const [watch, setWatch] = createSignal<AddressMessage>();
  /** The last status heard; undefined before the first. */
  let token: string | null | undefined;
  useTopic(addressTopic(address), {
    message: (m) => {
      setWatch(m);
      if (m.type !== "watching") return;
      const before = token;
      token = m.status;
      // The page fetched before the node watched the address, so the first status is followed by
      // one more fetch: nothing that arrived in between is missed (docs/WS.md).
      if (before === undefined) refresh();
      else if (before !== m.status) catchUp(fingerprint());
    },
    // After a reconnect the first message is compared with the last status heard.
    resync: () => undefined,
  });
  const socket = useLiveStatus();

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
        <LiveLine watch={watch()} socket={socket()} />
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

/**
 * Whether the page updates itself, said quietly: a word when it does, and why not when it cannot.
 * Nothing before the first answer, so a normal load shows no flicker of "connecting".
 */
function LiveLine(props: { watch: AddressMessage | undefined; socket: LiveStatus }) {
  const why = (): string | undefined => {
    const w = props.watch;
    if (w === undefined) return undefined;
    if (props.socket !== "live") {
      return "The live connection dropped. This page catches up by itself when it is back.";
    }
    if (w.type === "watching") return "";
    if (w.type === "unavailable") {
      return "The server has lost touch with its node. This page catches up by itself when it is back.";
    }
    if (w.reason === "no_index") return undefined; // The page already says the node has no index.
    return w.reason === "too_large"
      ? "This address has too much history for the node to watch, so the page does not update by itself. Reload it to see new transactions."
      : "The node is watching as many addresses as it can, so the page does not update by itself. Reload it to see new transactions.";
  };
  return (
    <Show when={why() !== undefined}>
      <p class={["address-live", { "address-live--on": why() === "" }]}>
        <svg class="address-live__icon" viewBox="0 0 12 12" aria-hidden="true">
          <circle cx="6" cy="6" r="4" />
        </svg>
        <span class="address-live__label">{why() === "" ? "Live" : "Not live"}</span>
        <Show when={why()}>
          <span class="address-live__detail">{why()}</span>
        </Show>
      </p>
    </Show>
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
  const [config] = createRequest(() => true, fetchConfig);
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
