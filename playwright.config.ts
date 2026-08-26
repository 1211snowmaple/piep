import { defineConfig, devices } from "@playwright/test";

const sizes = [
  { name: "900x600", width: 900, height: 600 },
  { name: "1200x800", width: 1200, height: 800 },
  { name: "1440x900", width: 1440, height: 900 },
] as const;
const themes = ["light", "dark"] as const;
const scales = [1, 1.5, 2] as const;

export default defineConfig({
  testDir: "./e2e",
  outputDir: "./test-results/playwright",
  snapshotPathTemplate: "{testDir}/__screenshots__/{projectName}/{arg}{ext}",
  fullyParallel: true,
  // Four browsers at 200dpi on a two-core runner exhausted it: sessions were
  // closed mid-test and pages that render instantly here never appeared.
  workers: process.env.CI ? 2 : undefined,
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 2 : 0,
  // Each test opens the library, waits up to 15s for it to render, then compares
  // a full-page screenshot - at 200dpi on a two-core runner that does not fit in
  // the default 30s, and the ones that overran were failing on the timeout
  // rather than on anything they measured.
  timeout: process.env.CI ? 90_000 : 30_000,
  reporter: process.env.CI ? [["github"], ["html", { outputFolder: "playwright-report", open: "never" }]] : "list",
  expect: {
    // The app splits its routes, so opening one waits on a chunk. On the CI
    // runner the static server shares two cores with the browsers rendering at
    // 200dpi, and that request has been seen to take longer than the fifteen
    // seconds a developer machine never needs.
    timeout: process.env.CI ? 45_000 : 15_000,
    toHaveScreenshot: {
      animations: "disabled",
      caret: "hide",
      // A 2.5% allowance let an entire light-background detail panel vanish:
      // its borders and text occupied fewer pixels than the threshold. Keep a
      // small anti-aliasing allowance, but make structural omissions fail.
      maxDiffPixelRatio: 0.005,
    },
  },
  use: {
    ...devices["Desktop Chrome"],
    // Match Vite/Tauri's default localhost binding. On Windows it commonly
    // listens on ::1; reusing that healthy server through 127.0.0.1 made the
    // visual suite fail intermittently with connection-refused chunk loads.
    baseURL: "http://localhost:1420",
    locale: "ja-JP",
    timezoneId: "Asia/Tokyo",
    reducedMotion: "reduce",
    trace: "retain-on-failure",
  },
  projects: sizes.flatMap((size) => themes.flatMap((colorScheme) => scales.map((deviceScaleFactor) => ({
    name: `${size.name}-${colorScheme}-${Math.round(deviceScaleFactor * 100)}dpi`,
    use: { viewport: { width: size.width, height: size.height }, colorScheme, deviceScaleFactor },
  })))),
  webServer: {
    // A built bundle in CI rather than the dev server. The dev server compiles
    // each module on request, and with several browsers pulling at once on a
    // small runner those requests stalled long enough for pages to come up
    // empty. Serving static files removes that; locally the dev server stays,
    // so a run still picks up edits without a build.
    command: process.env.CI
      ? "npm run preview -- --host localhost --port 1420 --strictPort"
      : "npm run dev -- --host localhost",
    url: "http://localhost:1420",
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
