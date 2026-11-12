// Chain data is attacker-controlled text (docs/SECURITY.md rule 14). This puts markup, a
// `javascript:` URL, bidi overrides, ANSI escapes and NUL on the run's chain, in OP_RETURN outputs
// and a hand-built coinbase tag, and checks every page shows them as inert, visible text. It mines,
// so it runs after every other test (the `mines` project).
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import type { Page } from "@playwright/test";
import { chain, expect, test } from "./fixtures";

const REGTEST = fileURLToPath(new URL("../../dev/regtest/regtest.py", import.meta.url));

interface Hostile {
  tx: string;
  block: string;
  coinbase: string;
}

/** What the page did with the text: anything that would mean it was read as markup or a link. */
async function inert(page: Page, dialogs: string[]) {
  const found = await page.evaluate(() => ({
    elements: document.body.querySelectorAll("script, img, iframe, object, embed, b").length,
    hrefs: [...document.querySelectorAll("a[href]")]
      .map((a) => a.getAttribute("href") ?? "")
      .filter((h) => !/^(\/|#|https:\/\/github\.com\/)/.test(h)),
    // Characters that would hide or reorder text must reach the screen only as visible marks.
    // (Escapes in \u{} form: oxfmt unescapes \uXXXX.)
    // oxlint-disable-next-line no-control-regex -- NUL and ESC are exactly what this looks for
    raw: /[\u{0}\u{1B}\u{202A}-\u{202E}\u{2066}-\u{2069}]/u.test(document.body.textContent),
    title: document.title,
  }));
  expect(found.elements, "elements injected").toBe(0);
  expect(found.hrefs, "links with an unexpected scheme").toEqual([]);
  expect(found.raw, "hidden characters shown raw").toBe(false);
  expect(found.title).not.toMatch(/[<>]/);
  expect(dialogs, "dialogs opened").toEqual([]);
}

test("hostile chain data is shown as text on every page that shows it", async ({ page, mine }) => {
  // Mines first through the fixture, so this fails clearly anywhere but the `mines` project.
  await mine(1);
  const made = JSON.parse(
    execFileSync("python3", [REGTEST, "hostile"], { encoding: "utf8" }),
  ) as Hostile;
  const dialogs: string[] = [];
  page.on("dialog", (d) => {
    dialogs.push(d.message());
    void d.dismiss();
  });

  await page.goto(`/regtest/tx/${made.tx}`);
  const outputs = page.getByRole("list", { name: "Outputs" });
  await expect(outputs).toContainText("OP_RETURN “<img src=x onerror=alert(1)>”");
  await expect(outputs).toContainText("OP_RETURN “javascript:alert(document.domain)”");
  await expect(outputs).toContainText("OP_RETURN “</title><script>alert(1)</script>”");
  await expect(outputs).toContainText(
    String.raw`OP_RETURN “line1\u{a}line2\u{d}\u{a}\u{1b}[31mANSI\u{1b}[0m\u{0}NUL”`,
  );
  // Mostly bidi and zero-width characters: not text, so it is shown as its bytes.
  await expect(outputs).toContainText("OP_RETURN, 34 bytes");
  await page.getByRole("button", { name: "Details of output 2" }).click();
  await expect(page.getByRole("heading", { name: "Output 2" })).toBeFocused();
  await expect(page.getByText(/^e280ae6576696ce280ac/).first()).toBeVisible();
  await inert(page, dialogs);

  await page.goto(`/regtest/block/${made.block}`);
  const tag = page.getByRole("definition").filter({ hasText: "Written by the miner" });
  await expect(tag).toContainText("<script>alert('cb')</script>· <b>pool</b>");
  // The miner claimed 1 000 sats less than the subsidy of an empty block.
  await expect(
    page.getByText("0.00001000 BTC less than the subsidy and fees allowed"),
  ).toBeVisible();
  await inert(page, dialogs);

  await page.goto(`/regtest/tx/${made.coinbase}`);
  await expect(page.getByRole("list", { name: "Inputs" })).toContainText("Coinbase");
  await page.getByRole("button", { name: "Details of input 0" }).click();
  await expect(page.getByRole("heading", { name: "Input 0" })).toBeFocused();
  await inert(page, dialogs);

  await page.goto(`/regtest/address/${chain.addresses.p2wpkh}`);
  await expect(
    page
      .getByRole("table", { name: "Transactions of this address, newest first" })
      .getByRole("link", {
        name: made.tx.slice(0, 8),
      }),
  ).toBeVisible();
  await inert(page, dialogs);
});
