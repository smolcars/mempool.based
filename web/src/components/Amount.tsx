import { Show } from "solid-js";
import { btcParts, formatBtc, formatBtcShort, formatSats, MISSING } from "../lib/format";
import "./Amount.css";

/**
 * An amount in BTC with all eight decimals, trailing zeros dimmed; the title shows sats. `short`
 * prints a summary's magnitude instead ({@link formatBtcShort}), the exact amount in its title;
 * `approx` puts `~` before it.
 * `unit={false}` hides the unit from sight only (a table names it in its header): screen readers
 * still read it.
 * `signed` adds a `+` to positive amounts (address history): the sign carries the direction, not
 * a colour.
 */
export function Amount(props: {
  sats: number;
  unit?: boolean;
  signed?: boolean;
  short?: boolean;
  approx?: boolean;
  /** Exact, but the whole bitcoins and two decimals lead and the rest is quieter: a balance. */
  lead?: boolean;
}) {
  const parts = () => btcParts(props.sats);
  const direction = () => (props.signed && props.sats > 0 ? "in" : undefined);
  return (
    <Show when={parts()} fallback={<span class="amount">{MISSING}</span>}>
      {(p) => (
        <Show
          when={!props.short}
          fallback={
            <span
              class="amount num"
              title={`${formatBtc(props.sats)} BTC, ${formatSats(props.sats)} sats`}
            >
              {props.approx ? "~" : ""}
              {formatBtcShort(props.sats)}
              <span class={{ unit: true, "visually-hidden": props.unit === false }}> BTC</span>
            </span>
          }
        >
          <span class="amount num" title={`${formatSats(props.sats)} sats`}>
            {direction() === "in" ? "+" : p().sign}
            {p().whole}.{p().significant}
            <span class="amount__zeros">{p().zeros}</span>
            <span class={{ unit: true, "visually-hidden": props.unit === false }}> BTC</span>
          </span>
        </Show>
      )}
    </Show>
  );
}

/** An amount in sats, grouped. */
export function Sats(props: { sats: number; unit?: boolean }) {
  return (
    <span class="amount num">
      {formatSats(props.sats)}
      <span class={{ unit: true, "visually-hidden": props.unit === false }}> sats</span>
    </span>
  );
}
