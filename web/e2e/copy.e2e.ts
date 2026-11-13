// The copy button where the browser has no async clipboard: plain HTTP at a name that is not
// localhost, as a self-hosted explorer on a home network is served. The tests' own server is on
// localhost, which browsers treat as secure, so the page is opened at another name and Playwright
// carries its requests to the server.
import type { Page } from "@playwright/test";
import { chain, expect, test } from "./fixtures";

// One clipboard for the whole browser: these two must not copy at once.
test.describe.configure({ mode: "serial" });

const INSECURE = "http://explorer.test";

/** Serves the site at INSECURE. Its live socket is left silent: these tests do not need it. */
async function openInsecure(page: Page, baseURL: string, path: string) {
  await page.routeWebSocket(/\/app\/ws$/, (ws) => {
    // Accepted and never answered: the page waits for a greeting that does not come.
    ws.onMessage(() => undefined);
  });
  await page.route(`${INSECURE}/**`, async (route) => {
    const url = new URL(route.request().url());
    await route.fulfill({
      response: await route.fetch({ url: baseURL + url.pathname + url.search }),
    });
  });
  await page.goto(INSECURE + path);
  expect(
    await page.evaluate(() => window.isSecureContext),
    "the page is not a secure context",
  ).toBe(false);
}

/**
 * What the clipboard holds, read from a secure page of the site: through the async clipboard, or
 * else by pasting into a field. Null where the engine allows neither; the read says which worked.
 */
async function clipboardText(page: Page, baseURL: string) {
  const reader = await page.context().newPage();
  try {
    // A page of the site's own: WebKit shows a JSON reply in a viewer the CSP refuses.
    await reader.goto(`${baseURL}/regtest/no/such/page`);
    // Firefox asks the reader before it lets a page read, so the wait is bounded.
    const read = await reader.evaluate(() =>
      Promise.race([
        navigator.clipboard.readText().catch(() => null),
        new Promise<null>((resolve) => {
          setTimeout(() => resolve(null), 2000);
        }),
      ]),
    );
    if (read) return { text: read, how: "readText" };
    const field = await reader.evaluateHandle(() => {
      const area = document.createElement("textarea");
      document.body.append(area);
      area.focus();
      return area;
    });
    await reader.keyboard.press("ControlOrMeta+V");
    const pasted = await field.evaluate((area) => area.value);
    return pasted ? { text: pasted, how: "paste" } : { text: null, how: "neither" };
  } finally {
    await reader.close();
  }
}

for (const [what, path, label, value] of [
  ["a block hash", `/regtest/block/${chain.blocks[106]}`, "Copy block hash", chain.blocks[106]],
  ["a txid", `/regtest/tx/${chain.txs.payment}`, "Copy transaction ID", chain.txs.payment],
] as const) {
  test(`over plain HTTP, the copy button copies ${what}`, async ({
    page,
    context,
    baseURL,
    browserName,
  }) => {
    if (browserName === "chromium") await context.grantPermissions(["clipboard-read"]);
    await openInsecure(page, baseURL ?? "", path);
    // The page subject's button: found by place, since its name changes once pressed.
    const button = page.getByRole("main").locator("button.copy").first();
    await expect(button).toHaveAccessibleName(label);
    await button.click();
    await expect(button).toHaveAccessibleName("Copied");
    // A sighted reader sees the word too, at a readable size, not only a green icon.
    const note = button.locator(".copy__note");
    await expect(note).toHaveText("Copied");
    await expect(note).toBeVisible();
    const box = await note.boundingBox();
    expect(box?.width ?? 0).toBeGreaterThan(30);
    expect(box?.height ?? 0).toBeGreaterThan(12);
    const copied = await clipboardText(page, baseURL ?? "");
    test.info().annotations.push({ type: "clipboard read back by", description: copied.how });
    // Chromium lets a test read the clipboard; where an engine will not, "Copied" is the proof.
    if (browserName === "chromium" || copied.text !== null) expect(copied.text).toBe(value);
  });
}
