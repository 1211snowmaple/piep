import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import path from "node:path";

export default defineConfig({
  plugins: [react()],
  resolve: { alias: { "@": path.resolve(import.meta.dirname, "./src"), "@shared": path.resolve(import.meta.dirname, "./shared") } },
  test: {
    // `npm test` は `vitest run src` で絞っているが、この位置引数は道の部分一致
    // なので `docs-tools/src/` の試験まで拾う。あちらは node:test で書かれており、
    // vitest から見ると「試験の無いファイル」として落ちる。走らせるのは
    // `docs:check` のほうであって、ここではない。
    include: ["src/**/*.{test,spec}.{ts,tsx}", "shared/**/*.test.ts"],
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
    // 上げたのは試験ぜんぶの budget で、本当に止まっている試験は、遅れて報されるだけで
    // 見逃されはしない。個々の `findBy*`・`waitFor` の待ちは `src/test/setup.ts` で決める。
    testTimeout: 20_000,
    hookTimeout: 20_000,
    // 子プロセスは論理 CPU の半分まで。既定は「CPU 数 − 1」で、16 の PC だと 15 本になる。
    // 実行役は同じ PC で visual-regression などのジョブも同時に回すので、15 本が一度に
    // jsdom を立ち上げると、vitest が子の起動を待つ固定の 60 秒（設定では変えられない）を
    // 超え、試験は全部通っても「Failed to start forks worker」で落ちた（2026-11-25、
    // main と PR #153 の Quality）。
    maxWorkers: "50%",
  },
});
