// A time-series chart in SVG: lines, areas, steps, percentile bands and stacked shares, with a
// readout that follows the pointer, the finger or the arrow keys (docs/FRONTEND.md "Charts").
// The default export so pages load it with lazy().
import { createMemo, createSignal, createUniqueId, For, onSettled, Show } from "solid-js";
import { FEE_KEY_BOUNDS } from "../../lib/format";
import { DataTable } from "../DataTable";
import {
  areaPath,
  extent,
  layerAt,
  linePath,
  maxGap,
  nearestIndex,
  stackLayout,
  type Value,
} from "./geometry";
import {
  describeSeries,
  formatDay,
  linearAxis,
  logAxis,
  type Scale,
  scale,
  timeTicks,
} from "./scale";
import "./Chart.css";

export interface ChartAxis {
  /** Tick labels. */
  format: (v: number) => string;
  /** A log scale; `"auto"` when the data spans more than three decades. */
  log?: boolean | "auto";
  /** Keep 0 in view (linear only). */
  zero?: boolean;
  /** A fixed domain, such as `[0, 1]` for shares. */
  domain?: readonly [number, number];
  /** Fee rates: the fee scale's colors run along the axis, as the fee key. */
  fee?: boolean;
}

export interface ChartSeries {
  name: string;
  values: readonly Value[];
  draw: "line" | "area" | "step";
  /** Which value axis; `right` needs the chart's `right` axis. */
  axis?: "left" | "right";
  /** 1 is the main series (solid, text color), 2 a second one (dashed, muted). */
  tone?: 1 | 2;
  /** The value in the readout and the figures. */
  format: (v: number) => string;
}

/** A band between two series, such as the 10th to 90th percentile. Bands lie under the lines. */
export interface ChartBand {
  name: string;
  lo: readonly Value[];
  hi: readonly Value[];
  /** 1 is the outer, paler band, 2 the inner one. */
  tone?: 1 | 2;
  format: (v: number) => string;
}

/** One stacked layer on the left axis, the first at the bottom: shares (0–1) or amounts. */
export interface ChartLayer {
  name: string;
  values: readonly Value[];
  /** A fee band (1–10): the layer is that band's fee color, for amounts by fee rate. */
  fee?: number;
  /** A shorter name for the readout, where the key says the rest; `name` is read aloud. */
  short?: string;
}

/** A figure the readout and the table show at each point, without drawing it. */
export interface ChartNote {
  name: string;
  values: readonly Value[];
  format: (v: number) => string;
}

export interface ChartProps {
  /** What the chart shows, as its accessible name: "Hashrate and difficulty, last year". */
  label: string;
  /** The points' times, unix seconds, ascending. */
  x: readonly number[];
  /** The time range drawn; the points' extent when not given. */
  domain?: readonly [number, number] | undefined;
  /**
   * The earliest time the data reaches (docs/API.md "Coverage"). The part of `domain` before it
   * is hatched and says no data is stored there, rather than showing a false zero.
   */
  dataFrom?: number | undefined;
  left: ChartAxis;
  right?: ChartAxis;
  series?: readonly ChartSeries[];
  bands?: readonly ChartBand[];
  layers?: readonly ChartLayer[];
  /** A layer's value in the readout and the figures. */
  layerFormat?: (v: number) => string;
  /** Figures read out after the series, not drawn: a count beside a size. */
  notes?: readonly ChartNote[];
  /**
   * The longest step between neighbouring points that is still one line, when the data has a
   * fixed step: wider is a gap. Three times the median step when not given.
   */
  gap?: number | undefined;
  /** How a point's time reads: a day by default. */
  time?: (t: number) => string;
  /** Replaces the summary written from the series. */
  summary?: string;
  /** Plot height in px. */
  height?: number;
}

const MARGIN = { top: 10, bottom: 26, left: 66, right: 14, rightAxis: 58 };
/** Shades of the text color for stacked layers, alternating so neighbours stand apart. */
const LAYER_MIX = [72, 30, 56, 16, 44, 24, 64, 36, 10, 50];

/** A value as the readout says it, or that there is none. */
const reading = (v: Value | undefined, format: (v: number) => string) =>
  typeof v === "number" ? format(v) : "no data";

const layerMix = (k: number) => `${LAYER_MIX[k % LAYER_MIX.length] ?? 50}%`;

/** A layer's fill: its fee color, or its shade of the text color (Chart.css). */
const layerStyle = (l: ChartLayer | undefined, k: number) =>
  l?.fee === undefined ? { "--mix": layerMix(k) } : { "--fill": `var(--fee-${l.fee})` };

interface AxisFrame {
  scale: Scale;
  ticks: number[];
}

function valueAxis(
  axis: ChartAxis,
  values: readonly (readonly Value[])[],
  range: readonly [number, number],
  count: number,
): AxisFrame {
  if (axis.domain) {
    return { scale: scale(axis.domain, range), ticks: linearAxis(...axis.domain, count).ticks };
  }
  const [lo, hi] = extent(...values) ?? [0, 1];
  const log = axis.log === true || (axis.log === "auto" && lo > 0 && hi / lo > 1000);
  const { domain, ticks } = log ? logAxis(lo, hi) : linearAxis(lo, hi, count, axis.zero);
  return { scale: scale(domain, range, log), ticks };
}

export default function Chart(props: ChartProps) {
  const id = createUniqueId();
  const [width, setWidth] = createSignal(0);
  /** The point the reader is on; undefined shows the latest. */
  const [picked, setPicked] = createSignal<number>();
  const [hoverLayer, setHoverLayer] = createSignal(-1);
  const [figures, setFigures] = createSignal(false);
  let box: HTMLDivElement | undefined;

  onSettled(() => {
    if (!box) return undefined;
    setWidth(box.clientWidth);
    const ro = new ResizeObserver(() => setWidth(box?.clientWidth ?? 0));
    ro.observe(box);
    return () => ro.disconnect();
  });

  const height = () => props.height ?? 240;
  const series = () => props.series ?? [];
  const bands = () => props.bands ?? [];
  const layers = () => props.layers ?? [];
  const notes = () => props.notes ?? [];

  const frame = createMemo(() => {
    const w = width();
    const xs = props.x;
    const first = xs[0];
    const last = xs.at(-1);
    if (w <= 0 || first === undefined || last === undefined) return undefined;
    const h = height();
    const left = MARGIN.left;
    const right = w - (props.right ? MARGIN.rightAxis : MARGIN.right);
    const bottom = h - MARGIN.bottom;
    const domain = props.domain ?? [first, first === last ? first + 1 : last];
    const sx = scale(domain, [left, right]);
    const yRange = [bottom, MARGIN.top] as const;
    const count = Math.max(2, Math.round((bottom - MARGIN.top) / 50));
    const stack = stackLayout(
      layers().map((l) => l.values),
      xs.length,
    );
    const on = (side: "left" | "right") =>
      series()
        .filter((s) => (s.axis ?? "left") === side)
        .map((s) => s.values);
    const yl = valueAxis(
      props.left,
      [...on("left"), ...bands().flatMap((b) => [b.lo, b.hi]), ...stack.map((s) => s.hi)],
      yRange,
      count,
    );
    const yr = props.right ? valueAxis(props.right, on("right"), yRange, count) : undefined;
    const gap = props.gap ?? maxGap(xs);
    const yOf = (s: ChartSeries) => ((s.axis ?? "left") === "right" && yr ? yr : yl).scale;
    const floor = yl.scale(yl.scale.domain[0]);
    const time = timeTicks(domain[0], domain[1], Math.max(2, Math.floor((right - left) / 84)));
    const covered = props.dataFrom;
    return {
      w,
      h,
      left,
      right,
      bottom,
      sx,
      yl,
      yr,
      yOf,
      time,
      hatch:
        covered !== undefined && covered > domain[0]
          ? { x: left, w: Math.min(right, sx(covered)) - left }
          : undefined,
      layers: stack.map((s) => areaPath(xs, s.hi, s.lo, sx, yl.scale, gap)),
      stack,
      bands: bands().map((b) => areaPath(xs, b.hi, b.lo, sx, yl.scale, gap)),
      areas: series().map((s) =>
        s.draw === "area" ? areaPath(xs, s.values, yOf(s).domain[0], sx, yOf(s), gap) : "",
      ),
      lines: series().map((s) => linePath(xs, s.values, sx, yOf(s), gap, s.draw === "step")),
      floor,
      fee: props.left.fee === true ? feeStrip(yl.scale) : [],
    };
  });

  /** The latest point any series has data for. */
  const latest = () => {
    for (let i = props.x.length - 1; i >= 0; i--) {
      if (
        series().some((s) => typeof s.values[i] === "number") ||
        bands().some((b) => typeof b.hi[i] === "number") ||
        layers().some((l) => typeof l.values[i] === "number")
      )
        return i;
    }
    return props.x.length - 1;
  };
  const index = () => Math.min(picked() ?? latest(), props.x.length - 1);
  const when = (i: number) => {
    const t = props.x[i];
    return t === undefined ? "" : (props.time ?? formatDay)(t);
  };
  const layerFormat = (v: number) => (props.layerFormat ?? String)(v);

  /** The figures at one point, as the readout shows them, with the key each is drawn with. */
  const readings = (i: number) => [
    ...bands().map((b) => ({
      kind: `band-${b.tone ?? 1}`,
      style: undefined,
      layer: -1,
      name: b.name,
      short: undefined,
      value:
        typeof b.lo[i] !== "number" || typeof b.hi[i] !== "number"
          ? "no data"
          : `${b.format(b.lo[i] ?? 0)} to ${b.format(b.hi[i] ?? 0)}`,
    })),
    ...series().map((s) => ({
      kind: `line-${s.tone ?? 1}`,
      style: undefined,
      layer: -1,
      name: s.name,
      short: undefined,
      value: reading(s.values[i], s.format),
    })),
    ...notes().map((s) => ({
      kind: "note",
      style: undefined,
      layer: -1,
      name: s.name,
      short: undefined,
      value: reading(s.values[i], s.format),
    })),
    ...layers().map((l, k) => ({
      kind: l.fee === undefined ? "layer" : "fee",
      style: layerStyle(l, k),
      layer: k,
      name: l.name,
      short: l.short,
      value: reading(l.values[i], layerFormat),
    })),
  ];
  const valueText = () => {
    const i = index();
    return `${when(i)}: ${readings(i)
      .map((r) => `${r.name} ${r.value}`)
      .join(", ")}`;
  };

  const summary = () =>
    props.summary ??
    [
      ...series().map((s) => describeSeries(s.name, props.x, s.values, s.format)),
      ...bands().map((b) => describeSeries(`${b.name}, upper`, props.x, b.hi, b.format)),
    ].join(" ");

  const pick = (e: PointerEvent) => {
    const f = frame();
    const el = e.currentTarget;
    if (!f || !(el instanceof Element)) return;
    const r = el.getBoundingClientRect();
    const i = nearestIndex(props.x, f.sx.invert(e.clientX - r.left));
    if (i >= 0) setPicked(i);
    setHoverLayer(layerAt(f.stack, i, f.yl.scale.invert(e.clientY - r.top)));
  };
  const leave = (e: PointerEvent) => {
    // A finger lifts: keep its reading, as a tap shows it.
    if (e.pointerType === "touch") return;
    setPicked(undefined);
    setHoverLayer(-1);
  };
  const key = (e: KeyboardEvent) => {
    const n = props.x.length;
    const steps: Record<string, number> = {
      ArrowLeft: -1,
      ArrowDown: -1,
      ArrowRight: 1,
      ArrowUp: 1,
      PageDown: -10,
      PageUp: 10,
    };
    const step = steps[e.key];
    let next: number | undefined;
    if (step !== undefined) next = index() + step;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = n - 1;
    else if (e.key === "Escape") {
      setPicked(undefined);
      return;
    }
    if (next === undefined) return;
    e.preventDefault();
    setPicked(Math.max(0, Math.min(n - 1, next)));
  };

  const columns = () => [
    { header: "Time", cell: (i: number) => when(i) },
    ...readings(0).map((r, k) => ({
      header: r.name,
      numeric: true,
      cell: (i: number) => readings(i)[k]?.value ?? "",
    })),
  ];

  return (
    <figure class="chart">
      <div class="chart__readout" aria-hidden="true">
        <span class="chart__when num">{when(index())}</span>
        <ul class="chart__values">
          <For keyed={false} each={readings(index())}>
            {(r) => (
              <li
                class={[
                  "chart__value",
                  {
                    "chart__value--on": r().layer >= 0 && r().layer === hoverLayer(),
                  },
                ]}
              >
                <Show when={r().kind !== "note"}>
                  <span class={`chart__key chart__key--${r().kind}`} style={r().style} />
                </Show>
                <span class="chart__name">{r().short ?? r().name}</span>
                <span class="chart__figure num">{r().value}</span>
              </li>
            )}
          </For>
        </ul>
      </div>
      <div
        class="chart__plot"
        ref={box}
        style={{ "--h": `${height()}px` }}
        tabindex="0"
        role="slider"
        aria-label={props.label}
        aria-describedby={`${id}-summary`}
        aria-valuemin={0}
        aria-valuemax={Math.max(0, props.x.length - 1)}
        aria-valuenow={index()}
        aria-valuetext={valueText()}
        onKeyDown={key}
      >
        <Show when={frame()}>
          {(f) => (
            <svg
              class="chart__svg"
              width={f().w}
              height={f().h}
              viewBox={`0 0 ${f().w} ${f().h}`}
              aria-hidden="true"
              onPointerMove={pick}
              onPointerDown={pick}
              onPointerLeave={leave}
            >
              <defs>
                <pattern
                  id={`${id}-hatch`}
                  width="6"
                  height="6"
                  patternUnits="userSpaceOnUse"
                  patternTransform="rotate(45)"
                >
                  <line class="chart__hatch-line" x1="0" y1="0" x2="0" y2="6" />
                </pattern>
              </defs>
              <For each={f().yl.ticks}>
                {(t) => (
                  <g class="chart__tick">
                    <line
                      class="chart__grid"
                      x1={f().left}
                      x2={f().right}
                      y1={f().yl.scale(t)}
                      y2={f().yl.scale(t)}
                    />
                    <text
                      class="chart__label"
                      x={f().left - 10}
                      y={f().yl.scale(t)}
                      text-anchor="end"
                      dy="0.32em"
                    >
                      {props.left.format(t)}
                    </text>
                  </g>
                )}
              </For>
              <Show when={f().yr}>
                {(yr) => (
                  <For each={yr().ticks}>
                    {(t) => (
                      <text
                        class="chart__label chart__label--right"
                        x={f().right + 8}
                        y={yr().scale(t)}
                        dy="0.32em"
                      >
                        {props.right?.format(t)}
                      </text>
                    )}
                  </For>
                )}
              </Show>
              <For each={f().fee}>
                {(b) => (
                  <rect
                    class="chart__fee"
                    x={f().left - 5}
                    width="3"
                    y={b.y}
                    height={b.h}
                    style={{ fill: `var(--fee-${b.band})` }}
                  />
                )}
              </For>
              <Show when={f().hatch}>
                {(hatch) => (
                  <g class="chart__nodata">
                    <rect
                      x={hatch().x}
                      y={MARGIN.top}
                      width={Math.max(0, hatch().w)}
                      height={f().bottom - MARGIN.top}
                      fill={`url(#${id}-hatch)`}
                    />
                    <Show when={hatch().w > 110}>
                      <text
                        class="chart__nodata-text"
                        x={hatch().x + hatch().w / 2}
                        y={(f().bottom + MARGIN.top) / 2}
                        text-anchor="middle"
                      >
                        No data stored
                      </text>
                    </Show>
                  </g>
                )}
              </Show>
              <For each={f().layers}>
                {(d, k) => (
                  <path
                    class={[
                      "chart__layer",
                      {
                        "chart__layer--fee": layers()[k()]?.fee !== undefined,
                        "chart__layer--on": hoverLayer() === k(),
                      },
                    ]}
                    d={d}
                    style={layerStyle(layers()[k()], k())}
                  />
                )}
              </For>
              <For each={f().bands}>
                {(d, k) => (
                  <path class={`chart__band chart__band--${bands()[k()]?.tone ?? 1}`} d={d} />
                )}
              </For>
              <For each={f().areas}>{(d) => <path class="chart__area" d={d} />}</For>
              <For each={f().lines}>
                {(d, k) => (
                  <path class={`chart__line chart__line--${series()[k()]?.tone ?? 1}`} d={d} />
                )}
              </For>
              <line
                class="chart__axis"
                x1={f().left}
                x2={f().right}
                y1={f().bottom}
                y2={f().bottom}
              />
              <For each={f().time.ticks}>
                {(t) => (
                  <text class="chart__label" x={f().sx(t)} y={f().bottom + 18} text-anchor="middle">
                    {f().time.label(t)}
                  </text>
                )}
              </For>
              <Show when={props.x[index()]}>
                {(t) => (
                  <g class="chart__cursor">
                    <line x1={f().sx(t())} x2={f().sx(t())} y1={MARGIN.top} y2={f().bottom} />
                    <For each={series()}>
                      {(s) => (
                        <Show when={typeof s.values[index()] === "number"}>
                          <circle
                            class={`chart__dot chart__dot--${s.tone ?? 1}`}
                            cx={f().sx(t())}
                            cy={f().yOf(s)(s.values[index()] ?? 0)}
                            r="3.5"
                          />
                        </Show>
                      )}
                    </For>
                  </g>
                )}
              </Show>
            </svg>
          )}
        </Show>
      </div>
      <figcaption class="chart__caption">
        <p id={`${id}-summary`} class="visually-hidden">
          {summary()}
        </p>
        <details class="chart__figures" onToggle={(e) => setFigures(e.currentTarget.open)}>
          <summary>Show the figures</summary>
          <Show when={figures()}>
            {/* It scrolls, so it takes focus: the keyboard can scroll it too. */}
            <div
              class="chart__table"
              // oxlint-disable-next-line jsx-a11y/no-noninteractive-tabindex -- a scrolling region must take focus to be scrolled by keyboard
              tabindex="0"
              role="region"
              aria-label={`${props.label}, figures`}
            >
              <DataTable
                caption={props.label}
                columns={columns()}
                rows={Array.from({ length: props.x.length }, (_, i) => props.x.length - 1 - i)}
              />
            </div>
          </Show>
        </details>
      </figcaption>
    </figure>
  );
}

/** The fee scale along a fee-rate axis: one strip per band, where the axis shows it. */
function feeStrip(sy: Scale): { band: number; y: number; h: number }[] {
  const [lo, hi] = sy.domain;
  return FEE_KEY_BOUNDS.flatMap((from, k) => {
    const a = Math.max(k === 0 ? lo : from, lo);
    const b = Math.min(FEE_KEY_BOUNDS[k + 1] ?? hi, hi);
    if (b <= a) return [];
    const y = sy(b);
    return [{ band: k + 1, y, h: sy(a) - y }];
  });
}
