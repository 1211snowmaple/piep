import { expect, test } from "@playwright/test";

test("deferred shelf remains reachable without changing watch settings", async ({ page }, testInfo) => {
  await page.goto("/#/updates");
  await page.getByRole("tab", { name: "保留・非表示", exact: true }).click();
  await expect(page.getByText("保留・非表示の作品（0件）")).toBeVisible();
  await expect(page.getByRole("button", { name: "権限変更後に再確認（0件）" })).toBeDisabled();
  await expect(page.getByText("ここにある作品は通常の更新確認・再試行では取得しません。", { exact: false })).toBeVisible();
  const widths = await page.evaluate(() => ({ actual: document.documentElement.scrollWidth, viewport: document.documentElement.clientWidth }));
  expect(widths.actual).toBeLessThanOrEqual(widths.viewport + 1);
  await page.screenshot({ path: testInfo.outputPath("acquisition-holds.png"), fullPage: true });
});

test("updates includes the reversible hold shelf in its layout", async ({ page }, testInfo) => {
  test.skip(!testInfo.project.name.endsWith("100dpi"), "Layout baselines use one scale per size and theme");
  await page.addInitScript(() => {
    localStorage.setItem("piep.nav-railed", "false");
    localStorage.setItem("piep.library-view", "gallery");
  });
  await page.goto("/#/library");
  await expect(page.getByRole("combobox", { name: "ライブラリを検索" })).toBeVisible();
  await page.goto("/#/updates");
  await expect(page.getByRole("heading", { name: "更新センター" })).toBeVisible();
  await page.addStyleTag({ content: "*,*::before,*::after{animation:none!important;transition:none!important;caret-color:transparent!important}" });
  await expect(page).toHaveScreenshot("updates.png", { fullPage: true });
});
