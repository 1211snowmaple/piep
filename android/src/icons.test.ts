import { describe, expect, it } from "vitest";

/** 画面のソースを、束ねる仕組みを通して読む。 */
const sources = import.meta.glob("/src/**/*.{ts,tsx}", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

describe("アイコンの対応表", () => {
  it("画面は lucide-react を直接読まず、共通の対応表の役割で引く", () => {
    // 同じ意味を画面ごとに別の絵で描かないため。対応表は shared/icons.ts にある。
    const offenders = Object.keys(sources)
      .filter((path) => !path.endsWith("icons.test.ts"))
      .filter((path) => sources[path].includes('from "lucide-react"'));
    expect(offenders).toEqual([]);
  });
});
