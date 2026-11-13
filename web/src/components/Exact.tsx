import { createSignal, createUniqueId, onCleanup } from "solid-js";
import type { JSX } from "@solidjs/web";
import "./Exact.css";

/** Room kept between the pop-up and the screen's edges, and between it and its figure, in px. */
const EDGE = 8;
const GAP = 4;

/**
 * A figure shown short, its exact value a tap or a key away, in a pop-up over the page: the
 * browser's popover, so no card or scroller clips it, opening it moves nothing, one is open at a
 * time, and a tap outside or Escape closes it. It sits above the figure, inside the screen
 * whichever column it opens from, and closes when the page scrolls.
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
  // Older engines and the test DOM have no popover: the button opens it in the page instead.
  const native = typeof HTMLElement !== "undefined" && "showPopover" in HTMLElement.prototype;
  let pop: HTMLElement | undefined;
  let button: HTMLButtonElement | undefined;
  const close = () => pop?.hidePopover();
  const stop = () => {
    window.removeEventListener("scroll", close, true);
    window.removeEventListener("resize", close);
  };
  onCleanup(stop);

  const place = () => {
    if (!pop || !button) return;
    const b = button.getBoundingClientRect();
    const p = pop.getBoundingClientRect();
    const width = document.documentElement.clientWidth;
    const x = Math.max(EDGE, Math.min(b.left, width - p.width - EDGE));
    const above = b.top - p.height - GAP;
    const y = above >= EDGE ? above : b.bottom + GAP;
    pop.style.setProperty("--x", `${Math.round(x)}px`);
    pop.style.setProperty("--y", `${Math.round(y)}px`);
    pop.dataset["placed"] = "";
  };

  return (
    <>
      <button
        ref={button}
        type="button"
        class={props.class}
        title={props.title}
        popovertarget={id}
        aria-expanded={open() ? "true" : "false"}
        onClick={() => {
          if (!native) setOpen(!open());
        }}
      >
        {props.figure}
      </button>
      <span
        ref={pop}
        id={id}
        class="exact"
        popover="auto"
        hidden={!native && !open()}
        aria-hidden={props.decorative === true ? "true" : undefined}
        onToggle={(e) => {
          const shown = e.newState === "open";
          setOpen(shown);
          if (shown) {
            place();
            window.addEventListener("scroll", close, { capture: true, passive: true });
            window.addEventListener("resize", close, { passive: true });
          } else {
            stop();
            if (pop) delete pop.dataset["placed"];
          }
        }}
      >
        {props.children}
      </span>
    </>
  );
}
