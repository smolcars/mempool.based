// Motion tokens read from the CSS (docs/DESIGN.md "Motion", rule 2), so reduced motion applies here too.

export interface Motion {
  /** Reduced motion: nothing moves, short fades remain. */
  reduced: boolean;
  slow: number;
  block: number;
  fade: number;
  stagger: number;
  easeOut: string;
  easeInOut: string;
  easeSettle: string;
}

/** A CSS time in ms. The build's minifier rewrites `900ms` as `.9s`, so both units are read. */
export function toMs(value: string): number {
  const v = value.trim();
  const n = Number.parseFloat(v);
  if (!Number.isFinite(n)) return 0;
  return v.endsWith("ms") ? n : v.endsWith("s") ? n * 1000 : n;
}

export function motion(): Motion {
  const css = getComputedStyle(document.documentElement);
  const ms = (name: string) => toMs(css.getPropertyValue(name));
  const ease = (name: string) => css.getPropertyValue(name).trim() || "ease-out";
  return {
    reduced: matchMedia("(prefers-reduced-motion: reduce)").matches,
    slow: ms("--dur-slow"),
    block: ms("--dur-block"),
    fade: ms("--dur-fade"),
    stagger: ms("--stagger"),
    easeOut: ease("--ease-out"),
    easeInOut: ease("--ease-in-out"),
    easeSettle: ease("--ease-settle"),
  };
}
