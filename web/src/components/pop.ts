// The page's small pop-ups (an exact amount, the network menu): one open at a time, closed by a
// press outside, Escape, and, where asked, a scroll that carries its button away. Not the browser's
// popover: while a top-layer element shows, iOS Safari repaints the band behind its toolbar and the
// page stops above it (WebKit bug 302272).
//
// A press outside a menu only closes it (`absorb`): the reader is dismissing a choice, as with a
// native menu, and a tap that also followed the link beneath it took them to another page. An
// exact amount lets the press through: it is a note on a figure, like a tooltip, and the reader is
// already on to what they tapped.

let closeOpen: (() => void) | undefined;

/**
 * Notes a pop-up as open, closing the one open before it. `inside` is what a press may land on
 * without closing it: its button first, which Escape returns focus to. Returns what closes it.
 */
export function openPop(
  onClose: () => void,
  inside: () => readonly (HTMLElement | undefined)[],
  options: { scroll?: boolean; absorb?: boolean } = {},
): () => void {
  closeOpen?.();
  const press = (e: PointerEvent) => {
    if (
      inside().some((el) => el !== undefined && e.target instanceof Node && el.contains(e.target))
    )
      return;
    close();
    if (options.absorb === true) absorbClick();
  };
  const key = (e: KeyboardEvent) => {
    if (e.key !== "Escape") return;
    close();
    inside()[0]?.focus();
  };
  // A scroll or resize closes it once its button has moved: not a nudge, such as iOS Safari's
  // toolbar settling as the pop-up opens.
  const at = inside()[0]?.getBoundingClientRect();
  const moved = () => {
    const now = inside()[0]?.getBoundingClientRect();
    if (at && now && Math.max(Math.abs(now.top - at.top), Math.abs(now.left - at.left)) > 16)
      close();
  };
  const close = () => {
    document.removeEventListener("pointerdown", press, true);
    document.removeEventListener("keydown", key);
    window.removeEventListener("scroll", moved, true);
    window.removeEventListener("resize", moved);
    if (closeOpen === close) closeOpen = undefined;
    onClose();
  };
  document.addEventListener("pointerdown", press, true);
  document.addEventListener("keydown", key);
  if (options.scroll === true) {
    window.addEventListener("scroll", moved, { capture: true, passive: true });
    window.addEventListener("resize", moved, { passive: true });
  }
  closeOpen = close;
  return close;
}

/**
 * Swallows the click that ends the press under way, so it activates nothing. The next press, or a
 * press the browser cancels (a scroll), lets clicks through again.
 */
function absorbClick(): void {
  const eat = (e: Event) => {
    e.preventDefault();
    e.stopPropagation();
    done();
  };
  const done = () => {
    window.removeEventListener("click", eat, true);
    window.removeEventListener("pointerdown", done, true);
    window.removeEventListener("pointercancel", done, true);
  };
  window.addEventListener("click", eat, true);
  // Added while this press is being dispatched: the window's capture phase is already past for it.
  window.addEventListener("pointerdown", done, true);
  window.addEventListener("pointercancel", done, true);
}
