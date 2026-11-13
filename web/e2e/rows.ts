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
  // A table fits and marks its edge a frame after it changes: give it a moment to settle.
  await expect(async () => {
    await tablesFitNow(page, label);
  }).toPass({ timeout: 3000 });
}

async function tablesFitNow(page: Page, label: string) {
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

/**
 * Every amount on the page whole: a number never wraps inside itself or away from its unit, and
 * never runs out of the field that holds it into its neighbour.
 */
export async function amountsIntact(page: Page, label: string) {
  const faults = await page.evaluate(() => {
    const SELECTOR = ".amount, .fee, .ln-msat, .num";
    const out: string[] = [];
    for (const el of document.querySelectorAll<HTMLElement>(`main :is(${SELECTOR})`)) {
      // The innermost figure only, shown, with a digit in it.
      if (el.querySelector(SELECTOR) || el.getClientRects().length === 0) continue;
      // Times are phrases ("1 day 9 h ago"), free to wrap between their words.
      if (el.closest(".visually-hidden, [hidden], svg, time")) continue;
      const text = (el.textContent ?? "").trim();
      if (!/\d/.test(text)) continue;
      const range = document.createRange();
      range.selectNodeContents(el);
      const tops = [...range.getClientRects()]
        .filter((r) => r.width > 0 && r.height > 0)
        .map((r) => r.top);
      tops.sort((a, b) => a - b);
      const lines = tops.filter((t, i) => i === 0 || t - (tops[i - 1] ?? t) > 6).length;
      if (lines > 1) out.push(`"${text}" wraps onto ${lines} lines`);
      const cell = el.closest("td, th, dd, li") ?? el.parentElement;
      if (!cell) continue;
      const r = el.getBoundingClientRect();
      const c = cell.getBoundingClientRect();
      if (r.left < c.left - 1 || r.right > c.right + 1) {
        const where = `${cell.tagName.toLowerCase()}.${cell.className}`;
        out.push(
          `"${text}" runs out of its field (${where}: ${Math.round(r.left)}–${Math.round(r.right)} in ${Math.round(c.left)}–${Math.round(c.right)})`,
        );
      }
    }
    return out;
  });
  expect(faults, label).toStrictEqual([]);
}

/**
 * Every exact-amount control on the page, opened one after another: its pop-up wholly on screen,
 * its text not cut, nothing else on the page moved by it, and Escape closing it.
 */
export async function exactsInside(page: Page, label: string) {
  const controls = page.locator("main button[popovertarget]");
  const where = () =>
    page.evaluate(() =>
      [...document.querySelectorAll<HTMLElement>("main :is(dt, dd, td, th, button, h1, h2)")]
        .filter((e) => !e.closest(".exact"))
        .map((e) => {
          const r = e.getBoundingClientRect();
          return `${Math.round(r.left)},${Math.round(r.top)}`;
        }),
    );
  for (let i = 0; i < (await controls.count()); i++) {
    const control = controls.nth(i);
    await control.scrollIntoViewIfNeeded();
    const before = await where();
    await control.click();
    const pop = page.locator(".exact[data-placed]");
    await expect(pop, `${label}: control ${i} opens`).toHaveCount(1);
    const fault = await pop.evaluate((el) => {
      const r = el.getBoundingClientRect();
      const w = document.documentElement.clientWidth;
      const h = document.documentElement.clientHeight;
      if (r.left < 0 || r.top < 0 || r.right > w || r.bottom > h) {
        return `off screen at ${Math.round(r.left)}–${Math.round(r.right)} × ${Math.round(r.top)}–${Math.round(r.bottom)} in ${w} × ${h}`;
      }
      return el.scrollWidth > el.clientWidth + 1 ? "its text is cut" : "";
    });
    expect(fault, `${label}: "${await control.textContent()}"`).toBe("");
    expect(
      await where(),
      `${label}: opening "${await control.textContent()}" moved the page`,
    ).toEqual(before);
    await page.keyboard.press("Escape");
    await expect(page.locator(".exact:popover-open")).toHaveCount(0);
  }
}
