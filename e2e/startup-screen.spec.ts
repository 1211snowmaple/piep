import { expect, test } from "@playwright/test";

const backgrounds = { light: "rgb(245, 247, 250)", dark: "rgb(13, 17, 23)" } as const;
const opposite = { light: "dark", dark: "light" } as const;

test.skip(
  ({ viewport, deviceScaleFactor }) => viewport?.width !== 1200 || deviceScaleFactor !== 1,
  "The startup screen is centred plain markup; one window size in each theme covers it",
);

/** The app's script never runs, which is how the screen looks while the library opens. */
async function holdAtStartupScreen(page: import("@playwright/test").Page) {
  await page.route(/\/src\/main\.tsx|\/assets\/index-[^/]*\.js/, (route) => route.abort());
  await page.goto("/");
  await expect(page.getByRole("status")).toContainText("ライブラリを準備しています");
}

test("the startup screen says the app is getting ready, in the theme's colour", async ({ page }, testInfo) => {
  const theme = testInfo.project.use.colorScheme as "light" | "dark";
  await holdAtStartupScreen(page);
  await expect(page.locator(".startup-splash")).toHaveCSS("background-color", backgrounds[theme]);
  await expect(page).toHaveScreenshot("startup-screen.png");
});

test("a theme chosen in the app wins over the system's, before the app has loaded", async ({ page }, testInfo) => {
  const theme = opposite[testInfo.project.use.colorScheme as "light" | "dark"];
  await page.addInitScript((chosen) => localStorage.setItem("mantine-color-scheme-value", chosen), theme);
  await holdAtStartupScreen(page);
  await expect(page.locator(".startup-splash")).toHaveCSS("background-color", backgrounds[theme]);
});

test("the app takes over from the startup screen without changing the background", async ({ page }, testInfo) => {
  const theme = testInfo.project.use.colorScheme as "light" | "dark";
  await page.goto("/#/library");
  await expect(page.getByRole("combobox", { name: "ライブラリを検索" })).toBeVisible();
  await expect(page.locator(".startup-splash")).toHaveCount(0);
  await expect(page.locator("body")).toHaveCSS("background-color", backgrounds[theme]);
});
