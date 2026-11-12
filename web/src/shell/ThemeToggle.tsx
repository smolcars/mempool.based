import { onSettled } from "solid-js";
import { scheme, toggleTheme, watchTheme } from "../lib/theme";

/** Switches between light and dark. Picking what the OS shows goes back to following the OS. */
export function ThemeToggle() {
  onSettled(watchTheme);
  return (
    <button
      class="icon-btn theme-toggle"
      type="button"
      onClick={toggleTheme}
      aria-label={scheme() === "dark" ? "Switch to light theme" : "Switch to dark theme"}
      title={scheme() === "dark" ? "Switch to light theme" : "Switch to dark theme"}
    >
      <svg viewBox="0 0 20 20" aria-hidden="true">
        <circle cx="10" cy="10" r="6.5" />
        <path class="theme-toggle__half" d="M10 3.5a6.5 6.5 0 0 1 0 13z" />
      </svg>
    </button>
  );
}
