import { For } from "solid-js";
import { FEE_KEY_BOUNDS, formatFeeRate, formatSize } from "../lib/format";
import "./Strata.css";

/** One layer: its fee band (1–10), and where it sits as fractions of the frame's height. */
export interface Layer {
  band: number;
  /** Bottom edge, from the frame's floor. */
  y: number;
  /** Thickness. */
  h: number;
  /** Near the floor: drawn without the seam above it, so its color still shows. */
  thin: boolean;
}

/**
 * The strata of a block (docs/DESIGN.md "Idea"): each fee band with any virtual size becomes a
 * layer, cheapest at the bottom, as thick as its share of the block, the stack as high as `fill`
 * (how full the block is, 0–1), and at least one hairline (`min`) so a block with any paying
 * transaction shows something. Where there is room, a layer is never thinner than `min`, so one
 * transaction in a band still shows, its thickness taken from the thicker layers. Where there is
 * not (a nearly empty block), the stack keeps the block's true height and the layers share it by
 * size: the fill height stays honest, and bands too small to see are deliberately not drawn
 * thicker. Empty bands get no layer.
 */
export function strataLayers(bands: readonly number[], fill: number, min: number): Layer[] {
  const held = bands
    .map((vsize, i) => ({ band: i + 1, vsize: Number.isFinite(vsize) && vsize > 0 ? vsize : 0 }))
    .filter((b) => b.vsize > 0);
  if (held.length === 0) return [];
  const hairline = Math.min(Math.max(0, min), 1);
  const top = Math.min(1, Math.max(Number.isFinite(fill) ? fill : 0, hairline));
  const floor = held.length * hairline <= top ? hairline : 0;
  // Layers below the floor are raised to it and the rest share what is left, until none is below.
  const thin = new Set<number>();
  let h: number[] = [];
  for (let pass = 0; pass <= held.length; pass++) {
    const rest = top - thin.size * floor;
    const restVsize = held.reduce((sum, b, i) => (thin.has(i) ? sum : sum + b.vsize), 0);
    h = held.map((b, i) => (thin.has(i) ? floor : (b.vsize / restVsize) * rest));
    const below = h.findIndex((x, i) => !thin.has(i) && x < floor);
    if (below === -1) break;
    for (const [i, x] of h.entries()) if (x < floor) thin.add(i);
  }
  let y = 0;
  return held.map((b, i) => {
    const thick = h[i] ?? 0;
    const layer = { band: b.band, y, h: thick, thin: thick < hairline * 2 };
    y += layer.h;
    return layer;
  });
}

/** The top of a stack of layers, 0 when there are none. */
export const strataTop = (layers: readonly Layer[]): number => {
  const last = layers.at(-1);
  return last ? last.y + last.h : 0;
};

/** A band's range in words: `under 0.20 sat/vB`, `0.50 to 1.00 sat/vB`, `10.0 sat/vB and up`. */
export function bandRange(band: number): string {
  const from = FEE_KEY_BOUNDS[band - 1];
  const to = FEE_KEY_BOUNDS[band];
  if (from === undefined) return "";
  if (to === undefined) return `${formatFeeRate(from)} sat/vB and up`;
  if (band === 1) return `under ${formatFeeRate(to)} sat/vB`;
  return `${formatFeeRate(from)} to ${formatFeeRate(to)} sat/vB`;
}

/** The strata as text, one line per band that holds anything: what a screen reader gets. */
export function describeBands(bands: readonly number[]): string[] {
  const total = bands.reduce((sum, v) => sum + (Number.isFinite(v) && v > 0 ? v : 0), 0);
  if (total === 0) return [];
  return bands.flatMap((vsize, i) => {
    if (!(vsize > 0)) return [];
    const share = vsize / total;
    const pct = share < 0.01 ? "under 1%" : `${Math.round(share * 100)}%`;
    return [`${bandRange(i + 1)}: ${formatSize(vsize)}, ${pct}`];
  });
}

/**
 * Fee layers drawn into the box that holds them (give it a height). `settle` drops them in one at
 * a time, cheapest first, like sediment; with reduced motion they fade in. Decorative: put the
 * figures, or {@link describeBands}, beside it as text.
 */
export function Strata(props: { layers: readonly Layer[]; settle?: boolean }) {
  return (
    <span class="strata" classList={{ "strata--settle": props.settle === true }} aria-hidden="true">
      <For each={props.layers}>
        {(layer, i) => (
          <span
            class="strata__layer"
            classList={{ "strata__layer--thin": layer.thin }}
            style={{
              "--y": String(layer.y),
              "--h": String(layer.h),
              "--c": `var(--fee-${layer.band})`,
              "--i": String(i()),
            }}
          />
        )}
      </For>
    </span>
  );
}
