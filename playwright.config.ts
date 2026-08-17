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
  workers: process.env.CI ? 4 : undefined,
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [["github"], ["html", { outputFolder: "playwright-report", open: "never" }]] : "list",
  expect: {
    timeout: 15_000,
    toHaveScreenshot: {
      animations: "disabled",
      caret: "hide",
      maxDiffPixelRatio: 0.025,
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
    command: "npm run dev -- --host localhost",
    url: "http://localhost:1420",
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
