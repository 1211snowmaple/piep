import { expect, test } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.goto("/#/library?tab=collections");
  await page.addStyleTag({ content: "*,*::before,*::after{animation:none!important;transition:none!important}" });
  await page.getByRole("button", { name: /^まとまりを探す(?:\s+\d+)?$/ }).click();
  await expect(page.getByRole("button", { name: "保留して次へ" })).toBeVisible();
});

test("discovery review keeps names, controls and both scroll panes inside the window", async ({ page }, info) => {
  const dialog = page.getByRole("dialog");
  await expect(dialog).toBeVisible();
  const geometry = await dialog.evaluate((element) => {
    const modal = element.getBoundingClientRect();
    const footer = element.querySelector(".discovery-review__actions")!.getBoundingClientRect();
    const panes = [...element.querySelectorAll<HTMLElement>(".discovery-sidebar,.discovery-detail,.discovery-review__scroll")];
    return { bottom: modal.bottom, right: modal.right, height: window.innerHeight, width: window.innerWidth, footerInside: footer.bottom <= modal.bottom && footer.right <= modal.right, panesContained: panes.every((pane) => pane.scrollWidth <= pane.clientWidth + 1) };
  });
  expect(geometry.bottom).toBeLessThanOrEqual(geometry.height);
  expect(geometry.right).toBeLessThanOrEqual(geometry.width);
  expect(geometry.footerInside).toBe(true);
  expect(geometry.panesContained).toBe(true);
  if (info.project.name.endsWith("100dpi")) await expect(dialog).toHaveScreenshot("collection-discovery.png");
});

test("review edits survive reading a work and closing the discovery window", async ({ page }, info) => {
  test.skip(!info.project.name.endsWith("100dpi"));
  const checks = page.getByRole("checkbox");
  const firstTitle = await checks.first().getAttribute("aria-label");
  await page.locator(".discovery-member__order").nth(1).getByRole("button").first().click();
  await checks.first().uncheck();
  const movedTitle = await checks.first().getAttribute("aria-label");
  expect(movedTitle).not.toEqual(firstTitle);
  await page.getByRole("button", { name: "本文を確認" }).first().click();
  await expect(page.locator(".discovery-preview__text")).toBeVisible();
  await page.getByRole("button", { name: "候補の確認に戻る" }).click();
  await expect(checks.first()).not.toBeChecked();
  await page.getByRole("button", { name: "まとまりを探すを閉じる" }).click();
  await page.getByRole("button", { name: /^まとまりを探す(?:\s+\d+)?$/ }).click();
  await expect(checks.first()).toHaveAttribute("aria-label", movedTitle!);
  await expect(checks.first()).not.toBeChecked();
});

test("narrow discovery switches between the list and review without horizontal clipping", async ({ page }, info) => {
  test.skip(info.project.name !== "900x600-light-100dpi");
  await page.setViewportSize({ width: 360, height: 800 });
  await expect(page.locator(".discovery-sidebar")).toBeVisible();
  await expect(page.locator(".discovery-detail")).not.toBeVisible();
  await page.locator(".discovery-candidate").first().click();
  await expect(page.getByRole("button", { name: "候補の一覧へ" })).toBeVisible();
  const dialog = page.getByRole("dialog");
  expect(await dialog.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
  await expect(page.getByRole("button", { name: "保留して次へ" })).toBeVisible();
  await page.getByRole("button", { name: "候補の一覧へ" }).click();
  await expect(page.locator(".discovery-sidebar")).toBeVisible();
  // 最後の候補を保留しても、狭い画面から一覧と保留中の候補へ戻れる。
  const count = await page.locator(".discovery-candidate").count();
  await page.locator(".discovery-candidate").first().click();
  for (let index = 0; index < count; index++) await page.getByRole("button", { name: "保留して次へ" }).click();
  await page.getByRole("button", { name: "候補の一覧へ" }).click();
  await page.getByRole("button", { name: `保留 ${count}` }).click();
  await page.locator(".discovery-candidate").first().click();
  await expect(page.getByRole("button", { name: "保留を戻す" })).toBeVisible();
});
