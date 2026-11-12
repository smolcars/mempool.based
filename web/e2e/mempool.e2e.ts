import { expect, test } from "./fixtures";

test("the home page links to the mempool's history, which switches interval in the address", async ({
  page,
}) => {
  await page.goto("/regtest/");
  await page.getByRole("link", { name: "Mempool history" }).click();
  await expect(page).toHaveURL(/\/regtest\/mempool$/);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Mempool history");
  const periods = page.getByRole("navigation", { name: "Period" });
  await expect(periods.getByRole("link", { name: /the last 24 hours/ })).toHaveAttribute(
    "aria-current",
    "page",
  );
  // A young server has nothing yet or its first minutes: either is an answer, never an error.
  const answered = page
    .getByText("Nothing recorded yet")
    .or(page.getByRole("slider", { name: /The mempool's size by fee rate/ }));
  await expect(answered).toBeVisible();
  await periods.getByRole("link", { name: /the last 2 hours/ }).click();
  await expect(page).toHaveURL(/\/regtest\/mempool\?interval=2h$/);
  await expect(periods.getByRole("link", { name: /the last 2 hours/ })).toHaveAttribute(
    "aria-current",
    "page",
  );
  await expect(answered).toBeVisible();
});
