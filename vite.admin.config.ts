import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import path from "node:path";

// 管理画面。出力は piep-server の実行ファイルへ埋め込む（src-tauri/build.rs）。
// 外部の資産を読まない。フォントも画像も、この出力の中だけで完結させる。
export default defineConfig({
  plugins: [react()],
  root: path.resolve(import.meta.dirname, "src/admin"),
  base: "/admin/",
  resolve: {
    alias: {
      "@": path.resolve(import.meta.dirname, "./src"),
      "@shared": path.resolve(import.meta.dirname, "./shared"),
    },
  },
  build: {
    outDir: path.resolve(import.meta.dirname, "src-tauri/admin-dist"),
    emptyOutDir: true,
    // data: の URL にしない。CSP を緩めずに済ませるため。
    assetsInlineLimit: 0,
    modulePreload: { polyfill: false },
    chunkSizeWarningLimit: 1500,
  },
  server: {
    port: 1432,
    strictPort: true,
  },
});
