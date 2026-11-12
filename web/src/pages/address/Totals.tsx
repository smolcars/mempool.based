import { Match, type Resource, Show, Switch } from "solid-js";
import type { JSX } from "@solidjs/web";
import { Amount } from "../../components/Amount";
import { ErrorState } from "../../components/States";
import type { AddressStats } from "../../lib/esplora";
import { groupDigits } from "../../lib/format";
import { refusal } from "./address";
import { Notice, Skeleton, settled, UnconfirmedMark } from "./parts";

const plural = (n: number, one: string, many: string) =>
  `${groupDigits(n)}${n === 1 ? ` ${one}` : ` ${many}`}`;

/** One figure: label, value and a sub-line. Both lines hold a bar until `ready`, so nothing moves. */
function Stat(props: {
  label: string;
  ready: boolean;
  value: JSX.Element;
  sub: JSX.Element;
  class?: string;
}) {
  return (
    <div class={`address-stat ${props.class ?? ""}`}>
      <dt class="address-stat__label">{props.label}</dt>
      <dd class="address-stat__value">
        <Show when={props.ready} fallback={<Skeleton width="9ch" />}>
          {props.value}
        </Show>
      </dd>
      <dd class="address-stat__sub">
        <Show when={props.ready} fallback={<Skeleton width="6ch" />}>
          {props.sub}
        </Show>
      </dd>
    </div>
  );
}

const EMPTY = {
  funded_txo_count: 0,
  funded_txo_sum: 0,
  spent_txo_count: 0,
  spent_txo_sum: 0,
  tx_count: 0,
};

/** Balance, received, sent, transaction count and the unconfirmed change. */
export function Totals(props: { stats: Resource<AddressStats>; retry: () => void }) {
  const s = () => settled(props.stats);
  const chain = () => s()?.chain_stats ?? EMPTY;
  const pool = () => s()?.mempool_stats ?? EMPTY;
  const ready = () => s() !== undefined;
  const failure = () => (props.stats.state === "errored" ? refusal(props.stats.error) : undefined);

  return (
    <section class="address-totals" aria-label="Totals" aria-busy={!ready() ? "true" : "false"}>
      <Switch>
        <Match when={failure() === "too-large"}>
          <Notice title="Totals are not available for this address">
            It has more history than this server's node will add up, which is common for exchange
            and mining pool addresses. Its transactions are listed below, newest first.
          </Notice>
        </Match>
        <Match when={failure()}>
          <ErrorState
            title="Could not load this address"
            error={props.stats.error}
            retry={props.retry}
          />
        </Match>
        <Match when={true}>
          <dl class="address-stats">
            <Stat
              class="address-stat--balance"
              label="Balance"
              ready={ready()}
              value={<Amount sats={chain().funded_txo_sum - chain().spent_txo_sum} />}
              sub="Confirmed"
            />
            <Stat
              label="Received"
              ready={ready()}
              value={<Amount sats={chain().funded_txo_sum} />}
              sub={plural(chain().funded_txo_count, "output", "outputs")}
            />
            <Stat
              label="Sent"
              ready={ready()}
              value={<Amount sats={chain().spent_txo_sum} />}
              sub={plural(chain().spent_txo_count, "output", "outputs")}
            />
            <Stat
              label="Transactions"
              ready={ready()}
              value={<span class="num">{groupDigits(chain().tx_count + pool().tx_count)}</span>}
              sub={`${groupDigits(chain().tx_count)} confirmed`}
            />
            <Stat
              label="Unconfirmed"
              ready={ready()}
              value={
                <Show when={pool().tx_count} fallback={<span class="muted">None</span>}>
                  <Amount sats={pool().funded_txo_sum - pool().spent_txo_sum} signed />
                </Show>
              }
              sub={
                <Show when={pool().tx_count} fallback="Nothing waiting">
                  {(n) => <UnconfirmedMark label={plural(n(), "transaction", "transactions")} />}
                </Show>
              }
            />
          </dl>
        </Match>
      </Switch>
    </section>
  );
}
