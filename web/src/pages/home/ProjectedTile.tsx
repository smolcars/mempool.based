import { type Accessor, createMemo, createSignal, onCleanup, Show } from "solid-js";
import { FeeRate } from "../../components/FeeRate";
import { type Layer, Strata, strataLayers } from "../../components/Strata";
import { formatFeeRate, groupDigits } from "../../lib/format";
import type { Network } from "../../lib/network";
import type { ProjectedTile as Tile } from "./projection";

const MAX_WEIGHT = 4_000_000;
/** The thinnest layer, as a share of a tile's strata box (about 45 px): 2 px, so one transaction shows. */
export const TILE_LAYER_MIN = 0.045;
const ORDINALS = ["Next block", "Second block", "Third block", "Fourth block"];

/** A tile's heading: when its block is expected. Regtest has no block interval to count in. */
export function projectedTitle(t: Pick<Tile, "slot" | "blocks">, network: Network): string {
  if (t.blocks > 1) return `+${groupDigits(t.blocks)} blocks`;
  if (t.slot === 0) return "Next block";
  return network === "regtest" ? (ORDINALS[t.slot] ?? "Later") : `In ~${(t.slot + 1) * 10} min`;
}

/** What a screen reader hears for a tile. */
export function projectedLabel(t: Tile, network: Network): string {
  const title = projectedTitle(t, network);
  if (t.txs === 0) return `${title}, projected: nothing waiting`;
  const txs = `${groupDigits(t.txs)} ${t.txs === 1 ? "transaction" : "transactions"}`;
  return `${title}, projected: ${txs}, median fee about ${formatFeeRate(t.medianRate)} sat/vB, fee range ${formatFeeRate(t.minRate)} to ${formatFeeRate(t.maxRate)} sat/vB`;
}

/** Whether two sets of layers draw the same, so their elements (and any settling) can stay. */
export const sameLayers = (a: readonly Layer[], b: readonly Layer[]) =>
  a.length === b.length &&
  a.every((l, i) => {
    const m = b[i];
    return !!m && m.band === l.band && Math.abs(m.y - l.y) < 1e-4 && Math.abs(m.h - l.h) < 1e-4;
  });

/** A projected block's layers, as tall as it is full. */
export const projectedLayers = (t: Pick<Tile, "bands" | "weight">): Layer[] =>
  strataLayers(t.bands, Math.min(1, t.weight / MAX_WEIGHT), TILE_LAYER_MIN);

/**
 * A projected block: dashed and open, its figures approximate. Not a link (yet), so it takes no
 * focus and shows no hover. Its figures change in place as the mempool does; only a tile that
 * forms after the strip is drawn lets its layers settle in.
 */
export function ProjectedTile(props: {
  tile: Accessor<Tile | undefined>;
  network: Network;
  settle: boolean;
}) {
  const t = () => props.tile();
  // Figures change every couple of seconds; layers that did not move keep their elements.
  const layers = createMemo<Layer[]>(
    () => {
      const tile = t();
      return tile ? projectedLayers(tile) : [];
    },
    [],
    { equals: sameLayers },
  );
  const [settling, setSettling] = createSignal(props.settle);
  // Only the first drawing settles: the layers are redrawn when the mempool changes.
  const done = setTimeout(() => setSettling(false), 2_000);
  onCleanup(() => clearTimeout(done));
  return (
    <Show when={t()}>
      {(tile) => (
        <li
          class="home-tile home-tile--projected"
          classList={{ "home-tile--empty": tile().txs === 0 }}
          data-key={tile().key}
        >
          <span class="visually-hidden">{projectedLabel(tile(), props.network)}</span>
          <div class="home-tile__body" aria-hidden="true">
            <span class="home-tile__title">{projectedTitle(tile(), props.network)}</span>
            <span class="home-tile__fee num">
              <Show when={tile().txs > 0} fallback="Nothing waiting">
                <FeeRate rate={tile().medianRate} approx />
              </Show>
            </span>
            <Show when={tile().txs > 0}>
              <span class="home-tile__range num">
                {`${formatFeeRate(tile().minRate)} – ${formatFeeRate(tile().maxRate)}`}
              </span>
            </Show>
            <span class="home-tile__strata">
              <Show
                when={layers().length > 0}
                fallback={<span class="home-tile__fill home-tile__fill--floor" />}
              >
                <span class="home-tile__layers">
                  <Strata layers={layers()} settle={settling()} />
                </span>
              </Show>
            </span>
            <span class="home-tile__foot">
              <span class="num">{groupDigits(tile().txs)} tx</span>
              <span>{tile().slot === 0 && props.network !== "regtest" ? "~10 min" : ""}</span>
            </span>
          </div>
          <span class="home-tile__pool" />
        </li>
      )}
    </Show>
  );
}
