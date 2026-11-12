// The fixture's own CSP check, in every engine: a style the policy refuses fails the test, even in
// a page with a Playwright screenshot, whose own refused style the fixture sets aside in WebKit.
import { expect, test } from "./fixtures";

test.fail("a style element the policy refuses fails its test", async ({ page }) => {
  await page.route("**/regtest/csp-probe", (route) =>
    route.fulfill({
      contentType: "text/html",
      headers: { "content-security-policy": "default-src 'self'" },
      body: "<!doctype html><style>body { color: red }</style><p>probe</p>",
    }),
  );
  await page.goto("/regtest/csp-probe");
  await expect(page.getByText("probe")).toBeVisible();
  await page.screenshot();
});
