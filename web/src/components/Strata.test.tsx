import { describe, expect, it } from "vitest";
import { feeBand } from "../lib/format";
import { bandRange, describeBands, Strata, strataLayers, strataTop } from "./Strata";
import { mount } from "./test-utils";

const zeros = () => Array.from({ length: 10 }, () => 0);
const close = (a: number, b: number) => expect(a).toBeCloseTo(b, 9);

describe("strataLayers", () => {
  it("draws nothing for a block with only its coinbase", () => {
    expect(strataLayers(zeros(), 0.0001, 0.05)).toStrictEqual([]);
  });

  it("fills to the block's fullness with a single band", () => {
    const bands = zeros();
    bands[3] = 500_000;
    expect(strataLayers(bands, 0.5, 0.05)).toStrictEqual([{ band: 4, y: 0, h: 0.5, thin: false }]);
  });

  it("stacks every band, cheapest at the bottom, each as thick as its share", () => {
    const layers = strataLayers(
      [1, 2, 3, 4, 5, 6, 7, 8, 9, 10].map((n) => n * 10_000),
      1,
      0.01,
    );
    expect(layers.map((l) => l.band)).toStrictEqual([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    for (const [i, l] of layers.entries()) close(l.h, (i + 1) / 55);
    for (const [i, l] of layers.entries())
      close(
        l.y,
        layers.slice(0, i).reduce((s, x) => s + x.h, 0),
      );
    close(strataTop(layers), 1);
  });

  it("keeps a tiny band visible at the floor, taking its room from the thick ones", () => {
    const bands = zeros();
    bands[0] = 999_000;
    bands[9] = 150; // one small transaction paying a lot
    const [cheap, dear] = strataLayers(bands, 1, 0.05);
    expect(dear).toMatchObject({ band: 10, h: 0.05, thin: true });
    close(cheap?.h ?? 0, 0.95);
    close(strataTop(strataLayers(bands, 1, 0.05)), 1);
  });

  it("keeps a nearly empty block's true height, its bands sharing it by size", () => {
    const bands = zeros();
    bands[2] = 100;
    bands[6] = 300;
    // Room for both at the floor: each gets it.
    const roomy = strataLayers(bands, 0.12, 0.05);
    close(roomy[0]?.h ?? 0, 0.05);
    close(roomy[1]?.h ?? 0, 0.07);
    // No room: one hairline in all, shared by size, with no seams.
    const layers = strataLayers(bands, 0.0004, 0.05);
    close(layers[0]?.h ?? 0, 0.0125);
    close(layers[1]?.h ?? 0, 0.0375);
    close(strataTop(layers), 0.05);
    expect(layers.every((l) => l.thin)).toBe(true);
  });

  it("never stands taller than the block, however many bands", () => {
    const layers = strataLayers(
      Array.from({ length: 10 }, () => 1),
      0.3,
      0.05,
    );
    expect(layers).toHaveLength(10);
    close(strataTop(layers), 0.3);
    for (const l of layers) close(l.h, 0.03);
  });

  it("ignores values that are not sizes", () => {
    const layers = strataLayers([Number.NaN, -5, 100, 0, 0, 0, 0, 0, 0, 0], 0.2, 0.05);
    expect(layers).toStrictEqual([{ band: 3, y: 0, h: 0.2, thin: false }]);
  });
});

describe("describeBands", () => {
  it("names each band that holds anything, with its size and share", () => {
    const bands = zeros();
    bands[0] = 600_000;
    bands[4] = 395_000;
    bands[9] = 5000;
    expect(describeBands(bands)).toStrictEqual([
      "under 0.20 sat/vB: 600.00 kvB, 60%",
      "1.00 to 1.50 sat/vB: 395.00 kvB, 40%",
      "10.0 sat/vB and up: 5\u202F000 vB, under 1%",
    ]);
    expect(describeBands(zeros())).toStrictEqual([]);
  });

  it("names the bands with the bounds the fee key and feeBand use", () => {
    for (let band = 2; band <= 10; band++) {
      const from = Number(/^([\d.]+)/.exec(bandRange(band))?.[1]);
      expect(feeBand(from)).toBe(band);
      expect(feeBand(from - 0.01)).toBe(band - 1);
    }
  });
});

describe("Strata", () => {
  it("draws one layer per band in its fee color, hidden from screen readers", () => {
    const bands = zeros();
    bands[1] = 10;
    bands[7] = 30;
    const el = mount(() => <Strata layers={strataLayers(bands, 1, 0.05)} settle />);
    const strata = el.querySelector(".strata");
    expect(strata?.getAttribute("aria-hidden")).toBe("true");
    expect(strata?.classList.contains("strata--settle")).toBe(true);
    const layers = [...el.querySelectorAll<HTMLElement>(".strata__layer")];
    expect(layers.map((l) => l.style.getPropertyValue("--c"))).toStrictEqual([
      "var(--fee-2)",
      "var(--fee-8)",
    ]);
    expect(layers.map((l) => l.style.getPropertyValue("--i"))).toStrictEqual(["0", "1"]);
  });
});
