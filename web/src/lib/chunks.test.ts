import { describe, expect, it } from "vitest";
import { isChunkError, retryStyles, watchStyles } from "./chunks";

watchStyles();

/** A stylesheet link that has failed to load, as the browser reports it. */
function failedLink(name: string): HTMLLinkElement {
  const link = document.createElement("link");
  link.rel = "stylesheet";
  link.dataset["name"] = name;
  document.head.append(link);
  link.dispatchEvent(new Event("error"));
  return link;
}
const current = (name: string) =>
  document.head.querySelector<HTMLLinkElement>(`link[data-name="${name}"]`);

describe("retryStyles", () => {
  it("asks again for each failed stylesheet and waits for all of them", async () => {
    failedLink("a");
    failedLink("b");
    let done = false;
    const retried = retryStyles().then(() => {
      done = true;
    });
    current("a")?.dispatchEvent(new Event("load"));
    await Promise.resolve();
    expect(done).toBe(false);
    current("b")?.dispatchEvent(new Event("load"));
    await retried;
    expect(done).toBe(true);
  });

  it("does not answer while one is still loading after another failed, and keeps both", async () => {
    failedLink("c");
    failedLink("d");
    let settled = false;
    const retried = retryStyles().then(
      () => {
        settled = true;
      },
      () => {
        settled = true;
      },
    );
    current("c")?.dispatchEvent(new Event("error"));
    await new Promise((resolve) => {
      setTimeout(resolve, 0);
    });
    expect(settled).toBe(false);
    current("d")?.dispatchEvent(new Event("error"));
    await retried;
    // Both are noted again, so the next attempt asks for both.
    const again = retryStyles();
    current("c")?.dispatchEvent(new Event("load"));
    current("d")?.dispatchEvent(new Event("load"));
    await expect(again).resolves.toBeUndefined();
  });
});

describe("isChunkError", () => {
  it("knows each browser's words for a failed download, and nothing else", () => {
    expect(isChunkError(new TypeError("Failed to fetch dynamically imported module: /a.js"))).toBe(
      true,
    );
    expect(isChunkError(new Error("Unable to preload CSS for /a.css"))).toBe(true);
    expect(isChunkError(new TypeError("Importing a module script failed."))).toBe(true);
    expect(isChunkError(new TypeError("x is undefined"))).toBe(false);
  });
});
