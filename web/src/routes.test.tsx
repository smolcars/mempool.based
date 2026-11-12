import { createMemoryHistory, MemoryRouter } from "@solidjs/router";
import { render } from "solid-js/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import { routes } from "./routes";
import { Shell } from "./shell/Shell";

afterEach(() => {
  vi.unstubAllGlobals();
  document.body.replaceChildren();
});

async function open(url: string, networks = ["mainnet", "signet", "testnet4"]) {
  vi.stubGlobal("fetch", async () => Response.json({ networks, version: "0.1.0" }));
  const history = createMemoryHistory();
  history.set({ value: url });
  const el = document.createElement("div");
  document.body.append(el);
  render(
    () => (
      <MemoryRouter history={history} root={Shell}>
        {routes}
      </MemoryRouter>
    ),
    el,
  );
  await vi.waitFor(() => expect(el.querySelector("h1")).not.toBeNull());
  return { el, history };
}

describe("routes", () => {
  it.each([
    ["/", "Blocks and mempool"],
    ["/block/840000", "Block 840\u202f000"],
    ["/blocks", "Blocks"],
    ["/signet/blocks/5", "Blocks"],
    ["/tx/ab", "Transaction"],
    ["/address/bc1q", "Address"],
    ["/mining", "Mining"],
    ["/lightning", "Lightning"],
    ["/nope", "Page not found"],
    ["/signet", "Blocks and mempool"],
    ["/signet/", "Blocks and mempool"],
    ["/testnet4/tx/ab", "Transaction"],
    ["/signet/nope/deeper", "Page not found"],
  ])("%s shows %s", async (url, heading) => {
    const { el } = await open(url);
    expect(el.querySelector("h1")?.textContent).toBe(heading);
  });

  it("marks test networks and keeps links on them", async () => {
    const { el } = await open("/testnet4/tx/ab");
    expect(document.documentElement.dataset.network).toBe("testnet4");
    expect(el.querySelector(".network-band")?.textContent).toBe(
      "Testnet4 is a test network. Coins here have no value.",
    );
    expect(el.querySelector(".brand")?.getAttribute("href")).toBe("/testnet4");
    expect(el.querySelector('nav[aria-label="Main"] a:nth-child(2)')?.getAttribute("href")).toBe(
      "/testnet4/mining",
    );
    expect(document.title).toBe("Testnet4: Transaction | mempool.based");
  });

  it("offers the same page on every enabled network", async () => {
    const { el } = await open("/signet/block/5?x=1");
    await vi.waitFor(() => expect(el.querySelectorAll(".network__option")).toHaveLength(3));
    const hrefs = [...el.querySelectorAll(".network__option")].map((a) => a.getAttribute("href"));
    expect(hrefs).toEqual(["/block/5?x=1", "/signet/block/5?x=1", "/testnet4/block/5?x=1"]);
    expect(el.querySelector(".network-band")).not.toBeNull();
  });

  it("moves off a network the server does not serve", async () => {
    const { history } = await open("/tx/ab", ["signet"]);
    await vi.waitFor(() => expect(history.get()).toBe("/signet/tx/ab"));
  });
});
