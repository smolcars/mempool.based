import { For } from "solid-js";
import { FEE_KEY_BOUNDS, feeColor, formatFeeRate } from "../lib/format";
import "./FeeRate.css";

/**
 * A fee rate with its band's swatch. The color is taken from the printed value, so it always
 * agrees with the number. `approx` prefixes `~` for projected values. `unit={false}` hides the
 * unit from sight only, as for {@link Amount}.
 */
export function FeeRate(props: { rate: number; unit?: boolean; approx?: boolean }) {
  return (
    <span class="fee">
      <span class="fee__swatch" style={{ "--swatch": feeColor(props.rate) }} aria-hidden="true" />
      <span class="num">
        {props.approx ? "~" : ""}
        {formatFeeRate(props.rate)}
      </span>
      <span classList={{ unit: true, "visually-hidden": props.unit === false }}> sat/vB</span>
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
