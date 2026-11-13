// Theme: follows the OS unless forced. public/boot.js applies a stored choice before first paint.
import { createSignal } from "solid-js";

export type Scheme = "light" | "dark";

const KEY = "theme";
const root = () => document.documentElement;
const osQuery = () => matchMedia("(prefers-color-scheme: dark)");
const osScheme = (): Scheme => (osQuery().matches ? "dark" : "light");

function forced(): Scheme | null {
  const t = root().dataset["theme"];
  return t === "light" || t === "dark" ? t : null;
}

// boot.js has applied any stored choice before this module runs.
const [scheme, setScheme] = createSignal<Scheme>(forced() ?? osScheme());

/**
 * The browser's own chrome follows the scheme on screen, not only the OS: Safari tinted its
 * toolbar with the OS's colour over a page toggled the other way. One theme colour, the page's
 * own background, and a colour scheme that is the one shown.
 */
export function applyChrome(shown: Scheme = scheme()): void {
  const head = document.head;
  for (const m of head.querySelectorAll('meta[name="theme-color"]')) m.remove();
  const meta = document.createElement("meta");
  meta.name = "theme-color";
  meta.content = getComputedStyle(root()).getPropertyValue("--color-bg").trim() || "#f4f6f7";
  head.append(meta);
  head.querySelector('meta[name="color-scheme"]')?.setAttribute("content", shown);
}

/** The scheme on screen now. It follows the OS while {@link watchTheme} runs. */
export { scheme };

/** Starts tracking the OS setting. Returns a function that stops. */
export function watchTheme(): () => void {
  // A write is seen at the next flush: the new scheme is handed on, not read back.
  const sync = () => {
    const shown = forced() ?? osScheme();
    setScheme(shown);
    applyChrome(shown);
  };
  applyChrome();
  const q = osQuery();
  q.addEventListener("change", sync);
  return () => q.removeEventListener("change", sync);
}

/**
 * Switches to the other scheme. Choosing what the OS already shows clears the forced choice, so
 * the page follows the OS again from then on.
 */
export function toggleTheme(): void {
  const next: Scheme = scheme() === "dark" ? "light" : "dark";
  const follow = next === osScheme();
  if (follow) delete root().dataset["theme"];
  else root().dataset["theme"] = next;
  try {
    if (follow) localStorage.removeItem(KEY);
    else localStorage.setItem(KEY, next);
  } catch {
    // Storage blocked (private mode): the choice still applies to this page.
  }
  setScheme(next);
  applyChrome(next);
}
