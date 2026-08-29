import { expect, test } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("piep.nav-railed", "false");
    localStorage.setItem("piep.library-view", "gallery");
  });
  await page.goto("/#/library");
  await expect(page.getByRole("textbox", { name: "ライブラリを検索" })).toBeVisible();
  await page.addStyleTag({ content: `
    *,*::before,*::after{animation:none!important;transition:none!important;caret-color:transparent!important}
    /* Reader prose normally follows the user's serif preference. The Windows
       runner and an ordinary installation carry different Japanese serif
       faces, so layout snapshots use the same stable UI face as the shell. */
    .reader-paper{--reader-font-family:"Yu Gothic UI",sans-serif!important}
  ` });
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

test("phone collection list keeps metadata and actions inside each row", async ({ page }, testInfo) => {
  test.skip(
    testInfo.project.name !== "900x600-light-100dpi",
    "The phone-width collection geometry check only needs one browser project",
  );
  await page.setViewportSize({ width: 360, height: 800 });
  await page.evaluate(() => localStorage.setItem("piep.library-view", "compact"));
  await page.goto("/#/collections/demo-long");
  await expect(page.locator(".collection-member").first()).toBeVisible();
  const geometry = await page.evaluate(() => ({
    documentWidth: document.documentElement.scrollWidth,
    viewportWidth: document.documentElement.clientWidth,
    mainWidth: document.querySelector(".app-main")?.scrollWidth ?? 0,
    mainViewport: document.querySelector(".app-main")?.clientWidth ?? 0,
    rowsContained: [...document.querySelectorAll<HTMLElement>(".collection-member")].every((row) => {
      const actions = row.querySelector<HTMLElement>(".work-row__actions");
      if (!actions) return true;
      const rowRect = row.getBoundingClientRect();
      const actionRect = actions.getBoundingClientRect();
      return actionRect.left >= rowRect.left - 1 && actionRect.right <= rowRect.right + 1;
    }),
  }));
  expect(geometry.documentWidth).toBeLessThanOrEqual(geometry.viewportWidth + 1);
  expect(geometry.mainWidth).toBeLessThanOrEqual(geometry.mainViewport + 1);
  expect(geometry.rowsContained).toBe(true);
});

test("critical workspaces keep a stable layout", async ({ page }, testInfo) => {
  test.skip(!testInfo.project.name.endsWith("100dpi"), "Route matrix runs once per size/theme; DPI scaling is covered by the library shell matrix");
  const routes = [
    ["diagnostics", "/#/diagnostics", "ライブラリ診断"],
    ["operations", "/#/operations", "操作履歴"],
    ["save", "/#/save/pixiv", "Webから保存"],
    ["reader", "/#/reader/101", "雨上がりの図書室で"],
    ["library-collections", "/#/library?tab=collections", "新しいコレクション"],
    ["collection", "/#/collections/demo-long", "同人女の感情 ハイスペイケメン女子の綾城さんにキモデブが溺愛されて界隈の姫になる話 関連作品"],
    ["settings-library", "/#/settings?section=library", "ローカルライブラリ"],
    ["settings-assist", "/#/settings?section=assist", "AIの手伝い"],
    ["updates", "/#/updates", "更新センター"],
    ["epub", "/#/epub", "EPUB書き出し"],
    ["epub-templates", "/#/epub/templates", "テンプレートスタジオ"],
    ["work", "/#/works/101", "雨上がりの図書室で"],
  ] as const;
  for (const [name, route, visibleText] of routes) {
    await page.goto(route);
    await expect(page.locator("main").getByText(visibleText, { exact: true }).first()).toBeVisible();
    await page.addStyleTag({ content: "*,*::before,*::after{animation:none!important;transition:none!important;caret-color:transparent!important}" });
    await expect(page).toHaveScreenshot(`${name}.png`, { fullPage: true });
  }
});

test("collection list view keeps covers, order controls, and actions aligned", async ({ page }, testInfo) => {
  test.skip(!testInfo.project.name.endsWith("100dpi"), "List layout runs once per size/theme; DPI scaling is covered by the library shell matrix");
  // 見方は束だけの好みではなくなったので、beforeEach と同じ鍵を上書きする。
  // addInitScript では効かない — 下の goto は断片だけの遷移で同じ文書のままなので、
  // 初期スクリプトが一度も走らず、黙ってギャラリーの画を撮ることになっていた。
  await page.evaluate(() => localStorage.setItem("piep.library-view", "compact"));
  await page.goto("/#/collections/demo-long");
  await expect(page.locator("main").getByText("同人女の感情 ハイスペイケメン女子の綾城さんにキモデブが溺愛されて界隈の姫になる話 関連作品", { exact: true }).first()).toBeVisible();
  await page.addStyleTag({ content: "*,*::before,*::after{animation:none!important;transition:none!important;caret-color:transparent!important}" });
  await expect(page).toHaveScreenshot("collection-list.png", { fullPage: true });
});

/** pixiv の表紙と同じ形（240x338）の、読み込みを挟む絵。 */
const COVER_SHAPED_LIKE_PIXIV = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAPAAAAFSCAIAAABDlMWUAAACp0lEQVR42u3WMQ3AIBRFUSD4qAAEECZcIaJKKqNrXVVA1y7AOSPjy80P8ehXgFUkEyBoEDQIGgTNvvL36TnNwhzqcKHx5QBBg6BB0CBoBA2CBkGDoEHQCBoEDYIGQSNoEDQIGgQNgkbQIGgQNAgaBI2gQdAgaBA0CBpBg6BB0CBoEDSCBkGDoEHQIGgEDYIGQYOgQdAIGgQNggZBI2gTIGgQNAgaBI2gQdAgaBA0CBpBg6BB0CBoEDSCBkGDoEHQIGgEDYIGQYOgQdAIGgQNggZBg6ARNAgaBA2CBkEjaBA0CBoEjaBB0CBoEDQIGkGDoEHQIGgQNIIGQYOgQdAgaAQNggZBg6BB0AgaBA2CBkGDoBE0CBoEDYIGQSNoEDQIGgQNgkbQIGgQNAgaQYOgQdAgaBA0ggZBg6BB0CBoBA2CBkGDoEHQCBoEDYIGQYOgETQIGgQNggZBI2gQNAgaBA2CRtAgaBA0CBpBg6BB0CBoEDSCBkGDoEHQIGgEDYIGQYOgQdAIGgQNggZBg6ARNAgaBA2CBkEjaBA0CBoEDYJG0CBoEDQIGgSNoEHQIGgQNIIGQYOgQdAgaAQNggZBg6BB0AgaBA2CBkGDoBE0CBoEDYIGQSNoEDQIGgQNgkbQIGgQNAgaBI2gQdAgaBA0ggZBg6BB0CBoBA2CBkGDoEHQCBoEDYIGQYOgETQIGgQNggZBI2gQNAgaBA2CRtAgaBA0CBoEjaBB0CBoEDQIGkGDoEHQIGgEDYIGQYOgQdAIGgQNggZBg6ARNAgaBA2CBkEjaBA0CBoEDYJG0CBoEDQIGgSNoEHQIGgQNAgaQYOgQdAgaARtAgQNggZBg6ARNCwh3q1YARcaBA2CBkEjaBA0CBoEDYJG0CBoEDQIGgSNoEHQIGj4zwvYRAbPx8pufgAAAABJRU5ErkJggg==";

/**
 * 表紙は、どの枠でも**全部**見えていなければならない。
 *
 * `.work-cover__image` は `width/height: 100%` と `object-fit: contain` を
 * 指定していたが、grid の行が内容で決まる場面では割合の高さが auto に落ちる。
 * 落ちると画像は幅なりの実寸に伸び、枠の `overflow: hidden` が**下を切る**。
 * リストの表紙が実際にそうなっていた（枠 92x123 に対して絵の箱が 90x127）。
 *
 * 見た目の突き合わせでは気づけない - 切れた状態で基準を撮ってしまえば、以後
 * それが正解になる。**数で確かめる。**
 */
test("every cover fits inside its frame instead of spilling out of it", async ({ page }, testInfo) => {
  test.skip(
    testInfo.project.name !== "1440x900-light-100dpi",
    "The geometry is the same at every scale; one project is enough",
  );
  for (const [view, route] of [
    ["gallery", "/#/library"],
    ["compact", "/#/library"],
  ] as const) {
    // `beforeEach` の初期化スクリプトは**毎回の遷移で走る**。あとから
    // `evaluate` で書いても、次の `goto` で書き戻される。後ろに積む。
    await page.addInitScript((mode) => localStorage.setItem("piep.library-view", JSON.stringify(mode)), view);
    await page.goto(route);
    await page.waitForSelector("img.work-cover__image");
    // 差し替えの表紙は寸法の分かる SVG なので、**読み込みを待たずに枠が決まる**。
    // 実際の表紙は JPEG で、寸法が分かるのは読み終えたあと。その順番でしか
    // 出ない不具合なので、ここで本物と同じ形（pixiv の 240x338）の絵に
    // 差し替えて測る。要素は作り直さない - React が置いたものをそのまま使う。
    await page.evaluate((src) => {
      for (const image of document.querySelectorAll("img.work-cover__image")) {
        (image as HTMLImageElement).src = src;
      }
    }, COVER_SHAPED_LIKE_PIXIV);
    await page.waitForFunction(() =>
      [...document.querySelectorAll("img.work-cover__image")].every((image) => (image as HTMLImageElement).naturalWidth === 240),
    );
    const result = await page.evaluate(() => {
      const images = [...document.querySelectorAll("img.work-cover__image")].filter(
        (image) => (image as HTMLImageElement).naturalWidth > 0,
      );
      const spills = images.flatMap((image) => {
        const frame = image.parentElement;
        if (!frame) return [];
        const f = frame.getBoundingClientRect();
        const b = image.getBoundingClientRect();
        // 半端な画素の丸めは許す。溢れは1画素より大きく出る。
        if (b.width <= f.width + 1 && b.height <= f.height + 1) return [];
        return [{ frame: `${f.width.toFixed(1)}x${f.height.toFixed(1)}`, image: `${b.width.toFixed(1)}x${b.height.toFixed(1)}` }];
      });
      return { checked: images.length, spills };
    });
    // 0件を「合格」にしない。見ていないだけかもしれない。
    expect(result.checked, `${view} に測れる表紙が無い`).toBeGreaterThan(0);
    expect(result.spills, `${view} で表紙が枠からはみ出している`).toEqual([]);
  }
});
