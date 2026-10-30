import { expect, test } from "@playwright/test";

const once = (name: string) => name === "1200x800-light-100dpi";

test("image preview keeps its readable action visible inside all four rounded corners",async({page},info)=>{
  test.skip(!info.project.name.endsWith("100dpi"),"The viewport and theme matrix covers this dialog");
  await page.goto("/#/works/108?tab=assets");
  await page.getByAltText("preview-sample.svg").click();
  const dialog=page.getByRole("dialog",{name:"preview-sample.svg"});
  const open=dialog.getByRole("button",{name:"画像を開く",exact:true});
  await expect(open).toBeVisible();
  await expect(dialog).toHaveCSS("opacity","1");
  const geometry=await open.evaluate(n=>({height:n.getBoundingClientRect().height,bottom:n.getBoundingClientRect().bottom,font:parseFloat(getComputedStyle(n).fontSize),viewport:innerHeight}));
  expect(geometry.height).toBeGreaterThanOrEqual(36);
  expect(geometry.font).toBeGreaterThanOrEqual(12);
  expect(geometry.bottom).toBeLessThanOrEqual(geometry.viewport-10);
  const spacing=await dialog.evaluate(n=>{
    const box=n.getBoundingClientRect();
    const close=n.querySelector(".mantine-Modal-close")!.getBoundingClientRect();
    const title=n.querySelector(".mantine-Modal-title")!.getBoundingClientRect();
    const action=n.querySelector(".asset-preview__footer button")!.getBoundingClientRect();
    const metadata=n.querySelector(".asset-preview__footer p")!.getBoundingClientRect();
    return{top:close.top-box.top,bottom:box.bottom-action.bottom,right:close.right-action.right,left:title.left-metadata.left};
  });
  expect(spacing.top).toBe(24);
  expect(spacing.bottom).toBe(24);
  expect(Math.abs(spacing.right)).toBeLessThan(1);
  expect(Math.abs(spacing.left)).toBeLessThan(1);
  await expect(dialog).toHaveCSS("overflow","hidden");
  await expect(dialog).toHaveCSS("border-top-right-radius",await dialog.evaluate(n=>getComputedStyle(n).borderTopLeftRadius));
});

test("search methods remain clickable above global search and preserve keyboard operation",async({page},info)=>{
  test.skip(!once(info.project.name),"Interaction is shared");
  await page.goto("/#/library");
  await page.keyboard.press("Control+k");
  const dialog=page.getByRole("dialog",{name:"検索と画面移動"});
  await dialog.getByRole("button",{name:"検索方法：標準検索",exact:true}).click();
  await page.getByRole("menuitem",{name:/キーワード検索/}).click();
  await expect(dialog.getByRole("button",{name:"検索方法：キーワード検索",exact:true})).toBeVisible();
  await dialog.getByRole("textbox").fill("の");
  const work=dialog.locator(".global-search__work").first();
  await expect(work).toBeVisible();
  expect((await work.boundingBox())!.height).toBe(56);
  await expect(work.getByLabel("pixiv")).toBeVisible();
  const alignment=await dialog.locator(".global-search__work-meta").evaluateAll(nodes=>nodes.map(n=>[...n.children].slice(1).map(c=>Math.round(c.getBoundingClientRect().x))));
  expect(alignment.length).toBeGreaterThan(1);
  expect(alignment.every(row=>row.every((x,index)=>x===alignment[0][index]))).toBe(true);
  await dialog.getByRole("button",{name:"検索方法：キーワード検索",exact:true}).click();
  await page.getByRole("menuitem",{name:"過去版も含める"}).click();
  await page.keyboard.press("Escape");
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button",{name:"検索結果をすべて表示",exact:true}).click();
  await expect(page).toHaveURL(/intent=exact&versions=all/);
});

test("revision shelf has the same removable filter as attention",async({page},info)=>{
  test.skip(!once(info.project.name),"URL normalization is shared");
  await page.goto("/#/library?revised=1");
  const token=page.locator(".filter-token").filter({hasText:"改稿あり"});
  await expect(token).toBeVisible();
  await token.getByRole("button").click();
  await expect(page).not.toHaveURL(/revised=|revision=/);
});

test("header controls keep transparent backgrounds and the library toolbar stays in one row",async({page})=>{
  await page.goto("/#/library");
  await expect(page.locator(".app-header")).toHaveCSS("height","48px");
  await expect(page.locator(".app-header .lucide-panel-left-open:visible,.app-header .lucide-panel-left-close:visible").first()).toBeVisible();
  await expect(page.locator(".mantine-Burger-root")).toHaveCount(0);
  const back=page.getByRole("button",{name:"前の画面へ戻る",exact:true});
  await expect(back).toHaveCSS("background-color","rgba(0, 0, 0, 0)");
  await back.hover();
  await expect(back).toHaveCSS("background-color","rgba(0, 0, 0, 0)");
  const search=await page.getByRole("combobox",{name:"ライブラリを検索"}).boundingBox();
  const filter=await page.getByRole("button",{name:"絞り込み",exact:true}).boundingBox();
  expect(Math.abs(search!.y+search!.height/2-filter!.y-filter!.height/2)).toBeLessThan(2);
});

test("acquisition conditions remain staged and can be removed before applying", async ({ page }, info) => {
  test.skip(!once(info.project.name), "Interaction is shared; the layout matrix covers other windows");
  await page.goto("/#/library");
  await page.getByRole("button", { name: "絞り込み", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "詳細フィルター" });
  await expect(dialog).toHaveCSS("opacity","1");
  const close=dialog.getByRole("button",{name:"閉じる",exact:true});
  await expect(close).toHaveCSS("width","32px");
  await expect(close).toHaveCSS("height","32px");
  expect(await dialog.evaluate(n=>{
    const edge=n.getBoundingClientRect();
    const closeBox=n.querySelector(".mantine-Modal-close")!.getBoundingClientRect();
    const action=[...n.querySelectorAll("button")].find(b=>b.textContent==="適用")!.getBoundingClientRect();
    return {top:closeBox.top-edge.top,right:closeBox.right-action.right,bottom:edge.bottom-action.bottom};
  })).toEqual({top:24,right:0,bottom:24});
  await expect(dialog.getByRole("tab", { name: /^保存状態/ })).toHaveAttribute("aria-selected", "true");
  await dialog.getByRole("group", { name: "取得状態", exact:true }).getByRole("button", { name: "一部保存", exact: true }).click();
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
  await expect(page.getByRole("group", { name: "取得状態", exact: true })).toBeVisible();
  await expect(page).toHaveScreenshot("ux-filter-state.png");
  await page.getByRole("button", { name: "閉じる", exact: true }).click();
  await page.getByRole("button", { name: "検索または移動", exact: true }).click();
  await page.getByRole("textbox", { name: "ライブラリ全体を検索", exact: true }).fill("雨上がり");
  await expect(page.getByRole("dialog").getByRole("button", { name: /雨上がりの図書室で/ })).toBeVisible();
  await expect(page).toHaveScreenshot("ux-library-search.png");
});
