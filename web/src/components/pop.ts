// The page's small pop-ups (an exact amount, the network menu): one open at a time, closed by a
// press outside, Escape, and, where asked, a scroll. Not the browser's popover: while a top-layer
// element shows, iOS Safari repaints the band behind its toolbar and the page stops above it
// (WebKit bug 302272).

let closeOpen: (() => void) | undefined;

/**
 * Notes a pop-up as open, closing the one open before it. `inside` is what a press may land on
 * without closing it: its button first, which Escape returns focus to. Returns what closes it.
 */
export function openPop(
  onClose: () => void,
  inside: () => readonly (HTMLElement | undefined)[],
  options: { scroll?: boolean } = {},
): () => void {
  closeOpen?.();
  const press = (e: PointerEvent) => {
    if (
      !inside().some((el) => el !== undefined && e.target instanceof Node && el.contains(e.target))
    )
      close();
  };
  const key = (e: KeyboardEvent) => {
    if (e.key !== "Escape") return;
    close();
    inside()[0]?.focus();
  };
  const close = () => {
    document.removeEventListener("pointerdown", press, true);
    document.removeEventListener("keydown", key);
    window.removeEventListener("scroll", close, true);
    window.removeEventListener("resize", close);
    if (closeOpen === close) closeOpen = undefined;
    onClose();
  };
  document.addEventListener("pointerdown", press, true);
  document.addEventListener("keydown", key);
  if (options.scroll === true) {
    window.addEventListener("scroll", close, { capture: true, passive: true });
    window.addEventListener("resize", close, { passive: true });
  }
  closeOpen = close;
  return close;
}
