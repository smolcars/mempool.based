// What every list and table on the site promises, checked the same way wherever it appears.
import type { Locator, Page } from "@playwright/test";
import { expect } from "./fixtures";

/**
 * Taps the first row of `table` at a point away from its own links and buttons, and checks it
 * opened the row's item. Shared with the Lightning lists, which run on recorded answers.
 */
export async function tapRow(page: Page, table: Locator, label: string) {
  const row = table.locator("tbody tr").first();
  await expect(row).toBeVisible({ timeout: 30_000 });
  const href = await row.locator(".table__rowlink").getAttribute("href");
  expect(href, `${label}: the row leads somewhere`).not.toBeNull();
  // A point on the row that only the row's link answers: right to left, as a thumb on its figures.
  await row.scrollIntoViewIfNeeded();
  const spot = await row.evaluate((tr) => {
    const box = tr.getBoundingClientRect();
    const link = tr.querySelector(".table__rowlink");
    const y = box.top + box.height / 2;
    for (let x = Math.min(box.right, window.innerWidth) - 6; x > box.left; x -= 6) {
      if (document.elementFromPoint(x, y) === link) return { x, y };
    }
    return null;
  });
  expect(spot, `${label}: some of the row is not a link or a button`).not.toBeNull();
  await page.mouse.click(spot?.x ?? 0, spot?.y ?? 0);
  await expect(page, label).toHaveURL(new URL(href ?? "", page.url()).href);
}

/**
 * Every table on the page fits its panel, or shows that it scrolls: its cut edge fades and it
 * takes the keyboard. No header is cut, mid-word or otherwise.
 */
export async function tablesFit(page: Page, label: string) {
  const tables = await page.locator(".table-scroll").evaluateAll((all) =>
    all
      .filter((s) => s.getClientRects().length > 0)
      .map((s) => ({
        name: s.querySelector("caption")?.textContent ?? "",
        over: s.scrollWidth - s.clientWidth,
        cue: s instanceof HTMLElement && ("moreEnd" in s.dataset || "moreStart" in s.dataset),
        focusable: s.getAttribute("tabindex") === "0",
        cut: [...s.querySelectorAll("th")]
          .filter((th) => th.getClientRects().length > 0 && th.scrollWidth > th.clientWidth + 1)
          .map((th) => th.textContent),
      })),
  );
  for (const t of tables) {
    if (t.over > 1) {
      expect(t.cue && t.focusable, `${label}: "${t.name}" scrolls ${t.over} px unmarked`).toBe(
        true,
      );
    }
    expect(t.cut, `${label}: "${t.name}" cuts a header`).toStrictEqual([]);
  }
  const wide = await page.evaluate(() => document.documentElement.scrollWidth);
  expect(wide, `${label}: page width`).toBeLessThanOrEqual(page.viewportSize()?.width ?? 0);
}
