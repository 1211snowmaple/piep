import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import path from "node:path";

export default defineConfig({
  plugins: [react()],
  resolve: { alias: { "@shared": path.resolve(import.meta.dirname, "../shared") } },
  test: {
    include: ["src/**/*.test.{ts,tsx}"],
    environment: "jsdom",
    clearMocks: true,
  },
});
