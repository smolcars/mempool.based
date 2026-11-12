import { chain, expect, test } from "./fixtures";

test("the app shell loads on /regtest/ with the regtest network shown", async ({ page }) => {
  await page.goto("/regtest/");
  await expect(page.locator("html")).toHaveAttribute("data-network", "regtest");
  await expect(page.getByRole("banner")).toContainText("Regtest");
  await expect(page).toHaveTitle(/^Regtest: .* \| rbitcoin\.space$/);
});

test("a deep link survives a reload", async ({ page }) => {
  const hash = chain.blocks[106];
  await page.goto(`/regtest/block/${hash}`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Block 106");
  await expect(page.getByText(hash)).toBeVisible();
  await page.reload();
  await expect(page).toHaveURL(`/regtest/block/${hash}`);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Block 106");
  await expect(page.getByText(hash)).toBeVisible();
});

test("an unknown page shows the not-found page inside the shell", async ({ page }) => {
  const res = await page.goto("/regtest/no/such/page");
  // The server sends the app shell for any page path; the app decides what is there.
  expect(res?.status()).toBe(200);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Page not found");
  await expect(page.getByRole("banner")).toContainText("Regtest");
});

test("digit groups show the same visible gap in every engine", async ({ page }) => {
  await page.goto("/regtest/no/such/page");
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Page not found");
  // The gap is the narrow no-break space in our own font, 0.15 em: before, each engine drew another
  // font's, and WebKit's (0.05 em) let "1 293" read as "1293".
  const gap = await page.evaluate(async () => {
    await document.fonts.ready;
    const width = (text: string) => {
      const span = document.createElement("span");
      span.className = "num";
      span.textContent = text;
      document.querySelector("main")?.append(span);
      const w = span.getBoundingClientRect().width;
      span.remove();
      return w;
    };
    const em = Number.parseFloat(getComputedStyle(document.body).fontSize);
    return (width("1\u202F293") - width("1293")) / em;
  });
  expect(gap).toBeCloseTo(0.15, 2);
});
