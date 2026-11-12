import { Router } from "@solidjs/router";
import { render } from "solid-js/web";
import "../../design/tokens.css";
import "./styles/global.css";
import { routes } from "./routes";
import { Shell } from "./shell/Shell";

const FONT_WAIT_MS = 1000;

/**
 * Takes the web font out of the token font stacks for this page load, so a font that arrives
 * late never swaps in and reflows text. The next load finds it in the cache.
 */
function useFallbackFonts() {
  const root = document.documentElement;
  const tokens = getComputedStyle(root);
  for (const name of ["--font-sans", "--font-mono"]) {
    const stack = tokens.getPropertyValue(name).replace(/["']?Recursive["']?\s*,\s*/, "");
    root.style.setProperty(name, stack);
  }
}

const root = document.getElementById("root");
if (root) {
  // Lay text out once: in the preloaded font if it is here in time, otherwise in the fallbacks for good.
  const font = document.fonts.load("1em Recursive").catch(() => []);
  const timeout = new Promise<[]>((resolve) => {
    setTimeout(() => resolve([]), FONT_WAIT_MS);
  });
  void Promise.race([font, timeout]).then((faces) => {
    if (!faces.some((f) => f.status === "loaded")) useFallbackFonts();
    render(() => <Router root={Shell}>{routes}</Router>, root);
  });
}
