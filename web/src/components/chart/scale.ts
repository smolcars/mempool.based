// Scales, ticks and value formats for the charts (docs/FRONTEND.md "Charts"). Pure.
import { formatPowerOfTen, formatUtc, MISSING, MONTHS, NNBSP } from "../../lib/format";

/** Maps a value in `domain` to a pixel in `range`, linearly or by its logarithm. */
export interface Scale {
  (v: number): number;
  domain: readonly [number, number];
  log: boolean;
  /** The inverse: a pixel back to a value. */
  invert: (px: number) => number;
}

export function scale(
  domain: readonly [number, number],
  range: readonly [number, number],
  log = false,
): Scale {
  const t = log ? Math.log10 : (v: number) => v;
  const d0 = t(domain[0]);
  const span = t(domain[1]) - d0 || 1;
  const [r0, r1] = range;
  const s = (v: number) => r0 + ((t(v) - d0) / span) * (r1 - r0);
  const invert = (px: number) => {
    const u = d0 + ((px - r0) / (r1 - r0 || 1)) * span;
    return log ? 10 ** u : u;
  };
  return Object.assign(s, { domain, log, invert });
}

/** A round step near `raw`: 1, 2 or 5 times a power of ten. */
export function niceStep(raw: number): number {
  if (!(raw > 0) || !Number.isFinite(raw)) return 1;
  const p = 10 ** Math.floor(Math.log10(raw));
  const f = raw / p;
  return (f <= 1 ? 1 : f <= 2 ? 2 : f <= 5 ? 5 : 10) * p;
}

/**
 * A value axis's domain and ticks: about `count` round ticks covering `[lo, hi]`, the domain
 * widened to the outer ticks so the line never touches the frame. `zero` keeps 0 in view.
 */
export function linearAxis(
  lo: number,
  hi: number,
  count: number,
  zero = false,
): { domain: [number, number]; ticks: number[] } {
  let a = zero ? Math.min(0, lo) : lo;
  let b = zero ? Math.max(0, hi) : hi;
  if (!Number.isFinite(a) || !Number.isFinite(b)) return { domain: [0, 1], ticks: [0, 1] };
  if (a === b) {
    const pad = Math.abs(a) * 0.1 || 1;
    a = zero && a >= 0 ? 0 : a - pad;
    b += pad;
  }
  const step = niceStep((b - a) / Math.max(1, count));
  const start = Math.floor(a / step) * step;
  const end = Math.ceil(b / step) * step;
  const ticks: number[] = [];
  // Rounded to the step's precision, so 0.1 + 0.2 never prints as 0.30000000000000004.
  const digits = Math.max(0, -Math.floor(Math.log10(step)) + 1);
  for (let v = start; v <= end + step / 2; v += step) ticks.push(Number(v.toFixed(digits)));
  return { domain: [start, end], ticks };
}

/**
 * A log axis over `[lo, hi]` (both above 0): powers of ten, with 2 and 5 between them when the
 * range spans fewer than three decades, and the domain widened to the outer ticks.
 */
export function logAxis(lo: number, hi: number): { domain: [number, number]; ticks: number[] } {
  if (!(lo > 0) || !(hi > 0) || !Number.isFinite(hi)) return { domain: [1, 10], ticks: [1, 10] };
  const e0 = Math.floor(Math.log10(lo));
  const e1 = Math.ceil(Math.log10(Math.max(hi, lo * 1.0001)));
  const steps = Math.log10(hi / lo) < 3 ? [1, 2, 5] : [1];
  const ticks: number[] = [];
  for (let e = e0; e <= e1; e++) {
    for (const m of steps) {
      const v = m * 10 ** e;
      if (v >= lo / 1.0001 && v <= hi * 1.0001) ticks.push(v);
    }
  }
  // Tight bounds when a decade is mostly empty; the ticks inside still read.
  const domain: [number, number] =
    ticks.length >= 2 ? [Math.min(lo, ticks[0] ?? lo), hi] : [lo, hi];
  if (domain[0] === domain[1]) domain[1] = domain[0] * 10;
  return { domain, ticks: ticks.length > 0 ? ticks : [lo] };
}

const HOUR = 3600;
const DAY = 86_400;

/** Tick steps for a time axis, finest first. Months and years step by calendar. */
const TIME_STEPS: readonly { unit: "s" | "month" | "year"; n: number; approx: number }[] = [
  { unit: "s", n: HOUR, approx: HOUR },
  { unit: "s", n: 3 * HOUR, approx: 3 * HOUR },
  { unit: "s", n: 6 * HOUR, approx: 6 * HOUR },
  { unit: "s", n: 12 * HOUR, approx: 12 * HOUR },
  { unit: "s", n: DAY, approx: DAY },
  { unit: "s", n: 2 * DAY, approx: 2 * DAY },
  { unit: "s", n: 7 * DAY, approx: 7 * DAY },
  { unit: "s", n: 14 * DAY, approx: 14 * DAY },
  { unit: "month", n: 1, approx: 30 * DAY },
  { unit: "month", n: 3, approx: 91 * DAY },
  { unit: "month", n: 6, approx: 182 * DAY },
  { unit: "year", n: 1, approx: 365 * DAY },
  { unit: "year", n: 2, approx: 730 * DAY },
  { unit: "year", n: 5, approx: 1826 * DAY },
  { unit: "year", n: 10, approx: 3652 * DAY },
];

const utc = (t: number) => new Date(t * 1000);
const two = (n: number) => String(n).padStart(2, "0");

/** A day in words, UTC: `3 Nov 2026`. */
export function formatDay(t: number): string {
  if (!Number.isFinite(t)) return MISSING;
  const d = utc(t);
  return `${d.getUTCDate()} ${MONTHS[d.getUTCMonth()] ?? ""} ${d.getUTCFullYear()}`;
}

/** A moment to the minute, UTC: `3 Nov 2026, 14:00 UTC`. */
export function formatMoment(t: number): string {
  return formatUtc(t, false);
}

/**
 * The lines a narrow chart's readout takes, one reading to a line: the time, each band, series,
 * note and layer, and one line for all the fee bands, which it names one at a time.
 */
export function readoutLines(p: {
  bands?: readonly unknown[] | undefined;
  series?: readonly unknown[] | undefined;
  notes?: readonly unknown[] | undefined;
  layers?: readonly { fee?: number | undefined }[] | undefined;
}): number {
  const layers = p.layers ?? [];
  const plain = layers.filter((l) => l.fee === undefined).length;
  const fee = layers.length > plain ? 1 : 0;
  return (
    1 + (p.bands?.length ?? 0) + (p.series?.length ?? 0) + (p.notes?.length ?? 0) + plain + fee
  );
}

/**
 * The two ends of a time axis too narrow for ticks (a phone): the first and last moment, each
 * saying as much as the span needs. Under two days the hours lead, the first with its day:
 * `12 Nov 11:14` and `20:35`; longer, the days: `9 Nov` and `12 Nov`, with the years when they differ, or `Nov 2020` and `Nov 2026` where
 * `room` (px) is short.
 */
export function endLabels(from: number, to: number, room = Infinity): [string, string] {
  const a = utc(from);
  const b = utc(to);
  const day = (d: Date) => `${d.getUTCDate()} ${MONTHS[d.getUTCMonth()] ?? ""}`;
  const hm = (d: Date) => `${two(d.getUTCHours())}:${two(d.getUTCMinutes())}`;
  if (to - from < 2 * 86_400) {
    return [`${day(a)} ${hm(a)}`, a.getUTCDate() === b.getUTCDate() ? hm(b) : `${day(b)} ${hm(b)}`];
  }
  if (a.getUTCFullYear() !== b.getUTCFullYear()) {
    // Two whole dates need about 150 px in a wide face: a narrower plot (a pool's, between two axes) names months.
    if (room < 170) {
      const month = (d: Date) => `${MONTHS[d.getUTCMonth()] ?? ""} ${d.getUTCFullYear()}`;
      return [month(a), month(b)];
    }
    return [formatDay(from), formatDay(to)];
  }
  return [day(a), day(b)];
}

/**
 * Ticks for a time axis over `[from, to]` (unix seconds), at most `max` of them, on round UTC
 * moments: hours, days, Mondays, months or years. Labels say no more than the step needs:
 * `14:00` (the day, `3 Nov`, at midnight), `3 Nov`, `Nov` (`Jan 2026` at a year's start), `2026`.
 */
export function timeTicks(
  from: number,
  to: number,
  max: number,
): { ticks: number[]; label: (t: number) => string } {
  const span = Math.max(1, to - from);
  const step =
    TIME_STEPS.find((s) => span / s.approx <= Math.max(1, max) * 1.25) ?? TIME_STEPS.at(-1);
  if (!step) return { ticks: [], label: formatDay };
  const ticks: number[] = [];
  if (step.unit === "s") {
    // Weeks start on Monday: the Unix epoch was a Thursday, 3 days after one.
    const offset = step.n === 7 * DAY || step.n === 14 * DAY ? -3 * DAY : 0;
    for (let t = Math.ceil((from - offset) / step.n) * step.n + offset; t <= to; t += step.n)
      ticks.push(t);
  } else {
    const d = utc(from);
    let y = d.getUTCFullYear();
    let m = step.unit === "year" ? 0 : d.getUTCMonth();
    if (step.unit === "year") y = Math.ceil(y / step.n) * step.n;
    else m = Math.ceil(m / step.n) * step.n;
    for (;;) {
      const t = Date.UTC(y, m, 1) / 1000;
      if (t > to) break;
      if (t >= from) ticks.push(t);
      if (step.unit === "year") y += step.n;
      else m += step.n;
    }
  }
  const label = (t: number) => {
    const d = utc(t);
    if (step.unit === "year") return String(d.getUTCFullYear());
    if (step.unit === "month") {
      const name = MONTHS[d.getUTCMonth()] ?? "";
      return d.getUTCMonth() === 0 ? `${name} ${d.getUTCFullYear()}` : name;
    }
    // Hours, with the day named at midnight so a week of them still says which day is which.
    if (step.n < DAY && t % DAY !== 0) return `${two(d.getUTCHours())}:${two(d.getUTCMinutes())}`;
    return `${d.getUTCDate()} ${MONTHS[d.getUTCMonth()] ?? ""}`;
  };
  return { ticks, label };
}

const PREFIXES = ["", "k", "M", "G", "T", "P", "E", "Z", "Y"];

/**
 * A large figure with an SI prefix and three significant digits: `952 EH/s`, `1.04 ZH/s`,
 * `149 T`. `digits` sets the significant digits.
 */
export function formatSi(v: number, unit: string, digits = 3): string {
  if (!Number.isFinite(v)) return MISSING;
  const sign = v < 0 ? "−" : "";
  let a = Math.abs(v);
  // So small that its zeros would run wider than a phone (regtest's difficulty, 0.00000000046565),
  // or past the last prefix: the power of ten keeps it short, and three digits say its size.
  if (a > 0 && (a < 0.001 || a >= 999.5e24)) {
    return `${sign}${formatPowerOfTen(a, Math.min(digits, 3))}${unit ? `${NNBSP}${unit}` : ""}`;
  }
  let i = 0;
  while (a >= 999.5 && i < PREFIXES.length - 1) {
    a /= 1000;
    i++;
  }
  // Digits before the point; negative below 0.1, so small figures keep their significant digits.
  const whole = a > 0 ? Math.floor(Math.log10(a)) + 1 : 1;
  const fixed = a.toFixed(Math.max(0, digits - whole));
  // Trailing zeros after the point say nothing: `1.00 ZH/s` reads as `1 ZH/s`.
  const shown = fixed.includes(".") ? fixed.replace(/\.?0+$/, "") : fixed;
  const prefix = PREFIXES[i] ?? "";
  return `${sign}${shown}${unit || prefix ? " " : ""}${prefix}${unit}`;
}

/** Hashes a second: `952 EH/s`. */
export const formatHashrate = (hps: number, digits = 3): string => formatSi(hps, "H/s", digits);

/** A share, 0–1, as a percentage: `31.2%`, `0.4%`, `under 0.1%`. */
export function formatShare(share: number): string {
  if (!Number.isFinite(share)) return MISSING;
  const pct = share * 100;
  if (pct > 0 && pct < 0.1) return "under 0.1%";
  return pct === 0 ? "0%" : `${pct.toFixed(1)}%`;
}

/**
 * One series in words, for a screen reader: where it started and ended, and its range.
 * `Hashrate went from 820 EH/s on 12 Nov 2025 to 952 EH/s on 12 Nov 2026; lowest 790 EH/s, highest 980 EH/s.`
 */
export function describeSeries(
  name: string,
  xs: readonly number[],
  values: readonly (number | null)[],
  format: (v: number) => string,
): string {
  const points = xs.flatMap((t, i) => {
    const v = values[i];
    return typeof v !== "number" || !Number.isFinite(v) ? [] : [{ t, v }];
  });
  const first = points[0];
  const last = points.at(-1);
  if (!first || !last) return `${name}: no data.`;
  if (points.length === 1) return `${name}: ${format(first.v)} on ${formatDay(first.t)}.`;
  const lo = points.reduce((a, b) => (b.v < a.v ? b : a));
  const hi = points.reduce((a, b) => (b.v > a.v ? b : a));
  return `${name} went from ${format(first.v)} on ${formatDay(first.t)} to ${format(last.v)} on ${formatDay(last.t)}; lowest ${format(lo.v)}, highest ${format(hi.v)}.`;
}
