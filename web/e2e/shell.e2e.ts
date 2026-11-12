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
  // The gap is the narrow no-break space in our own font, 0.15 em, in the system font's prose and in
  // the monospace alike: before, each engine drew another font's, and WebKit's (0.05 em) let "1 293"
  // read as "1293". Measured in the stacks by name (a page whose font came late leaves it out for
  // that load) and at 100 px, as Chromium on Linux rounds each advance to a whole pixel.
  const gaps = await page.evaluate(async () => {
    await document.fonts.load('100px "Recursive Gap"', "1\u202F293");
    await document.fonts.load('100px "Recursive Mono"', "1\u202F293");
    const width = (family: string, text: string) => {
      const span = document.createElement("span");
      span.className = "num";
      span.style.fontSize = "100px";
      span.style.fontFamily = family;
      span.textContent = text;
      document.querySelector("main")?.append(span);
      const w = span.getBoundingClientRect().width;
      span.remove();
      return w;
    };
    const gap = (family: string) => (width(family, "1\u202F293") - width(family, "1293")) / 100;
    return [gap('"Recursive Gap", system-ui, sans-serif'), gap('"Recursive Mono", monospace')];
  });
  expect(gaps[0]).toBeCloseTo(0.15, 2);
  expect(gaps[1]).toBeCloseTo(0.15, 2);
});
