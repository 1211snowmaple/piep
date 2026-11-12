import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import path from "node:path";

export default defineConfig({
  plugins: [react()],
  // vite.config.ts と同じ。shared/ から読む依存も、携帯に入れたものを使う。
  resolve: {
    alias: {
      "@shared": path.resolve(import.meta.dirname, "../shared"),
      "lucide-react": path.resolve(import.meta.dirname, "node_modules/lucide-react"),
    },
  },
  test: {
    include: ["src/**/*.test.{ts,tsx}"],
    environment: "jsdom",
    clearMocks: true,
  },
});
