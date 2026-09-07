import { expect, test } from "@playwright/test";

test("save notifications leave editor and reader actions accessible", async ({ page }, info) => {
  test.skip(!["900x600-light-100dpi", "1440x900-light-100dpi"].includes(info.project.name));
  await page.goto("/#/editor/101");
  await page.getByRole("textbox", { name: "この作品のタイトル" }).fill("通知の表示中も反映できる作品");
  await page.getByRole("button", { name: "下書き保存", exact: true }).click();
  const notice = page.getByRole("alert").filter({ hasText: "下書きを保存しました" });
  await expect(notice).toBeVisible();
  const toolbar = await page.locator(".editor-toolbar").boundingBox();
  const notification = await notice.boundingBox();
  expect(toolbar).not.toBeNull();
  expect(notification).not.toBeNull();
  expect(notification!.y).toBeGreaterThanOrEqual(toolbar!.y + toolbar!.height);

  await page.getByRole("button", { name: "反映", exact: true }).click();
  const publishedNotice = page.getByRole("alert").filter({ hasText: "編集版を反映しました" });
  await expect(publishedNotice).toBeVisible();

  await page.goto("/#/reader/101");
  await expect(page.locator(".reader-toolbar")).toBeVisible();
  await expect(publishedNotice).toBeVisible();
  const readerToolbar = await page.locator(".reader-toolbar").boundingBox();
  const readerNotification = await notice.boundingBox();
  expect(readerToolbar).not.toBeNull();
  expect(readerNotification).not.toBeNull();
  expect(readerNotification!.y).toBeGreaterThanOrEqual(readerToolbar!.y + readerToolbar!.height);
  await page.getByRole("button", { name: "リーダーの表示設定" }).click();
  await expect(page.getByRole("dialog", { name: "読書設定" })).toBeVisible();
});
