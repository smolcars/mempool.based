import { createSignal, createUniqueId, flush, onCleanup } from "solid-js";
import type { JSX } from "@solidjs/web";
import { openPop } from "./pop";
import "./Exact.css";

/** Room kept between the pop-up and the screen's edges, and between it and its figure, in px. */
const EDGE = 8;
const GAP = 4;

/**
 * A figure shown short, its exact value a tap or a key away, in a pop-up fixed over the page: no
 * card or scroller clips it and opening it moves nothing. It sits above the figure, inside the
 * screen whichever column it opens from; one is open at a time, and a press elsewhere, Escape or
 * a scroll closes it (components/pop.ts).
 */
export function Exact(props: {
  /** The button's class: how the short figure looks. */
  class: string;
  title?: string | undefined;
  /** The short figure, and anything a screen reader hears with it. */
  figure: JSX.Element;
  /** The exact value, and a copy button if it has one. */
  children: JSX.Element;
  /** The figure already says it to a screen reader: the pop-up is for the eyes only. */
  decorative?: boolean;
}) {
  const id = createUniqueId();
  const [open, setOpen] = createSignal(false);
  let button: HTMLButtonElement | undefined;
  let pop: HTMLElement | undefined;
  let close: (() => void) | undefined;
  onCleanup(() => close?.());

  /**
   * Above the figure, or below it where the room above is short; never past either side of the
   * screen, and never over the sticky header (it once covered "Blocks" in Safari).
   */
  const place = () => {
    if (!pop || !button) return;
    const b = button.getBoundingClientRect();
    const p = pop.getBoundingClientRect();
    const width = document.documentElement.clientWidth;
    const x = Math.max(EDGE, Math.min(b.left, width - p.width - EDGE));
    const header = document.querySelector(".site-header")?.getBoundingClientRect().bottom ?? 0;
    const above = b.top - p.height - GAP;
    const fits = above >= Math.max(EDGE, header + GAP);
    pop.style.setProperty("--x", `${Math.round(x)}px`);
    pop.style.setProperty("--y", `${Math.round(fits ? above : b.bottom + GAP)}px`);
  };
  const show = () => {
    setOpen(true);
    // Drawn before it is measured, and placed before the browser paints it.
    flush();
    place();
    close = openPop(
      () => {
        close = undefined;
        setOpen(false);
      },
      () => [button, pop],
      { scroll: true },
    );
  };

  return (
    <>
      <button
        ref={button}
        type="button"
        class={props.class}
        title={props.title}
        aria-expanded={open() ? "true" : "false"}
        aria-controls={id}
        onClick={() => (close ? close() : show())}
      >
        {props.figure}
      </button>
      <span
        ref={pop}
        id={id}
        class="exact"
        hidden={!open()}
        aria-hidden={props.decorative === true ? "true" : undefined}
      >
        {props.children}
      </span>
    </>
  );
}
