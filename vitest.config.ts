import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import path from "node:path";

export default defineConfig({
  plugins: [react()],
  resolve: { alias: { "@": path.resolve(import.meta.dirname, "./src") } },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    environmentOptions: { jsdom: { url: "http://localhost/" } },
    css: true,
    clearMocks: true,
    // 既定の 5 秒は、画面まるごとを描く試験には足りない。
    //
    // CI の runner は3つのジョブを同時に回すので、`LibraryPage` のような
    // 大きな画面を描く試験は、待ち時間ではなく**描画そのもの**で数秒を使う。
    // 手元では 5/5 通るのに CI だけが「5000ms で時間切れ」になっていた。
    //
    // 個々の `waitFor` の待ち時間は変えない。変えるのは試験ぜんぶの budget で、
    // 本当に止まっている試験は、遅れて報されるだけで見逃されはしない。
    testTimeout: 20_000,
    hookTimeout: 20_000,
  },
});
