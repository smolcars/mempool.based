// Mines on the run's node, so it runs after every other test (the `mines` project). Named to run
// after the other mining files: home.mine.e2e.ts counts on what address.mine.e2e.ts leaves waiting.
import { expect, test } from "./fixtures";

// A new block's facts are computed on first ask; the page retries a busy answer quietly.
test.use({ consoleErrorsAllowed: [/status of (429|503)/] });

test("a block the server watched arrive shows its audit, and its match rate in the list", async ({
  page,
  mine,
}) => {
  const [hash] = await mine(1);
  await page.goto(`/regtest/block/${hash}`);
  const audit = page.getByRole("region", { name: "Audit" });
  await expect(audit.getByText("Match rate", { exact: true })).toBeVisible();
  await expect(audit.getByText(/^\d+(\.\d\d)?%$/)).toBeVisible();
  await expect(
    audit.getByRole("table", { name: "The block we expected and the block mined" }),
  ).toBeVisible();
  await expect(audit.getByText("Not audited", { exact: false })).toHaveCount(0);

  await page.goto("/regtest/blocks");
  const table = page.getByRole("table", { name: "Recent blocks, newest first" });
  await expect(table.getByRole("columnheader", { name: "Match rate, %" })).toBeVisible();
  const first = table.getByRole("row").nth(1);
  await expect(first.getByRole("link").first()).toHaveAttribute("href", `/regtest/block/${hash}`);
  await expect(first.getByRole("cell", { name: /^\d+(\.\d\d)? %$/ })).toBeVisible();
});
