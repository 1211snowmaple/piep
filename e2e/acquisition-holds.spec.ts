import { expect, test } from "@playwright/test";

test("deferred shelf remains reachable without changing watch settings", async ({ page }, testInfo) => {
  await page.goto("/#/updates");
  await page.getByRole("tab", { name: "要確認", exact: true }).click();
  await expect(page.getByText("要確認（0件）")).toBeVisible();
  await expect(page.getByRole("button", { name: "候補に戻す（0）" })).toBeDisabled();
  await expect(page.getByText("取得できなかった理由を残しています。", { exact: false })).toBeVisible();
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
