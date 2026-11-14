// The fixture's own console check: the wording WebKit gives a fetch that leaving a page cut off
// passes only while a page is being left. A navigation answered with no content keeps the page,
// so the same error after it still fails the test, whatever navigation comes next.
import { test } from "./fixtures";

test.fail(
  "a fetch error after a navigation that kept the page fails its test",
  async ({ page }) => {
    await page.route("**/regtest/no-content", (route) => route.fulfill({ status: 204 }));
    await page.goto("/regtest/");
    const asked = page.waitForRequest("**/regtest/no-content");
    await page.evaluate(() => {
      location.href = "/regtest/no-content";
    });
    await asked;
    await page.evaluate(() => {
      // oxlint-disable-next-line no-console -- the console is what is under test
      console.error(`Fetch API cannot load ${location.origin}/x due to access control checks.`);
    });
    // A later navigation that does start a document leaves that error counted.
    await page.goto("/regtest/");
  },
);
