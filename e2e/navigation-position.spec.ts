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
    document.querySelector(selector as string)!.scrollTop = value as number;
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
  await expect(page.getByRole("textbox", { name: "ライブラリを検索" })).toBeVisible({ timeout: 15_000 });
  await page.waitForTimeout(400);

  await scrollMain(page, 500);
  expect(await mainScrollTop(page)).toBe(500);

  // Dispatched rather than clicked: a real click first scrolls its target into
  // view, which would undo the position this test is about before opening it.
  await page.getByRole("link", { name: /を開く$/ }).first().dispatchEvent("click");
  await expect(page).toHaveURL(/#\/works\//);
  // A new destination opens at its own top.
  expect(await mainScrollTop(page)).toBe(0);

  await page.getByLabel("前の画面へ戻る").click();
  await expect(page).toHaveURL(/#\/library/);
  await expect(page.getByRole("textbox", { name: "ライブラリを検索" })).toBeVisible();
  // The restore keeps reapplying while the list is still measuring itself, and
  // on a loaded machine that takes a while.
  await expect.poll(() => mainScrollTop(page), { timeout: 8000 }).toBe(500);
});

test("the history controls say whether they lead anywhere", async ({ page }, testInfo) => {
  onlyOnce(testInfo.project.name);
  await page.goto("/#/library");
  await expect(page.getByRole("textbox", { name: "ライブラリを検索" })).toBeVisible({ timeout: 15_000 });

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
  await expect(page.getByRole("tab", { name: "本文" })).toBeVisible({ timeout: 15_000 });
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
  await expect(page.getByRole("tab", { name: /アセット/ })).toBeVisible({ timeout: 15_000 });

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
  await expect(page.getByRole("textbox", { name: "ライブラリを検索" })).toBeVisible({ timeout: 15_000 });
  await page.waitForTimeout(300);

  await scrollMain(page, 400);
  const before = await mainScrollTop(page);
  await page.getByRole("tab", { name: "作者・クリエイター" }).click();
  await page.waitForTimeout(600);
  expect(await mainScrollTop(page)).toBe(Math.min(before, await maxScrollTop(page)));
});

test("an author screen opens at its profile, not at its works", async ({ page }, testInfo) => {
  onlyOnce(testInfo.project.name);
  await page.goto("/#/library?tab=people");
  await expect(page.getByRole("textbox", { name: "ライブラリを検索" })).toBeVisible({ timeout: 15_000 });
  await page.waitForTimeout(300);

  await page.getByRole("link", { name: /を開く$/ }).first().click();
  await expect(page).toHaveURL(/#\/people\//);
  // A stored preference settling a tick after mount used to count as a page
  // change here, which scrolled straight past the profile.
  await page.waitForTimeout(900);
  expect(await mainScrollTop(page)).toBe(0);
});

test("the filter drawer keeps its apply button on screen", async ({ page }, testInfo) => {
  onlyOnce(testInfo.project.name);
  await page.goto("/#/library");
  await expect(page.getByRole("textbox", { name: "ライブラリを検索" })).toBeVisible({ timeout: 15_000 });

  await page.getByRole("button", { name: "絞り込み" }).click();
  const apply = page.getByRole("button", { name: "適用" });
  await expect(apply).toBeVisible();

  // Shortened until the form has to scroll, which is the case the two buttons
  // used to fall out of: they were the last thing in the form, so on an
  // ordinary window they opened below the bottom edge.
  await page.setViewportSize({ width: 1200, height: 560 });
  await page.waitForTimeout(300);
  expect(await page.evaluate(() => {
    const fields = document.querySelector(".filter-form__fields") as HTMLElement;
    fields.scrollTop = fields.scrollHeight;
    const last = document.querySelector(".filter-form__fields > :last-child") as HTMLElement;
    const button = [...document.querySelectorAll("button")].find((b) => b.textContent?.trim() === "適用")!;
    return {
      overflows: fields.scrollHeight > fields.clientHeight,
      // And the end of the form is still reachable underneath them.
      lastFieldReachable: last.getBoundingClientRect().bottom <= fields.getBoundingClientRect().bottom + 1,
      applyOnScreen: button.getBoundingClientRect().bottom <= window.innerHeight,
    };
  })).toEqual({ overflows: true, lastFieldReachable: true, applyOnScreen: true });
});

test("the page you were on survives opening something from it", async ({ page }, testInfo) => {
  onlyOnce(testInfo.project.name);
  // Five to a page so the demo library has more than one of them.
  await page.addInitScript(() => {
    localStorage.setItem("piep.paging-mode", JSON.stringify("pages"));
    localStorage.setItem("piep.page-size", JSON.stringify(5));
  });
  await page.goto("/#/library?tab=people");
  await expect(page.getByRole("textbox", { name: "ライブラリを検索" })).toBeVisible({ timeout: 15_000 });

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
  await expect(page.getByRole("textbox", { name: "ライブラリを検索" })).toBeVisible({ timeout: 15_000 });

  // The search field is taller than every control beside it, so aligning tops
  // left the whole row after it sitting a few pixels high against it.
  const centres = await page.evaluate(() => {
    const row = document.querySelector(".library-toolbar .mantine-Group-root") as HTMLElement;
    return [...row.children]
      .map((child) => child.getBoundingClientRect())
      .filter((rect) => rect.height > 0)
      .map((rect) => Math.round(rect.top + rect.height / 2));
  });
  expect(centres.length).toBeGreaterThan(3);
  expect(new Set(centres).size).toBe(1);
});
