import { expect, test } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("piep.nav-railed", "false");
    localStorage.setItem("piep.library-view", "gallery");
  });
  await page.goto("/#/library");
  await expect(page.getByRole("textbox", { name: "ライブラリを検索" })).toBeVisible();
  await page.addStyleTag({ content: "*,*::before,*::after{animation:none!important;transition:none!important;caret-color:transparent!important}" });
  await page.evaluate(async () => { await document.fonts.ready; });
});

test("library shell remains stable without clipped horizontal content", async ({ page }) => {
  const geometry = await page.evaluate(() => ({
    documentWidth: document.documentElement.scrollWidth,
    viewportWidth: document.documentElement.clientWidth,
    mainWidth: document.querySelector(".app-main")?.scrollWidth ?? 0,
    mainViewport: document.querySelector(".app-main")?.clientWidth ?? 0,
  }));
  expect(geometry.documentWidth).toBeLessThanOrEqual(geometry.viewportWidth + 1);
  expect(geometry.mainWidth).toBeLessThanOrEqual(geometry.mainViewport + 1);
  await expect(page).toHaveScreenshot("library-shell.png", { fullPage: true });
});

test("phone settings navigation wraps without an inner horizontal scroller", async ({ page }, testInfo) => {
  test.skip(
    testInfo.project.name !== "900x600-light-100dpi",
    "The phone-width geometry check only needs one browser project",
  );
  await page.setViewportSize({ width: 360, height: 800 });
  await page.goto("/#/settings?section=library");
  await expect(page.getByRole("heading", { name: "ローカルライブラリ" })).toBeVisible();
  const geometry = await page.evaluate(() => {
    const navigation = document.querySelector<HTMLElement>(".settings-nav");
    const main = document.querySelector<HTMLElement>(".app-main");
    return {
      documentWidth: document.documentElement.scrollWidth,
      viewportWidth: document.documentElement.clientWidth,
      mainWidth: main?.scrollWidth ?? 0,
      mainViewport: main?.clientWidth ?? 0,
      navigationWidth: navigation?.scrollWidth ?? 0,
      navigationViewport: navigation?.clientWidth ?? 0,
    };
  });
  expect(geometry.documentWidth).toBeLessThanOrEqual(geometry.viewportWidth + 1);
  expect(geometry.mainWidth).toBeLessThanOrEqual(geometry.mainViewport + 1);
  expect(geometry.navigationWidth).toBeLessThanOrEqual(geometry.navigationViewport + 1);
});

test("critical workspaces keep a stable layout", async ({ page }, testInfo) => {
  test.skip(!testInfo.project.name.endsWith("100dpi"), "Route matrix runs once per size/theme; DPI scaling is covered by the library shell matrix");
  const routes = [
    ["diagnostics", "/#/diagnostics", "ライブラリ診断"],
    ["operations", "/#/operations", "操作履歴"],
    ["save", "/#/save/pixiv", "Webから保存"],
    ["reader", "/#/reader/1", "雨上がりの図書室で"],
    ["settings-library", "/#/settings?section=library", "ローカルライブラリ"],
    ["updates", "/#/updates", "更新センター"],
    ["epub", "/#/epub", "EPUB書き出し"],
    ["epub-templates", "/#/epub/templates", "テンプレートスタジオ"],
    ["work", "/#/works/1", "雨上がりの図書室で"],
  ] as const;
  for (const [name, route, visibleText] of routes) {
    await page.goto(route);
    await expect(page.locator("main").getByText(visibleText, { exact: true }).first()).toBeVisible();
    await page.addStyleTag({ content: "*,*::before,*::after{animation:none!important;transition:none!important;caret-color:transparent!important}" });
    await expect(page).toHaveScreenshot(`${name}.png`, { fullPage: true });
  }
});
