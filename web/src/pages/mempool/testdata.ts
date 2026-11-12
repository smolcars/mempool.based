// Made-up history answers for the tests.
import type { History } from "./history";

export const T0 = Date.UTC(2026, 10, 12, 14, 0) / 1000;

export function point(time: number, over: Partial<History["points"][number]> = {}) {
  return {
    time,
    samples: 1,
    count: 1000,
    vsize: 550_000,
    fees: 100_000,
    min_fee_rate: 100,
    bands: [100_000, 0, 0, 50_000, 400_000, 0, 0, 0, 0, 0],
    in_count: 10,
    in_vsize: 6000,
    out_count: 5,
    out_vsize: 3000,
    mined_count: 4,
    mined_vsize: 2400,
    ...over,
  };
}

export function history(over: Partial<History> = {}): History {
  return {
    interval: "2h",
    from: T0,
    to: T0 + 7200,
    step: 60,
    first: T0,
    minutes: 120,
    sampled: 120,
    complete: true,
    points: [point(T0)],
    ...over,
  };
}
