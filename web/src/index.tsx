import { Router } from "@solidjs/router";
import { render } from "solid-js/web";
import "../../design/tokens.css";
import "./styles/global.css";
import { routes } from "./routes";
import { Shell } from "./shell/Shell";

const FONT_WAIT_MS = 400;

const root = document.getElementById("root");
if (root) {
  // Give the preloaded font a moment, so text is laid out once in it and never reflows on swap.
  const font = document.fonts?.load("1em Recursive") ?? Promise.resolve();
  const timeout = new Promise((resolve) => setTimeout(resolve, FONT_WAIT_MS));
  void Promise.race([font, timeout])
    .catch(() => undefined)
    .then(() => render(() => <Router root={Shell}>{routes}</Router>, root));
}
