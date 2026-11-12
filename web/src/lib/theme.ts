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

const [scheme, setScheme] = createSignal<Scheme>("light");

/** The scheme on screen now. Call {@link watchTheme} once before reading it. */
export { scheme };

/** Starts tracking the OS setting. Returns a function that stops. */
export function watchTheme(): () => void {
  const sync = () => setScheme(forced() ?? osScheme());
  sync();
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
}
