// Paths, gaps, the stacked layout and hit testing for the charts. Pure.

/** A value, or null where there is no data: the line breaks there rather than dropping to zero. */
export type Value = number | null;

type Px = (v: number) => number;

/** A sorted copy. */
export function sorted<T>(xs: Iterable<T>, by: (a: T, b: T) => number): T[] {
  // oxlint-disable-next-line unicorn/no-array-sort -- sorts a fresh array; toSorted is newer than the browsers we build for
  return [...xs].sort(by);
}

/**
 * The longest step between neighbouring points that is still one line: three times the median
 * step. Farther apart, the points are two runs with no data between them.
 */
export function maxGap(xs: readonly number[]): number {
  if (xs.length < 2) return Number.POSITIVE_INFINITY;
  const steps = sorted(
    xs.slice(1).map((x, i) => x - (xs[i] ?? x)),
    (a, b) => a - b,
  );
  return (steps[Math.floor(steps.length / 2)] ?? 0) * 3;
}

/** Index ranges `[start, end]` of consecutive points with data and no gap wider than `gap`. */
export function runs(
  xs: readonly number[],
  has: (i: number) => boolean,
  gap: number,
): [number, number][] {
  const out: [number, number][] = [];
  let start = -1;
  for (let i = 0; i < xs.length; i++) {
    const ok = has(i);
    const joined = start !== -1 && (xs[i] ?? 0) - (xs[i - 1] ?? 0) <= gap;
    if (start !== -1 && (!ok || !joined)) {
      out.push([start, i - 1]);
      start = -1;
    }
    if (ok && start === -1) start = i;
  }
  if (start !== -1) out.push([start, xs.length - 1]);
  return out;
}

const n = (v: number) => (Math.round(v * 10) / 10).toString();

/** A line through the points, broken at gaps. `step` holds each value until the next point. */
export function linePath(
  xs: readonly number[],
  ys: readonly Value[],
  sx: Px,
  sy: Px,
  gap: number,
  step = false,
): string {
  let d = "";
  for (const [a, b] of runs(xs, (i) => typeof ys[i] === "number", gap)) {
    for (let i = a; i <= b; i++) {
      const x = sx(xs[i] ?? 0);
      const y = sy(ys[i] ?? 0);
      if (i === a) d += `M${n(x)} ${n(y)}`;
      else if (step) d += `H${n(x)}V${n(y)}`;
      else d += `L${n(x)} ${n(y)}`;
    }
    // A lone point still shows: a short tick.
    if (a === b) d += "h0.1";
  }
  return d;
}

/**
 * The area between `hi` and `lo` (a constant baseline when `lo` is a number), broken at gaps:
 * filled areas under a line, and bands between percentiles. With `sliver`, a lone point is a
 * sliver that many px wide; without, it draws nothing, as the mining charts always have.
 */
export function areaPath(
  xs: readonly number[],
  hi: readonly Value[],
  lo: readonly Value[] | number,
  sx: Px,
  sy: Px,
  gap: number,
  sliver = 0,
): string {
  const low = (i: number) => (typeof lo === "number" ? lo : lo[i]);
  let d = "";
  for (const [a, b] of runs(
    xs,
    (i) => typeof hi[i] === "number" && typeof low(i) === "number",
    gap,
  )) {
    if (a === b && sliver > 0) {
      const x = sx(xs[a] ?? 0);
      const w = sliver / 2;
      const t = n(sy(hi[a] ?? 0));
      const f = n(sy(low(a) ?? 0));
      d += `M${n(x - w)} ${t}L${n(x + w)} ${t}L${n(x + w)} ${f}L${n(x - w)} ${f}Z`;
      continue;
    }
    let top = "";
    let bottom = "";
    for (let i = a; i <= b; i++) {
      top += `${i === a ? "M" : "L"}${n(sx(xs[i] ?? 0))} ${n(sy(hi[i] ?? 0))}`;
    }
    for (let i = b; i >= a; i--) bottom += `L${n(sx(xs[i] ?? 0))} ${n(sy(low(i) ?? 0))}`;
    d += `${top}${bottom}Z`;
  }
  return d;
}

/**
 * Stacked shares: for each layer, its floor and ceiling at each point, the first layer at the
 * bottom. A point where no layer has data is a gap in every layer.
 */
export function stackLayout(
  layers: readonly (readonly Value[])[],
  length: number,
): {
  lo: Value[];
  hi: Value[];
}[] {
  const floor: Value[] = Array.from({ length }, (_, i) =>
    layers.some((l) => typeof l[i] === "number") ? 0 : null,
  );
  return layers.map((l) => {
    const lo = [...floor];
    const hi = floor.map((f, i) => (f === null ? null : f + (l[i] ?? 0)));
    for (const [i, h] of hi.entries()) floor[i] = h;
    return { lo, hi };
  });
}

/** The index of the point nearest `x` in ascending `xs`, or -1 when there are none. */
export function nearestIndex(xs: readonly number[], x: number): number {
  if (xs.length === 0) return -1;
  let lo = 0;
  let hi = xs.length - 1;
  while (hi - lo > 1) {
    const mid = (lo + hi) >> 1;
    if ((xs[mid] ?? 0) <= x) lo = mid;
    else hi = mid;
  }
  return Math.abs((xs[hi] ?? 0) - x) < Math.abs(x - (xs[lo] ?? 0)) ? hi : lo;
}

/** Which stacked layer holds the value `y` at point `i`, or -1 for none. */
export function layerAt(
  stack: readonly { lo: readonly Value[]; hi: readonly Value[] }[],
  i: number,
  y: number,
): number {
  return stack.findIndex((l) => {
    const lo = l.lo[i];
    const hi = l.hi[i];
    return typeof lo === "number" && typeof hi === "number" && hi > lo && y >= lo && y <= hi;
  });
}

/** The smallest and largest of the values with data, or undefined when none has. */
export function extent(...lists: readonly (readonly Value[])[]): [number, number] | undefined {
  let lo = Number.POSITIVE_INFINITY;
  let hi = Number.NEGATIVE_INFINITY;
  for (const l of lists) {
    for (const v of l) {
      if (typeof v === "number" && Number.isFinite(v)) {
        if (v < lo) lo = v;
        if (v > hi) hi = v;
      }
    }
  }
  return lo <= hi ? [lo, hi] : undefined;
}
