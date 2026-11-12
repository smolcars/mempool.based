import {
  createMemoryHistory,
  MemoryRouter,
  Route,
  useLocation,
  useNavigate,
} from "@solidjs/router";
import { type Component, lazy, Suspense } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import { clearApiCache } from "../lib/api";
import { SearchField } from "./SearchField";
import { mount } from "./test-utils";

const TXID = "65428488a6202f1d681d053b3ff5d8ea9f5b0e5e1e0dcf7bf2e4c1e5a3f9c2e1";

/** The field, and the path the router is on. */
function setup(url = "/") {
  const el = mount(() => {
    const location = useLocation();
    return (
      <>
        <SearchField />
        <output data-testid="path">{location.pathname}</output>
      </>
    );
  }, url);
  const input = el.querySelector("input");
  if (!input) throw new Error("no input");
  const path = () => el.querySelector("output")?.textContent;
  const type = (value: string) => {
    input.focus();
    input.value = value;
    input.dispatchEvent(new InputEvent("input", { bubbles: true }));
  };
  const enter = () => input.form?.requestSubmit();
  return { el, input, path, type, enter };
}

const reply = (status: number, body: unknown) => Response.json(body, { status });

afterEach(() => {
  vi.unstubAllGlobals();
  clearApiCache();
});

describe("SearchField", () => {
  it("is a labelled search landmark with a shortcut", () => {
    const { el, input } = setup();
    expect(el.querySelector("search")).not.toBeNull();
    expect(el.querySelector(`label[for="${input.id}"]`)?.textContent).toBe("Search");
    expect(input.getAttribute("aria-keyshortcuts")).toBe("/");
    expect(input.maxLength).toBe(512);
  });

  it("reads the query as it is typed, and says when Enter will act", async () => {
    const { el, type } = setup();
    type("840000");
    const reading = () => el.querySelector(".search__reading");
    await vi.waitFor(() => expect(reading()?.textContent).toContain("Block 840\u{202F}000"));
    expect(reading()?.querySelector("kbd")).not.toBeNull();
    type(`${TXID.slice(0, 10)}abc`);
    await vi.waitFor(() => expect(reading()?.textContent).toBe("13 of 64 characters"));
    expect(reading()?.querySelector("kbd")).toBeNull();
  });

  it("goes straight to a height's block page on its network, then empties the field", async () => {
    const fetch = vi.fn<typeof globalThis.fetch>();
    vi.stubGlobal("fetch", fetch);
    const { input, path, type, enter } = setup("/signet/");
    type(" #975,411 ");
    enter();
    await vi.waitFor(() => expect(path()).toBe("/signet/block/975411"));
    expect(input.value).toBe("");
    expect(fetch).not.toHaveBeenCalled();
  });

  it("looks a 64-hex id up and opens what it is", async () => {
    vi.stubGlobal("fetch", (url: string) =>
      Promise.resolve(url.endsWith("/status") ? reply(200, { confirmed: true }) : reply(404, {})),
    );
    const { path, type, enter } = setup();
    type(`https://mempool.space/tx/${TXID}#vout=1`);
    enter();
    await vi.waitFor(() => expect(path()).toBe(`/tx/${TXID}`));
  });

  it("shows what was searched and what search finds when nothing matches", async () => {
    vi.stubGlobal("fetch", () => Promise.resolve(reply(404, { error: "not found" })));
    const { el, path, type, enter } = setup("/testnet4/");
    type(TXID);
    enter();
    const panel = () => el.querySelector(".search__panel");
    await vi.waitFor(() => expect(panel()?.textContent).toContain("Nothing matches"));
    expect(panel()?.textContent).toContain("No block or transaction on Testnet4 has the ID");
    expect(panel()?.querySelector("[title]")?.getAttribute("title")).toBe(TXID);
    expect(panel()?.querySelector("dl")?.textContent).toContain("the whole address");
    expect(el.querySelector('[role="status"]')?.textContent).toContain("Nothing matches");
    expect(path()).toBe("/testnet4/");
    // The field itself says the ID was understood and nothing has it, until the query changes.
    const reading = () => el.querySelector(".search__reading")?.textContent;
    expect(reading()).toBe("Nothing has this ID");
    type(TXID.slice(1));
    await vi.waitFor(() => expect(reading()).toBe("63 of 64 characters"));
  });

  it("explains an unrecognised query as text, and closes on Escape", async () => {
    const { el, input, type, enter } = setup();
    type('<img src=x onerror="alert(1)">');
    enter();
    const panel = () => el.querySelector(".search__panel");
    await vi.waitFor(() => expect(panel()).not.toBeNull());
    expect(panel()?.querySelector("q")?.textContent).toBe('<img src=x onerror="alert(1)">');
    expect(panel()?.querySelector("img")).toBeNull();
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(panel()).toBeNull();
    expect(input.value).toBe('<img src=x onerror="alert(1)">');
  });

  it("says the node is unavailable, not that nothing matches, and retries", async () => {
    let up = false;
    vi.stubGlobal("fetch", () =>
      Promise.resolve(up ? reply(200, {}) : reply(502, { error: "upstream unavailable" })),
    );
    const { el, path, type, enter } = setup();
    type(TXID);
    enter();
    const panel = () => el.querySelector(".search__panel");
    await vi.waitFor(() => expect(panel()?.textContent).toContain("Search could not finish"));
    expect(panel()?.textContent).toContain("Upstream unavailable.");
    up = true;
    panel()?.querySelector("button")?.click();
    await vi.waitFor(() => expect(path()).toBe(`/tx/${TXID}`));
  });

  it("shows it is looking while the answer is slow, and typing cancels it", async () => {
    const signals: AbortSignal[] = [];
    vi.stubGlobal(
      "fetch",
      (_url: string, init: RequestInit) =>
        new Promise(() => {
          if (init.signal) signals.push(init.signal);
        }),
    );
    const { el, path, type, enter } = setup();
    type(TXID);
    enter();
    await vi.waitFor(() => expect(signals).toHaveLength(1));
    expect(el.querySelector(".search__busy")?.textContent).toBe("Looking up");
    type(`${TXID}0`);
    expect(el.querySelector(".search__busy")).toBeNull();
    expect(signals[0]?.aborted).toBe(true);
    expect(path()).toBe("/");
  });
});

describe("SearchField and navigation", () => {
  it("drops a search when the reader navigates elsewhere, even before that page has loaded", async () => {
    let answer: ((r: Response) => void) | undefined;
    vi.stubGlobal(
      "fetch",
      () =>
        new Promise<Response>((resolve) => {
          answer = resolve;
        }),
    );
    let release: () => void = () => undefined;
    const Slow = lazy(
      () =>
        new Promise<{ default: Component }>((resolve) => {
          release = () => resolve({ default: () => <p>slow page</p> });
        }),
    );
    let go: (path: string) => void = () => undefined;
    const history = createMemoryHistory();
    history.set({ value: "/" });
    const el = document.createElement("div");
    document.body.append(el);
    const dispose = render(
      () => (
        <MemoryRouter
          history={history}
          root={(props) => {
            const navigate = useNavigate();
            go = (path) => navigate(path);
            return (
              <>
                <SearchField />
                <Suspense>{props.children}</Suspense>
              </>
            );
          }}
        >
          <Route path="/" component={() => <p>home</p>} />
          <Route path="/slow" component={Slow} />
          <Route path="/tx/:txid" component={() => <p>tx page</p>} />
        </MemoryRouter>
      ),
      el,
    );
    const input = el.querySelector("input");
    if (!input) throw new Error("no input");
    input.value = TXID;
    input.dispatchEvent(new InputEvent("input", { bubbles: true }));
    input.form?.requestSubmit();
    await vi.waitFor(() => expect(answer).toBeDefined());
    go("/slow");
    // The search answers while the page the reader chose is still loading.
    answer?.(Response.json({ confirmed: true }));
    await new Promise((resolve) => {
      setTimeout(resolve, 20);
    });
    release();
    await vi.waitFor(() => expect(el.textContent).toContain("slow page"));
    expect(history.get()).toBe("/slow");
    dispose();
  });
});
