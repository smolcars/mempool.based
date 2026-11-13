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
  // In prose, Chromium and WebKit take the space from our face; Firefox takes a space from the font
  // of the digits beside it (the system font's narrow space, 0.10 em), still a visible gap.
  expect(gaps[0]).toBeGreaterThanOrEqual(0.095);
  expect(gaps[0]).toBeLessThanOrEqual(0.155);
  expect(gaps[1]).toBeCloseTo(0.15, 2);
});

test("the not-found page offers the search and ways on", async ({ page }) => {
  await page.goto("/regtest/no/such/page");
  const main = page.getByRole("main");
  await expect(main.getByRole("searchbox")).toBeVisible();
  await expect(main.getByRole("navigation", { name: "Where to go" }).getByRole("link")).toHaveCount(
    5,
  );
});

/** Whether two boxes overlap by more than a hairline. */
const overlaps = (
  a: { x: number; y: number; width: number; height: number },
  b: { x: number; y: number; width: number; height: number },
) =>
  a.x + 1 < b.x + b.width &&
  b.x + 1 < a.x + a.width &&
  a.y + 1 < b.y + b.height &&
  b.y + 1 < a.y + a.height;

// A desktop at 200% zoom lays the page out at half its width in CSS pixels: the header must
// switch to its two-row form before the search and the sections collide.
for (const [name, viewport] of [
  ["a desktop at 200% zoom", { width: 720, height: 450 }],
  ["a phone", { width: 390, height: 844 }],
  ["a small phone", { width: 320, height: 640 }],
] as const) {
  test(`the header keeps search, network and sections apart on ${name}`, async ({ page }) => {
    await page.setViewportSize(viewport);
    await page.goto("/regtest/blocks");
    const header = page.getByRole("banner");
    // The search's box, which holds the field and its hints; the network switch is a plain label
    // when the server serves one network, as here.
    const search = await header.locator(".search__field").boundingBox();
    const network = await header.locator(".network__button").boundingBox();
    const theme = await header
      .getByRole("button", { name: /Switch to (light|dark) theme/ })
      .boundingBox();
    const links = await header.getByRole("navigation", { name: "Main" }).getByRole("link").all();
    expect(links).toHaveLength(4);
    expect(search && search.width).toBeGreaterThan(90);
    // The network and theme are a thumb's target too, however small they are drawn.
    expect(network && network.height).toBeGreaterThanOrEqual(43.5);
    expect(theme && Math.min(theme.width, theme.height)).toBeGreaterThanOrEqual(43.5);
    for (const link of links) {
      const box = await link.boundingBox();
      if (!box || !search || !network) throw new Error("missing header part");
      expect(overlaps(box, search), "a section over the search").toBe(false);
      expect(overlaps(box, network), "a section over the network").toBe(false);
      // In sight without scrolling the row sideways, and a thumb's target.
      expect(box.x + box.width, "a section past the edge").toBeLessThanOrEqual(viewport.width);
      expect(box.height).toBeGreaterThanOrEqual(43.5);
    }
    // About 100 px at most on a phone, so the page starts in the first screen.
    const height = (await header.boundingBox())?.height ?? 0;
    expect(height).toBeLessThanOrEqual(viewport.width < 700 ? 101 : 110);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(
      viewport.width,
    );
    // On a phone, searching takes the row: the field and its answer are nearly the page's width.
    if (viewport.width < 700) {
      await header.getByRole("searchbox").fill("nonsense query");
      await header.getByRole("searchbox").press("Enter");
      const panel = page.getByText("Nothing matches", { exact: true });
      await expect(panel).toBeVisible();
      const field = await header.locator(".search__field").boundingBox();
      expect(field && field.width).toBeGreaterThan(viewport.width * 0.85);
    }
  });
}

// The current section was once unmarked once a period was chosen or a child page opened.
test("the main nav marks the current section on every page under it, whatever the query", async ({
  page,
}) => {
  const routes: [string, string][] = [
    ["/regtest/blocks", "Blocks"],
    [`/regtest/block/${chain.blocks[106]}`, "Blocks"],
    ["/regtest/block/next", "Blocks"],
    ["/regtest/mempool", "Mempool"],
    ["/regtest/mempool?interval=2h", "Mempool"],
    ["/regtest/mempool?interval=all", "Mempool"],
    ["/regtest/mining", "Mining"],
    ["/regtest/mining?period=24h", "Mining"],
    ["/regtest/mining?period=all", "Mining"],
    ["/regtest/mining/pool/unknown", "Mining"],
    ["/regtest/lightning", "Lightning"],
    ["/regtest/lightning/rankings", "Lightning"],
  ];
  const nav = page.getByRole("navigation", { name: "Main" });
  const marked = async () =>
    nav
      .locator("a")
      .evaluateAll((all) =>
        all
          .filter(
            (a) =>
              a.getAttribute("aria-current") === "true" &&
              getComputedStyle(a, "::after").backgroundColor !== "rgba(0, 0, 0, 0)",
          )
          .map((a) => a.textContent),
      );
  for (const [path, section] of routes) {
    await page.goto(path);
    await expect.poll(marked, { message: path }).toStrictEqual([section]);
  }
  // In-page: a period chosen keeps the section marked.
  await page.goto("/regtest/mining");
  await page
    .getByRole("navigation", { name: "Period" })
    .getByRole("link", { name: /all time/ })
    .click();
  await expect(page).toHaveURL(/period=all$/);
  await expect.poll(marked).toStrictEqual(["Mining"]);
});

// Safari zoomed the page into the Lightning search (its text under 16 px) and stayed zoomed; and a
// tapped control kept its hover look until something else was tapped.
test("on a touch screen no field is small enough to zoom into, and a tap leaves no hover behind", async ({
  browser,
  baseURL,
}) => {
  const touch = await browser.newContext({
    hasTouch: true,
    isMobile: true,
    viewport: { width: 390, height: 844 },
  });
  const page = await touch.newPage();
  for (const path of ["/regtest/", "/regtest/lightning", "/regtest/no/such/page"]) {
    await page.goto(`${baseURL ?? ""}${path}`);
    await expect(page.locator("input").first()).toBeAttached();
    const sizes = await page
      .locator("input")
      .evaluateAll((all) => all.map((i) => Number.parseFloat(getComputedStyle(i).fontSize)));
    for (const size of sizes) expect(size, path).toBeGreaterThanOrEqual(16);
  }
  // Hover styles are for pointers that hover: a button tapped keeps its resting look.
  expect(await page.evaluate(() => matchMedia("(hover: hover)").matches)).toBe(false);
  await page.goto(`${baseURL ?? ""}/regtest/mining`);
  const button = page.locator("main .btn").first();
  await expect(button).toBeVisible();
  const rest = await button.evaluate((b) => getComputedStyle(b).backgroundColor);
  await button.tap();
  expect(await button.evaluate((b) => getComputedStyle(b).backgroundColor)).toBe(rest);
  await touch.close();
});

// Safari kept its toolbar in the system's colour over a page toggled the other way.
test("the browser's chrome takes the colour of the scheme on screen, not only the system's", async ({
  page,
}) => {
  await page.emulateMedia({ colorScheme: "dark" });
  await page.goto("/regtest/");
  const chrome = () =>
    page.evaluate(() => ({
      theme: [...document.querySelectorAll('meta[name="theme-color"]')].map((m) =>
        m.getAttribute("content"),
      ),
      scheme: document.querySelector('meta[name="color-scheme"]')?.getAttribute("content"),
    }));
  await expect.poll(chrome).toStrictEqual({ theme: ["#121518"], scheme: "dark" });
  await page.getByRole("button", { name: "Switch to light theme" }).click();
  await expect.poll(chrome).toStrictEqual({ theme: ["#f4f6f7"], scheme: "light" });
});

// At 150% page zoom a phone is about 268 px wide: the search shrank to "Se" and the sections touched.
test("the header keeps a usable search and apart sections down to a zoomed phone's width", async ({
  page,
}) => {
  for (const width of [320, 268]) {
    await page.setViewportSize({ width, height: 800 });
    await page.goto("/regtest/");
    const search = page.getByRole("searchbox", { name: "Search" });
    const box = await search.boundingBox();
    expect(box?.width ?? 0, `search at ${width} px`).toBeGreaterThanOrEqual(
      width >= 300 ? 90 : 200,
    );
    // The placeholder whole: its text fits the field.
    const fits = await search.evaluate((input: HTMLInputElement) => {
      const probe = document.createElement("span");
      const style = getComputedStyle(input);
      probe.style.font = style.font;
      probe.textContent = input.placeholder;
      document.body.append(probe);
      const w = probe.getBoundingClientRect().width;
      probe.remove();
      return (
        w <=
        input.clientWidth -
          Number.parseFloat(style.paddingLeft) -
          Number.parseFloat(style.paddingRight) +
          1
      );
    });
    expect(fits, `placeholder at ${width} px`).toBe(true);
    const links = await page
      .getByRole("navigation", { name: "Main" })
      .getByRole("link")
      .evaluateAll((all) => all.map((a) => a.getBoundingClientRect()));
    for (let i = 1; i < links.length; i++) {
      expect((links[i]?.left ?? 0) >= (links[i - 1]?.right ?? 0), `sections at ${width} px`).toBe(
        true,
      );
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(
      width,
    );
  }
});
