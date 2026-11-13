// What every page promises whatever the reader's font: no text runs into other text, no text is
// cut where it cannot be read whole, and controls keep their labels on one line.
import type { Page } from "@playwright/test";
import { expect } from "./fixtures";

/**
 * Faults in the page as drawn now, in words: text over other text, text cut by a box that does not
 * scroll (unless its whole is in a title, as a long name's or a hash's is), a control's label on
 * two lines, and the page wider than the window.
 */
export function layoutFaults(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    const out: string[] = [];
    interface Box {
      l: number;
      t: number;
      r: number;
      b: number;
    }
    const name = (el: Element) =>
      `${el.tagName.toLowerCase()}${el.classList.length > 0 ? `.${[...el.classList].join(".")}` : ""}`;
    const said = (s: string) => {
      const t = s.replaceAll(/\s+/g, " ").trim();
      return t.length > 40 ? `${t.slice(0, 39)}…` : t;
    };

    // The box an element's content is clipped to, and for each edge whether scrolling can reveal
    // what it cuts: only when the box that sets that edge scrolls on that axis.
    interface Clip {
      box: Box;
      scrolls: Record<keyof Box, boolean>;
    }
    const clips = new Map<Element, Clip | null>();
    const clipOf = (el: Element | null): Clip | null => {
      if (!el || el === document.documentElement) return null;
      const known = clips.get(el);
      if (known !== undefined) return known;
      const up = clipOf(el.parentElement);
      const s = getComputedStyle(el);
      let clip = up;
      if (s.overflowX !== "visible" || s.overflowY !== "visible") {
        const r = el.getBoundingClientRect();
        const l = r.left + el.clientLeft;
        const t = r.top + el.clientTop;
        const x = /auto|scroll/.test(s.overflowX);
        const y = /auto|scroll/.test(s.overflowY);
        const own: Clip = {
          box: { l, t, r: l + el.clientWidth, b: t + el.clientHeight },
          scrolls: { l: x, r: x, t: y, b: y },
        };
        clip = own;
        if (up) {
          // Each edge from whichever box is tighter there; a tie scrolls only if both do.
          const edge = (k: keyof Box, inner: (a: number, b: number) => boolean) => {
            const o = own.box[k];
            const u = up.box[k];
            if (o === u) return { at: o, scrolls: own.scrolls[k] && up.scrolls[k] };
            return inner(o, u)
              ? { at: o, scrolls: own.scrolls[k] }
              : { at: u, scrolls: up.scrolls[k] };
          };
          const larger = (a: number, b: number) => a > b;
          const smaller = (a: number, b: number) => a < b;
          const e = {
            l: edge("l", larger),
            t: edge("t", larger),
            r: edge("r", smaller),
            b: edge("b", smaller),
          };
          clip = {
            box: { l: e.l.at, t: e.t.at, r: e.r.at, b: e.b.at },
            scrolls: { l: e.l.scrolls, t: e.t.scrolls, r: e.r.scrolls, b: e.b.scrolls },
          };
        }
      }
      clips.set(el, clip);
      return clip;
    };

    // Shown at all: not hidden, faded out, in a closed disclosure or only for screen readers.
    const shown = new Map<Element, boolean>();
    const isShown = (el: Element): boolean => {
      const known = shown.get(el);
      if (known !== undefined) return known;
      const yes =
        el.checkVisibility({ opacityProperty: true, visibilityProperty: true }) &&
        el.closest(".visually-hidden") === null;
      shown.set(el, yes);
      return yes;
    };

    // A cut name or hash carries its whole in a title, as the design asks; a shortened one
    // ("a3f9c2e1…7b0d44e9") its two ends.
    const inTitle = (el: Element, text: string) => {
      const whole = (el.closest("[title]")?.getAttribute("title") ?? "").replaceAll(/\s/g, "");
      return text
        .replaceAll(/\s/g, "")
        .split("…")
        .every((part) => whole.includes(part));
    };

    // Every line of text on the page as drawn: what of it is in sight, and whether it was cut.
    const lines: { el: Element; node: Text; box: Box }[] = [];
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    const range = document.createRange();
    for (let n = walker.nextNode(); n; n = walker.nextNode()) {
      const node = n as Text;
      const el = node.parentElement;
      if (!el || (node.textContent ?? "").trim() === "") continue;
      if (el.closest("script, style, noscript, title") || !isShown(el)) continue;
      range.selectNodeContents(node);
      const clip = clipOf(el);
      for (const r of range.getClientRects()) {
        if (r.width < 1 || r.height < 1) continue;
        let box = { l: r.left, t: r.top, r: r.right, b: r.bottom };
        if (clip) {
          const seen = {
            l: Math.max(box.l, clip.box.l),
            t: Math.max(box.t, clip.box.t),
            r: Math.min(box.r, clip.box.r),
            b: Math.min(box.b, clip.box.b),
          };
          // Wholly out of its box's sight: scrolled away or hidden by design.
          if (seen.r - seen.l < 2 || seen.b - seen.t < 2) continue;
          // Cut, on any edge, by a box that scrolling cannot move.
          const cut =
            (seen.l > box.l + 1 && !clip.scrolls.l) ||
            (seen.r < box.r - 1 && !clip.scrolls.r) ||
            (seen.t > box.t + 1 && !clip.scrolls.t) ||
            (seen.b < box.b - 1 && !clip.scrolls.b);
          if (cut && !inTitle(el, node.textContent ?? "")) {
            out.push(`"${said(node.textContent ?? "")}" is cut (${name(el)})`);
          }
          box = seen;
        }
        lines.push({ el, node, box });
      }
    }

    // Text over other text: lines of two different texts that share more than a sliver.
    lines.sort((a, b) => a.box.t - b.box.t);
    const reported = new Set<string>();
    for (let i = 0; i < lines.length; i++) {
      const a = lines[i];
      if (!a) continue;
      for (let j = i + 1; j < lines.length; j++) {
        const b = lines[j];
        if (!b || b.box.t >= a.box.b) break;
        if (a.node === b.node) continue;
        const across = Math.min(a.box.r, b.box.r) - Math.max(a.box.l, b.box.l);
        const down = Math.min(a.box.b, b.box.b) - Math.max(a.box.t, b.box.t);
        const shorter = Math.min(a.box.b - a.box.t, b.box.b - b.box.t);
        if (across > 1 && down > shorter * 0.4) {
          const fault = `"${said(a.node.textContent ?? "")}" (${name(a.el)}) runs into "${said(b.node.textContent ?? "")}" (${name(b.el)})`;
          if (!reported.has(fault)) out.push(fault);
          reported.add(fault);
        }
      }
    }

    // A control's label on one line, unless the control already takes its whole row: a button, a
    // tab or a section link squeezed narrower than its words.
    const CONTROL = "button, [role='tab'], .btn, nav a, summary";
    const rows = new Map<Element, number[]>();
    for (const line of lines) {
      const control = line.el.closest(CONTROL);
      if (control) rows.set(control, [...(rows.get(control) ?? []), line.box.t]);
    }
    for (const [control, tops] of rows) {
      tops.sort((x, y) => x - y);
      const count = tops.filter((t, i) => i === 0 || t - (tops[i - 1] ?? t) > 6).length;
      const row = control.parentElement?.clientWidth ?? 0;
      if (count > 1 && control.getBoundingClientRect().width < row - 1)
        out.push(`the label "${said(control.textContent ?? "")}" wraps (${name(control)})`);
    }

    const wide = document.documentElement.scrollWidth;
    const room = document.documentElement.clientWidth;
    if (wide > room) out.push(`the page is ${wide} px wide in ${room} px`);
    return out;
  });
}

/** Waits two frames, so a layout that follows a resize, a theme or a font has settled. */
export function frames(page: Page): Promise<void> {
  return page.evaluate(
    () =>
      new Promise<void>((resolve) => {
        requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
      }),
  );
}

/** The page holds: no faults, once whatever follows a change has had a moment to settle. */
export async function layoutHolds(page: Page, label: string) {
  await expect(async () => {
    expect(await layoutFaults(page), label).toStrictEqual([]);
  }).toPass({ timeout: 2000 });
}
