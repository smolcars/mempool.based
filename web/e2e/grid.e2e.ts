// The block page's transaction map. Headless Chromium draws WebGL2 in software (SwiftShader), so
// these prove the squares are drawn, placed and hit right, not how fast a real GPU draws them.
import type { Page } from "@playwright/test";
import { chain, expect, test } from "./fixtures";

test.use({ consoleErrorsAllowed: [/status of (429|503)/] });

const BLOCK = `/regtest/block/${chain.blocks[106]}`;

/** The squares on screen, found in a screenshot of the canvas: each one's centre in page pixels. */
async function squares(page: Page): Promise<{ x: number; y: number }[]> {
  const canvas = page.getByRole("img", { name: /^Map of the block's/ });
  // The screenshot scrolls the canvas into view, so its box is read after. At the device's own
  // scale: scaled down to CSS pixels, the gaps between small squares blur away.
  const png = (await canvas.screenshot()).toString("base64");
  const box = await canvas.boundingBox();
  if (!box) throw new Error("the map has no box");
  const found = await page.evaluate(async (data) => {
    // Decoded in the page: a Blob made from bytes loads nothing, so the CSP is not involved.
    const bytes = Uint8Array.from(atob(data), (c) => c.codePointAt(0) ?? 0);
    const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
    const ctx = new OffscreenCanvas(bitmap.width, bitmap.height).getContext("2d");
    if (!ctx) return { out: [], width: 1 };
    ctx.drawImage(bitmap, 0, 0);
    const { data: px, width, height } = ctx.getImageData(0, 0, bitmap.width, bitmap.height);
    // The frame is empty at its top left in a nearly empty block: that is the background.
    const bg = [px[0] ?? 0, px[1] ?? 0, px[2] ?? 0];
    const ink = (i: number) =>
      Math.abs((px[i * 4] ?? 0) - (bg[0] ?? 0)) +
        Math.abs((px[i * 4 + 1] ?? 0) - (bg[1] ?? 0)) +
        Math.abs((px[i * 4 + 2] ?? 0) - (bg[2] ?? 0)) >
      40;
    const seen = new Uint8Array(width * height);
    const out: { x: number; y: number; n: number }[] = [];
    for (let start = 0; start < width * height; start++) {
      if (seen[start] === 1 || !ink(start)) continue;
      const stack = [start];
      seen[start] = 1;
      let [sx, sy, n] = [0, 0, 0];
      while (stack.length > 0) {
        const p = stack.pop() ?? 0;
        const x = p % width;
        const y = (p - x) / width;
        sx += x;
        sy += y;
        n++;
        for (const q of [x > 0 ? p - 1 : -1, x < width - 1 ? p + 1 : -1, p - width, p + width]) {
          if (q >= 0 && q < width * height && seen[q] !== 1 && ink(q)) {
            seen[q] = 1;
            stack.push(q);
          }
        }
      }
      out.push({ x: sx / n, y: sy / n, n });
    }
    return { out, width };
  }, png);
  const scale = found.width / box.width;
  return found.out
    .filter((s) => s.n > 4)
    .map((s) => ({ x: box.x + s.x / scale, y: box.y + s.y / scale }));
}

test("a block's map draws one square per transaction", async ({ page }) => {
  await page.goto(BLOCK);
  const canvas = page.getByRole("img", { name: /^Map of the block's 3 transactions/ });
  await expect(canvas).toBeVisible();
  await expect(page.locator(".block-core--grid")).toBeVisible();
  // Coinbase, script_types and consolidation, once the squares have settled.
  await expect.poll(async () => (await squares(page)).length, { timeout: 5000 }).toBe(3);
});

test("hovering a square shows its transaction, and clicking opens it", async ({
  page,
  request,
}) => {
  const tx = (await (await request.get(`/regtest/api/tx/${chain.txs.script_types}`)).json()) as {
    fee: number;
    weight: number;
  };
  await page.goto(BLOCK);
  await expect(page.locator(".block-core--grid")).toBeVisible();
  // The squares slide into place, slower on a busy machine: aim only once two looks agree.
  let found: { x: number; y: number }[] = [];
  let last = "";
  await expect
    .poll(
      async () => {
        found = await squares(page);
        const settled = JSON.stringify(found) === last;
        last = JSON.stringify(found);
        return settled ? found.length : 0;
      },
      { timeout: 5000 },
    )
    .toBe(3);
  const tip = page.locator(".txgrid__tip");
  let target: { x: number; y: number } | undefined;
  for (const s of found) {
    await page.mouse.move(s.x, s.y);
    await expect(tip).toHaveClass(/txgrid__tip--shown/);
    // The txid is looked up once the pointer rests.
    await expect(tip).not.toContainText("–");
    if (((await tip.textContent()) ?? "").includes(chain.txs.script_types.slice(0, 8))) target = s;
  }
  if (!target) throw new Error("no square is script_types");
  await page.mouse.move(target.x, target.y);
  const vsize = Math.ceil(tx.weight / 4);
  await expect(tip).toContainText((tx.fee / vsize).toFixed(2));
  await expect(tip).toContainText(`Fee ${tx.fee.toLocaleString("en").replaceAll(",", " ")} sats`);
  await expect(tip).toContainText(`Size ${vsize} vB`);
  await page.mouse.click(target.x, target.y);
  await expect(page).toHaveURL(`/regtest/tx/${chain.txs.script_types}`);
});

test("a row in the list rings its square in the map", async ({ page }) => {
  await page.goto(BLOCK);
  await expect(page.locator(".block-core--grid")).toBeVisible();
  await page.getByRole("row").filter({ hasText: "Coinbase" }).hover();
  await expect(page.locator(".txgrid__ring")).toBeVisible();
});

test("a lost WebGL context brings the layers back until it is restored", async ({ page }) => {
  await page.goto(BLOCK);
  const grid = page.locator(".block-core--grid");
  await expect(grid).toBeVisible();
  await page.evaluate(() => {
    const canvas = document.querySelector("canvas");
    const lose = canvas?.getContext("webgl2")?.getExtension("WEBGL_lose_context");
    lose?.loseContext();
    setTimeout(() => lose?.restoreContext(), 500);
  });
  await expect(grid).toHaveCount(0);
  await expect(grid).toBeVisible();
  await expect.poll(async () => (await squares(page)).length, { timeout: 5000 }).toBe(3);
});

test.describe("without WebGL2", () => {
  test.beforeEach(async ({ page }) => {
    await page.addInitScript(() => {
      const original = Reflect.get(HTMLCanvasElement.prototype, "getContext") as (
        ...args: unknown[]
      ) => unknown;
      Object.defineProperty(HTMLCanvasElement.prototype, "getContext", {
        value(this: HTMLCanvasElement, id: string, options?: unknown): RenderingContext | null {
          return id === "webgl2"
            ? null
            : (Reflect.apply(original, this, [id, options]) as RenderingContext | null);
        },
      });
    });
  });

  test("the fee layers stay and the page says the map needs WebGL2", async ({ page }) => {
    await page.goto(BLOCK);
    await expect(page.getByText("needs WebGL2, which this browser does not offer")).toBeVisible();
    await expect(page.locator(".block-core .strata__layer").first()).toBeVisible();
    await expect(page.locator(".block-core--grid")).toHaveCount(0);
  });
});
