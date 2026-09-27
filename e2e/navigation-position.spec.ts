import { expect, test, type Page } from "@playwright/test";

/**
 * Where each screen starts after a navigation.
 *
 * These run in a real browser because the behaviour is layout: a page that is
 * briefly too short to hold an offset has it clamped away, and neither jsdom
 * nor a browser that is not painting frames reproduces that.
 */

const MAIN = "#main-content";

/** Only one project needs to run these; the behaviour has no size, theme or DPI axis. */
function onlyOnce(name: string) {
  test.skip(name !== "1200x800-light-100dpi", "Position behaviour has no size, theme or DPI axis");
}

async function scrollMain(page: Page, top: number) {
  await page.evaluate(async ([selector, value]) => {
    const main = document.querySelector(selector as string)!;
    // Programmatic restoration deliberately ignores programmatic scroll events.
    // Signal the same intent as grabbing a scrollbar before moving it, so this
    // helper tests a reader's movement rather than fighting an active restore.
    main.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
    main.scrollTop = value as number;
    // The app records the position from a scroll event, and the browser only
    // dispatches those while producing frames. Waiting a fixed few milliseconds
    // is not the same thing on a loaded machine.
    const frame = () => new Promise((resolve) => requestAnimationFrame(resolve));
    await frame();
    await frame();
  }, [MAIN, top]);
}

const mainScrollTop = (page: Page) => page.evaluate((selector) => document.querySelector(selector)!.scrollTop, MAIN);

/**
 * The furthest down the current screen can be scrolled.
 *
 * A shorter panel cannot hold the offset a taller one was at, and the browser
 * clamps it - that is the page ending, not the app deciding to move anybody.
 */
const maxScrollTop = (page: Page) => page.evaluate((selector) => {
  const main = document.querySelector(selector)!;
  return Math.max(0, main.scrollHeight - main.clientHeight);
}, MAIN);

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("piep.nav-railed", "false");
    localStorage.setItem("piep.library-view", "compact");
  });
});

test("going back returns to the row the work was opened from", async ({ page }, testInfo) => {
  onlyOnce(testInfo.project.name);
  await page.goto("/#/library");
  await expect(page.getByRole("combobox", { name: "ライブラリを検索" })).toBeVisible();
  await page.waitForTimeout(400);

  await scrollMain(page, 500);
  expect(await mainScrollTop(page)).toBe(500);

  // Dispatched rather than clicked: a real click first scrolls its target into
  // view, which would undo the position this test is about before opening it.
  await page.getByRole("link", { name: /を開く$/ }).first().dispatchEvent("click");
  await expect(page).toHaveURL(/#\/works\//);
  // 住所が変わった瞬間ではなく、行き先が出てから測る。分割読み込みのあいだは
  // 前の画面を出したままにする（白い枠へ置き換えない）ので、住所の変化と
  // 画面の入れ替わりは同じ瞬間ではない。**利用者が見るのも入れ替わった後**で、
  // その時点で上から始まっていることがこの試験の言いたいことである。
  await expect(page.getByRole("combobox", { name: "ライブラリを検索" })).toBeHidden();
  // A new destination opens at its own top.
  await expect.poll(() => mainScrollTop(page), { timeout: 4000 }).toBe(0);

  await page.getByLabel("前の画面へ戻る").click();
  await expect(page).toHaveURL(/#\/library/);
  await expect(page.getByRole("combobox", { name: "ライブラリを検索" })).toBeVisible();
  // The restore keeps reapplying while the list is still measuring itself, and
  // on a loaded machine that takes a while.
  await expect.poll(() => mainScrollTop(page), { timeout: 8000 }).toBe(500);
});

test("the history controls say whether they lead anywhere", async ({ page }, testInfo) => {
  onlyOnce(testInfo.project.name);
  await page.goto("/#/library");
  await expect(page.getByRole("combobox", { name: "ライブラリを検索" })).toBeVisible();

  // Nothing has been visited yet. A desktop window has no browser chrome, so a
  // lit control that does nothing is indistinguishable from a hung app.
  await expect(page.getByLabel("前の画面へ戻る")).toBeDisabled();
  await expect(page.getByLabel("次の画面へ進む")).toBeDisabled();

  await page.getByRole("link", { name: /を開く$/ }).first().click();
  await expect(page).toHaveURL(/#\/works\//);
  await expect(page.getByLabel("前の画面へ戻る")).toBeEnabled();
  await expect(page.getByLabel("次の画面へ進む")).toBeDisabled();

  await page.getByLabel("前の画面へ戻る").click();
  await expect(page).toHaveURL(/#\/library/);
  await expect(page.getByLabel("次の画面へ進む")).toBeEnabled();
});

test("choosing a tab does not move the page", async ({ page }, testInfo) => {
  onlyOnce(testInfo.project.name);
  await page.goto("/#/works/101?tab=content");
  await expect(page.getByRole("tab", { name: "本文" })).toBeVisible();
  await page.waitForTimeout(400);

  await scrollMain(page, 300);
  const before = await mainScrollTop(page);
  expect(before).toBeGreaterThan(0);

  // A tab changes what is listed, not where the reader is standing. Ground that
  // moves under a press meant only to change the contents is worse than any
  // position the move could have picked.
  await page.getByRole("tab", { name: "概要" }).click();
  await page.waitForTimeout(600);
  expect(await mainScrollTop(page)).toBe(Math.min(before, await maxScrollTop(page)));
});

test("the reader returns to the detail screen instead of opening a second one", async ({ page }, testInfo) => {
  onlyOnce(testInfo.project.name);
  await page.goto("/#/works/101?tab=assets");
  await expect(page.getByRole("tab", { name: /アセット/ })).toBeVisible();

  await page.getByRole("button", { name: "読む" }).click();
  await expect(page).toHaveURL(/#\/reader\/101/);

  await page.getByLabel("作品詳細へ戻る").click();
  // The tab it was opened from is still selected, which a pushed copy loses.
  await expect(page).toHaveURL(/#\/works\/101\?tab=assets/);
  // And the reader is now ahead rather than behind, so back does not lead into it.
  await expect(page.getByLabel("次の画面へ進む")).toBeEnabled();
});

test("choosing a library tab does not move the page either", async ({ page }, testInfo) => {
  onlyOnce(testInfo.project.name);
  await page.goto("/#/library");
  await expect(page.getByRole("combobox", { name: "ライブラリを検索" })).toBeVisible();
  await page.waitForTimeout(300);

  await scrollMain(page, 400);
  const before = await mainScrollTop(page);
  await page.getByRole("tab", { name: "作者・クリエイター" }).click();
  await page.waitForTimeout(600);
  expect(await mainScrollTop(page)).toBe(Math.min(before, await maxScrollTop(page)));
});

test("opening another library shelf starts that listing at the top", async ({ page }, testInfo) => {
  onlyOnce(testInfo.project.name);
  await page.goto("/#/library");
  await expect(page.getByRole("combobox", { name: "ライブラリを検索" })).toBeVisible();
  await page.waitForTimeout(300);

  await scrollMain(page, 500);
  expect(await mainScrollTop(page)).toBe(500);

  // This is a same-path push, but it is not a continuation of the current
  // listing. Keeping 500px would either skip the first favourites or clamp the
  // reader to the bottom when the shelf is shorter than the full library.
  await page.getByRole("button", { name: /^お気に入り 84$/ }).click();
  await expect(page).toHaveURL(/#\/library\?favorite=1/);
  await expect.poll(() => mainScrollTop(page), { timeout: 8000 }).toBe(0);
});

test("reselecting the active library shelf returns it to the top", async ({ page }, testInfo) => {
  onlyOnce(testInfo.project.name);
  await page.goto("/#/library");
  await expect(page.getByRole("combobox", { name: "ライブラリを検索" })).toBeVisible();
  await page.waitForTimeout(300);

  await scrollMain(page, 500);
  expect(await mainScrollTop(page)).toBe(500);
  await page.getByRole("button", { name: /^すべて 1,284$/ }).click();
  await expect.poll(() => mainScrollTop(page), { timeout: 8000 }).toBe(0);
  await expect(page).toHaveURL(/#\/library$/);
});

test("an author screen opens at its profile, not at its works", async ({ page }, testInfo) => {
  onlyOnce(testInfo.project.name);
  await page.setViewportSize({ width: 900, height: 600 });
  await page.goto("/#/library?tab=people");
  await expect(page.getByRole("combobox", { name: "ライブラリを検索" })).toBeVisible();
  await page.waitForTimeout(300);

  // A detail route is a new place. It starts at its own top even when the
  // virtualised list we left was scrolled, rather than inheriting that offset.
  await scrollMain(page, 180);
  expect(await mainScrollTop(page)).toBeGreaterThan(0);

  await page.getByRole("link", { name: /を開く$/ }).first().dispatchEvent("click");
  await expect(page).toHaveURL(/#\/people\//);
  // A stored preference settling a tick after mount used to count as a page
  // change here, which scrolled straight past the profile.
  await page.waitForTimeout(900);
  expect(await mainScrollTop(page)).toBe(0);
});

test("an author cannot leak its scroll position into the library history entry", async ({ page }, testInfo) => {
  onlyOnce(testInfo.project.name);
  await page.goto("/#/library?tab=people");
  await expect(page.getByRole("combobox", { name: "ライブラリを検索" })).toBeVisible();
  await expect.poll(() => mainScrollTop(page)).toBe(0);

  await page.getByRole("link", { name: /を開く$/ }).first().dispatchEvent("click");
  await expect(page).toHaveURL(/#\/people\//);
  await expect(page.getByRole("combobox", { name: "ライブラリを検索" })).toBeHidden();

  await scrollMain(page, 1_000_000);
  const authorBottom = await mainScrollTop(page);
  expect(authorBottom).toBeGreaterThan(0);

  await page.getByLabel("前の画面へ戻る").click();
  await expect(page).toHaveURL(/#\/library\?tab=people/);
  await expect(page.getByRole("combobox", { name: "ライブラリを検索" })).toBeVisible();
  await expect.poll(() => mainScrollTop(page), { timeout: 8000 }).toBe(0);

  // The two offsets remain independent in both directions. Going forward is
  // allowed to restore the author's bottom, but must not make it the library's.
  await page.getByLabel("次の画面へ進む").click();
  await expect(page).toHaveURL(/#\/people\//);
  await expect(page.getByRole("combobox", { name: "ライブラリを検索" })).toBeHidden();
  await expect.poll(() => mainScrollTop(page), { timeout: 8000 }).toBe(authorBottom);

  // Back and a new push reuse the discarded forward entry's numeric index.
  // That slot must belong to the new author, not retain the old author's bottom.
  const firstAuthorUrl = page.url();
  await page.getByLabel("前の画面へ戻る").click();
  await expect(page.getByRole("combobox", { name: "ライブラリを検索" })).toBeVisible();
  await page.getByRole("link", { name: /を開く$/ }).nth(1).dispatchEvent("click");
  await expect(page).toHaveURL(/#\/people\//);
  expect(page.url()).not.toBe(firstAuthorUrl);
  await expect.poll(() => mainScrollTop(page), { timeout: 8000 }).toBe(0);

  await page.getByLabel("前の画面へ戻る").click();
  await expect(page.getByRole("combobox", { name: "ライブラリを検索" })).toBeVisible();
  await page.getByLabel("次の画面へ進む").click();
  await expect(page).toHaveURL(/#\/people\//);
  await expect.poll(() => mainScrollTop(page), { timeout: 8000 }).toBe(0);
});

test("the filter spotlight switches sections without default scrolling", async ({ page }, testInfo) => {
  onlyOnce(testInfo.project.name);
  await page.goto("/#/library");
  await expect(page.getByRole("combobox", { name: "ライブラリを検索" })).toBeVisible();

  await page.getByRole("button", { name: "絞り込み" }).click();
  const spotlight = page.getByRole("dialog", { name: "詳細フィルター" });
  const apply = spotlight.getByRole("button", { name: "適用" });
  await expect(apply).toBeVisible();

  // The modal keeps one stable editing area. Changing the subject replaces
  // that area instead of growing one long, pre-scrolled form.
  await page.setViewportSize({ width: 900, height: 600 });
  await page.waitForTimeout(300);
  const stableFrame = await spotlight.evaluate((dialog) => ({
    height: dialog.clientHeight,
    top: dialog.getBoundingClientRect().top,
  }));
  for (const name of [/^基本/, /^状態/, /^作者/, /^タグ/, /^日付/, /^内容・ファイル/]) {
    await spotlight.getByRole("tab", { name }).click();
    await page.waitForTimeout(80);
    expect(await spotlight.evaluate((dialog) => {
      const panel = dialog.querySelector(".filter-spotlight__panel") as HTMLElement;
      const button = [...dialog.querySelectorAll("button")].find((candidate) => candidate.textContent?.trim() === "適用")!;
      return {
        height: dialog.clientHeight,
        top: dialog.getBoundingClientRect().top,
        dialogOverflows: dialog.scrollHeight > dialog.clientHeight,
        panelStartsAtTop: panel.scrollTop === 0,
        applyOnScreen: button.getBoundingClientRect().bottom <= window.innerHeight,
      };
    })).toEqual({ ...stableFrame, dialogOverflows: false, panelStartsAtTop: true, applyOnScreen: true });
  }

  // A busy selection must not push the actions away or make the dialog itself
  // wider. The two summary lanes scroll their own tokens when necessary.
  await spotlight.getByRole("tab", { name: /^作者/ }).click();
  for (const author of ["青葉しおり", "遠野つむぎ", "背徳亭無題＠ボイスドラマ発売中", "mizu atelier"]) {
    await spotlight.getByRole("button", { name: `${author}を含める作者へ追加` }).click();
  }
  await spotlight.getByRole("button", { name: "白鳥ケイを除外する作者へ追加" }).click();
  await spotlight.getByRole("tab", { name: /^タグ/ }).click();
  const includeTagButtons = spotlight.getByRole("button", { name: /を含めるタグへ追加/ });
  for (let index = 0; index < 4; index += 1) await includeTagButtons.nth(index).click();
  await spotlight.getByRole("tab", { name: /^内容・ファイル/ }).click();
  const includedLane = spotlight.getByRole("region", { name: "含める条件" });
  const includedTokens = includedLane.getByLabel("含める条件の選択項目");
  const nextIncluded = includedLane.getByRole("button", { name: "含める条件を後ろへ" });
  await expect(nextIncluded).toBeVisible();
  await expect(nextIncluded).toBeEnabled();
  const leftBefore = await includedTokens.evaluate((element) => element.scrollLeft);
  await nextIncluded.click();
  await expect.poll(() => includedTokens.evaluate((element) => element.scrollLeft)).toBeGreaterThan(leftBefore);
  await expect(includedLane.getByRole("button", { name: "含める条件を前へ" })).toBeEnabled();
  expect(await spotlight.evaluate((dialog) => {
    const summary = dialog.querySelector(".filter-spotlight__selection-summary") as HTMLElement;
    const lanes = [...summary.querySelectorAll<HTMLElement>(".filter-spotlight__selection-lane")];
    const actions = dialog.querySelector(".filter-form__actions") as HTMLElement;
    const reset = [...actions.querySelectorAll("button")].find((candidate) => candidate.textContent?.trim() === "リセット")!;
    const applyButton = [...actions.querySelectorAll("button")].find((candidate) => candidate.textContent?.trim() === "適用")!;
    return {
      height: dialog.clientHeight,
      top: dialog.getBoundingClientRect().top,
      dialogHasHorizontalOverflow: dialog.scrollWidth > dialog.clientWidth,
      summaryHasHorizontalOverflow: summary.scrollWidth > summary.clientWidth,
      summaryUsesFullWidthRows: lanes.every((lane) => lane.clientWidth >= summary.clientWidth - 32),
      summaryRowsAreStacked: lanes[1].getBoundingClientRect().top >= lanes[0].getBoundingClientRect().bottom,
      actionsHaveHorizontalOverflow: actions.scrollWidth > actions.clientWidth,
      actionsShareRow: Math.abs(reset.getBoundingClientRect().top - applyButton.getBoundingClientRect().top) < 2,
      applyOnScreen: applyButton.getBoundingClientRect().bottom <= window.innerHeight,
    };
  })).toEqual({
    ...stableFrame,
    dialogHasHorizontalOverflow: false,
    summaryHasHorizontalOverflow: false,
    summaryUsesFullWidthRows: true,
    summaryRowsAreStacked: true,
    actionsHaveHorizontalOverflow: false,
    actionsShareRow: true,
    applyOnScreen: true,
  });
});

test("the page you were on survives opening something from it", async ({ page }, testInfo) => {
  onlyOnce(testInfo.project.name);
  // Five to a page so the demo library has more than one of them.
  await page.addInitScript(() => {
    localStorage.setItem("piep.paging-mode", JSON.stringify("pages"));
    localStorage.setItem("piep.page-size", JSON.stringify(5));
  });
  await page.goto("/#/library?tab=people");
  await expect(page.getByRole("combobox", { name: "ライブラリを検索" })).toBeVisible();

  // Authors and series are counted, so the pager can name the last page rather
  // than growing one number at a time.
  await expect(page.locator(".list-pager")).toContainText("6件中 1 / 2ページ");

  await page.getByRole("button", { name: "2ページ目" }).click();
  await expect(page).toHaveURL(/page=2/);

  await page.getByRole("link", { name: /を開く$/ }).first().click();
  await expect(page).toHaveURL(/#\/people\//);

  // The preference is read a render late by default, and for that one render
  // the app believed it was on scrolling mode - which deleted the page number
  // from the address on the way back in.
  await page.getByLabel("前の画面へ戻る").click();
  await expect(page).toHaveURL(/page=2/);
});

test("the toolbar controls sit on one line, not stepped", async ({ page }, testInfo) => {
  onlyOnce(testInfo.project.name);
  await page.goto("/#/library");
  await expect(page.getByRole("combobox", { name: "ライブラリを検索" })).toBeVisible();

  // The search field is taller than every control beside it, so aligning tops
  // left the whole row after it sitting a few pixels high against it.
  const centres = await page.evaluate(() => {
    const row = document.querySelector(".library-toolbar .mantine-Group-root") as HTMLElement;
    // タブごとに意味を持たない道具は、消さずに `display: contents` の包みへ入れて
    // 隠す。包みは箱を持たないので、行の子をそのまま測ると中の押しボタンを丸ごと
    // 見落とす。包みは開いてから測る。
    const controls = [...row.children].flatMap((child) =>
      getComputedStyle(child).display === "contents" ? [...child.children] : [child]);
    return controls
      .map((child) => child.getBoundingClientRect())
      .filter((rect) => rect.height > 0)
      .map((rect) => Math.round(rect.top + rect.height / 2));
  });
  expect(centres.length).toBeGreaterThan(3);
  expect(new Set(centres).size).toBe(1);
});
