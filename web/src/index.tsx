import { Router } from "@solidjs/router";
import { render } from "solid-js/web";
import "../../design/tokens.css";
import "./styles/global.css";
import { routes } from "./routes";
import { Shell } from "./shell/Shell";

// As long as a browser's own block period for web fonts; only a slower font can still reflow text.
const FONT_WAIT_MS = 3000;

const root = document.getElementById("root");
if (root) {
  // Wait for the preloaded font (fetched alongside this script), so text is laid out once in it.
  const font = document.fonts?.load("1em Recursive") ?? Promise.resolve();
  const timeout = new Promise((resolve) => setTimeout(resolve, FONT_WAIT_MS));
  void Promise.race([font, timeout])
    .catch(() => undefined)
    .then(() => render(() => <Router root={Shell}>{routes}</Router>, root));
}
