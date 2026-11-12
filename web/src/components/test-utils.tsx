// Mounts components for tests, inside a router so links and network hooks work.
import { createMemoryHistory, MemoryRouter, Route } from "@solidjs/router";
import type { JSX } from "solid-js";
import { render } from "solid-js/web";
import { afterEach } from "vitest";

const mounted: (() => void)[] = [];
afterEach(() => {
  for (const dispose of mounted.splice(0)) dispose();
  document.body.replaceChildren();
});

/** Renders `ui` at `url` and returns its container. */
export function mount(ui: () => JSX.Element, url = "/"): HTMLElement {
  const history = createMemoryHistory();
  history.set({ value: url });
  const el = document.createElement("div");
  document.body.append(el);
  mounted.push(
    render(
      () => (
        <MemoryRouter history={history}>
          <Route path="*" component={ui} />
        </MemoryRouter>
      ),
      el,
    ),
  );
  return el;
}
