import { expect, test } from "@playwright/test";

const once = (name: string) => name === "1200x800-light-100dpi";

test("acquisition conditions remain staged and can be removed before applying", async ({ page }, info) => {
  test.skip(!once(info.project.name), "Interaction is shared; the layout matrix covers other windows");
  await page.goto("/#/library");
  await page.getByRole("button", { name: "絞り込み", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "詳細フィルター" });
  await expect(dialog.getByRole("tab", { name: /^保存状態/ })).toHaveAttribute("aria-selected", "true");
  await dialog.getByRole("combobox", { name: "取得状態" }).click();
  await page.getByRole("option", { name: "一部保存", exact: true }).click();
  await expect(dialog.getByLabel("適用する条件").getByRole("button", { name: "一部保存の作品を解除" })).toBeVisible();
  await expect(page).not.toHaveURL(/acquisition=/);
  await dialog.getByRole("button", { name: "適用", exact: true }).click();
  await expect(page).toHaveURL(/acquisition=incomplete/);
});

test("global search separates works from commands without losing the typed text", async ({ page }, info) => {
  test.skip(!once(info.project.name), "Interaction is shared");
  await page.goto("/#/library");
  await page.keyboard.press("Control+k");
  const search = page.getByRole("textbox", { name: "ライブラリ全体を検索", exact: true });
  await expect(search).toBeFocused();
  await search.fill("設定");
  await page.getByRole("radiogroup", { name: "検索する場所" }).getByText("画面・操作", { exact: true }).click();
  await expect(page.getByRole("textbox", { name: "画面・操作を検索", exact: true })).toHaveValue("設定");
  await page.getByRole("button", { name: /^設定を開く/ }).click();
  await expect(page).toHaveURL(/#\/settings/);
});

test("identity names wrap inside the available card width", async ({ page }) => {
  await page.goto("/#/library");
  const author = page.locator(".work-card__author .identity-name").first();
  await expect(author).toBeVisible();
  await author.evaluate((node) => { node.textContent = "長い作者名＠ウィンドウの幅に合わせて全文を折り返して表示する作者"; });
  await expect.poll(() => author.evaluate((node) => {
    const label = node as HTMLElement;
    return label.scrollWidth <= label.clientWidth + 1 && label.scrollHeight <= label.clientHeight + 1;
  })).toBe(true);
  const width = await page.evaluate(() => ({ document: document.documentElement.scrollWidth, viewport: innerWidth }));
  expect(width.document).toBeLessThanOrEqual(width.viewport + 1);
});

test("settings sections can be found by purpose and revisited with Back", async ({ page }, info) => {
  test.skip(!once(info.project.name), "Navigation is shared");
  await page.goto("/#/settings");
  await page.getByRole("textbox", { name: "設定項目を検索" }).fill("テーマ");
  await page.getByRole("button", { name: /表示と操作/ }).click();
  await expect(page).toHaveURL(/section=appearance/);
  await expect(page.getByRole("textbox", { name: "設定項目を検索" })).toHaveValue("");
  await page.getByRole("button", { name: /piepについて/ }).click();
  await expect(page).toHaveURL(/section=about/);
  await page.getByRole("button", { name: "前の画面へ戻る", exact: true }).click();
  await expect(page).toHaveURL(/section=appearance/);
  await expect(page.getByRole("heading", { name: "表示と操作", exact: true })).toBeVisible();
});

test("settings, filter states and global search have readable layouts", async ({ page }, info) => {
  test.skip(!info.project.name.endsWith("100dpi"), "DPI scaling is covered by identity and library matrices");
  await page.goto("/#/settings?section=appearance");
  await expect(page.getByRole("heading", { name: "表示と操作", exact: true })).toBeVisible();
  await page.addStyleTag({ content: "*,*::before,*::after{animation:none!important;transition:none!important;caret-color:transparent!important}" });
  await page.evaluate(async () => { await document.fonts.ready; });
  await expect(page).toHaveScreenshot("ux-settings-display.png", { fullPage: true });
  await page.goto("/#/library");
  await page.getByRole("button", { name: "絞り込み", exact: true }).click();
  await expect(page.getByRole("combobox", { name: "取得状態", exact: true })).toBeVisible();
  await expect(page).toHaveScreenshot("ux-filter-state.png");
  await page.getByRole("button", { name: "閉じる", exact: true }).click();
  await page.getByRole("button", { name: "検索または移動", exact: true }).click();
  await page.getByRole("textbox", { name: "ライブラリ全体を検索", exact: true }).fill("雨上がり");
  await expect(page.getByRole("dialog").getByRole("button", { name: /雨上がりの図書室で/ })).toBeVisible();
  await expect(page).toHaveScreenshot("ux-library-search.png");
});
