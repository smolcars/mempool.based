import { onCleanup } from "solid-js";

/**
 * Use as a `ref` on anything that scrolls sideways. It marks the edges that hide more,
 * `data-more-start` and `data-more-end`, and global.css fades them, so a cut-off column reads as
 * "there is more this way", not as a fault. While it scrolls, it also takes keyboard focus, so the
 * arrow keys can reach what is off to the side.
 */
export function scrollCue(el: HTMLElement): void {
  const mark = () => {
    const more = el.scrollWidth - el.clientWidth;
    el.toggleAttribute("data-more-start", el.scrollLeft > 1);
    el.toggleAttribute("data-more-end", el.scrollLeft < more - 1);
    if (more > 1) el.tabIndex = 0;
    else el.removeAttribute("tabindex");
  };
  el.addEventListener("scroll", mark, { passive: true });
  // Absent where there is no layout (tests): there is nothing to measure there.
  const observer = typeof ResizeObserver === "undefined" ? undefined : new ResizeObserver(mark);
  observer?.observe(el);
  if (el.firstElementChild) observer?.observe(el.firstElementChild);
  onCleanup(() => {
    observer?.disconnect();
    el.removeEventListener("scroll", mark);
  });
}
