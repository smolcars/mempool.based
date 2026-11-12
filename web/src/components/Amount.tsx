import { Show } from "solid-js";
import { btcParts, formatSats, MISSING } from "../lib/format";
import "./Amount.css";

/**
 * An amount in BTC with all eight decimals, trailing zeros dimmed; the title shows sats.
 * `unit={false}` hides the unit from sight only (a table names it in its header): screen readers
 * still read it.
 * `signed` adds a `+` to positive amounts and colors incoming ones (address history).
 */
export function Amount(props: { sats: number; unit?: boolean; signed?: boolean }) {
  const parts = () => btcParts(props.sats);
  const direction = () => (props.signed && props.sats > 0 ? "in" : undefined);
  return (
    <Show when={parts()} fallback={<span class="amount">{MISSING}</span>}>
      {(p) => (
        <span
          class="amount num"
          classList={{ "amount--in": direction() === "in" }}
          title={`${formatSats(props.sats)} sats`}
        >
          {direction() === "in" ? "+" : p().sign}
          {p().whole}.{p().significant}
          <span class="amount__zeros">{p().zeros}</span>
          <span classList={{ unit: true, "visually-hidden": props.unit === false }}> BTC</span>
        </span>
      )}
    </Show>
  );
}

/** An amount in sats, grouped. */
export function Sats(props: { sats: number; unit?: boolean }) {
  return (
    <span class="amount num">
      {formatSats(props.sats)}
      <span classList={{ unit: true, "visually-hidden": props.unit === false }}> sats</span>
    </span>
  );
}
