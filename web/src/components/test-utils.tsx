// Mounts components for tests, inside a router so links and network hooks work.
import { createRouter, memoryHistory } from "@solidjs/router";
import { type JSX, render } from "@solidjs/web";
import { afterEach } from "vitest";

const mounted: (() => void)[] = [];
afterEach(() => {
  for (const dispose of mounted.splice(0)) dispose();
  document.body.replaceChildren();
});

/** Renders `ui` at `url` and returns its container. */
export function mount(ui: () => JSX.Element, url = "/"): HTMLElement {
  const Router = createRouter({
    routes: [{ path: "*", component: ui }],
    history: memoryHistory(url),
  });
  const el = document.createElement("div");
  document.body.append(el);
  mounted.push(render(() => <Router>{(page) => page.children}</Router>, el));
  return el;
}
