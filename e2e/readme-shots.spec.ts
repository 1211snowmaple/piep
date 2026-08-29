import { test, type Page } from "@playwright/test";

/**
 * Regenerates the screenshots the README embeds.
 *
 * They are shot against the browser preview, so every work, creator and job on
 * them comes from `src/mocks/demoData.ts` and nothing from a real library is
 * ever published. Two projects carry the README: one light, one dark.
 *
 *   npx playwright test readme-shots --project=1440x900-light-200dpi --project=1440x900-dark-200dpi
 */
const projects = ["1440x900-light-200dpi", "1440x900-dark-200dpi"];

type ShotTheme = "light" | "dark";

/** Screen, route, a text that proves it rendered, and the themes it is shot in. */
const shots: ReadonlyArray<
  readonly [name: string, route: string, visibleText: string, themes: readonly ShotTheme[]]
> = [
  ["home", "/#/", "最近の保存", ["light"]],
  ["library", "/#/library", "作品", ["light", "dark"]],
  ["library-people", "/#/library?tab=people", "作者・クリエイター", ["light"]],
  ["work", "/#/works/101", "雨上がりの図書室で", ["light"]],
  ["reader", "/#/reader/101", "雨上がりの図書室で", ["light", "dark"]],
  ["editor", "/#/editor/101", "雨上がりの図書室で", ["light"]],
  ["updates", "/#/updates", "更新センター", ["light"]],
  ["epub", "/#/epub", "EPUB書き出し", ["light"]],
  ["diagnostics", "/#/diagnostics", "ライブラリ診断", ["light"]],
];

test("capture readme screenshots", async ({ page }, testInfo) => {
  test.skip(!projects.includes(testInfo.project.name), "The README is shot at one size, in both themes");
  const theme = testInfo.project.name.includes("dark") ? "dark" : "light";
  await page.addInitScript(() => {
    localStorage.setItem("piep.nav-railed", "false");
    localStorage.setItem("piep.library-view", "gallery");
    localStorage.setItem("piep.epub-queue.v2", JSON.stringify([101, 103, 108]));
    // The browser-preview banner is about the harness, not the product, and it
    // comes back on every re-render, so it stays hidden for as long as we shoot.
    const hideBanner = () => {
      for (const alert of document.querySelectorAll<HTMLElement>("[class*='mantine-Alert-root']")) {
        if (alert.textContent?.includes("ブラウザプレビュー")) alert.style.display = "none";
      }
    };
    document.addEventListener("DOMContentLoaded", () => {
      hideBanner();
      new MutationObserver(hideBanner).observe(document.body, { childList: true, subtree: true });
    });
  });
  for (const [name, route, visibleText, themes] of shots) {
    if (!themes.includes(theme)) continue;
    await page.goto(route);
    await page.getByText(visibleText).first().waitFor({ state: "visible", timeout: 20_000 });
    await page.addStyleTag({ content: "*,*::before,*::after{animation:none!important;transition:none!important;caret-color:transparent!important}" });
    await page.evaluate(async () => { await document.fonts.ready; });
    await page.waitForTimeout(600);
    await writeShot(page, `docs/screenshots/${name}-${theme}.png`);
  }
});

/**
 * Writes one screenshot, retrying a locked file once.
 *
 * The two themes shoot into the same tracked folder while the rest of the
 * suite runs, and on Windows a virus scanner or the search indexer can still
 * hold a file that was written moments ago - which surfaces as an `UNKNOWN`
 * open error and fails a run that had nothing wrong with it. A second attempt
 * settles it; a second failure is still a failure.
 */
async function writeShot(page: Page, path: string) {
  try {
    await page.screenshot({ path });
  } catch (error) {
    if (!String(error).includes("UNKNOWN") && !String(error).includes("EBUSY")) throw error;
    await page.waitForTimeout(750);
    await page.screenshot({ path });
  }
}
