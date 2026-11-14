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
  /**
   * Exact, but where a phone's narrow field cannot hold it (a thousand bitcoin and more), in
   * magnitude there (`.amount--long`, CSS by the field), the exact amount in its title. A field
   * that measures its exact amount wider than itself in the reader's font marks it
   * `.amount--tight`, which reads in magnitude too (`data-short`, drawn by CSS, so the text is
   * still the exact amount).
   */
  fit?: boolean;
}) {
  const parts = () => btcParts(props.sats);
  const long = () => props.fit === true && Math.abs(props.sats) >= LONG;
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
          <span
            class={["amount num", { "amount--long": long() }]}
            data-short={props.fit === true && !long() ? formatBtcShort(props.sats) : undefined}
            title={
              props.fit === true
                ? `${formatBtc(props.sats)} BTC, ${formatSats(props.sats)} sats`
                : `${formatSats(props.sats)} sats`
            }
          >
            <span class="amount__exact">
              {direction() === "in" ? "+" : p().sign}
              {p().whole}.{p().significant}
              <span class="amount__zeros">{p().zeros}</span>
            </span>
            <Show when={long()}>
              <span class="amount__short">{formatBtcShort(props.sats)}</span>
            </Show>
            <span class={{ unit: true, "visually-hidden": props.unit === false }}> BTC</span>
          </span>
        </Show>
      )}
    </Show>
  );
}

/** A thousand bitcoin: from here an exact amount outgrows a phone's flow card. */
const LONG = 100_000_000_000;

/** An amount in sats, grouped. */
export function Sats(props: { sats: number; unit?: boolean }) {
  return (
    <span class="amount num">
      {formatSats(props.sats)}
      <span class={{ unit: true, "visually-hidden": props.unit === false }}> sats</span>
    </span>
  );
}
