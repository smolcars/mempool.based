import { For, Show } from "solid-js";
import { FEE_KEY_BOUNDS, feeColor, formatFeeRate, formatFeeRateUp } from "../lib/format";
import "./FeeRate.css";

/**
 * A fee rate. `swatch` puts its band's colour beside it, only where it is the key to a drawing (a
 * square's tooltip): a number already says the rate, and colour belongs to the drawings. The colour
 * is taken from the printed value, so it always agrees with the number. `approx` prefixes `~` for projected values. `up` prints a
 * recommendation rounded up ({@link formatFeeRateUp}). `unit={false}` hides the
 * unit from sight only, as for {@link Amount}.
 */
export function FeeRate(props: {
  rate: number;
  unit?: boolean;
  approx?: boolean;
  up?: boolean;
  swatch?: boolean;
}) {
  const print = () => (props.up ? formatFeeRateUp : formatFeeRate);
  return (
    <span class="fee">
      <Show when={props.swatch}>
        <span
          class="fee__swatch"
          style={{ "--swatch": feeColor(props.rate, print()) }}
          aria-hidden="true"
        />
      </Show>
      <span class="num">
        {props.approx ? "~" : ""}
        {print()(props.rate)}
      </span>
      <span class={{ unit: true, "visually-hidden": props.unit === false }}> sat/vB</span>
    </span>
  );
}

/** The fee key: one swatch per band with its lower bound. Shown next to every fee visual. */
export function FeeKey() {
  return (
    <ol class="feekey" aria-label="Fee rate key, sat/vB, lower bound of each color">
      <For each={FEE_KEY_BOUNDS}>
        {(bound, i) => (
          <li class="feekey__band">
            <span
              class="feekey__swatch"
              style={{ "--swatch": `var(--fee-${i() + 1})` }}
              aria-hidden="true"
            />
            <span class="feekey__label num">
              {i() === FEE_KEY_BOUNDS.length - 1 ? `${bound}+` : String(bound)}
            </span>
          </li>
        )}
      </For>
    </ol>
  );
}
