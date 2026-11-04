import { describe, expect, it } from "vitest";

const stylesheets = import.meta.glob<string>("/src/**/*.css", { query: "?raw", import: "default", eager: true });
const components = import.meta.glob<string>(["/src/**/*.tsx", "!/src/**/*.test.tsx"], { query: "?raw", import: "default", eager: true });

interface Rule { selectors: string[]; body: string }

function rules(css: string): Rule[] {
  const plain = css.replace(/\/\*[\s\S]*?\*\//g, "");
  return [...plain.matchAll(/([^{}]+)\{([^{}]*)\}/g)].map((match) => ({
    selectors: match[1].split(",").map((selector) => selector.trim()),
    body: match[2],
  }));
}

const allRules = Object.values(stylesheets).flatMap(rules);
const reserving = new Set(allRules
  .filter((rule) => /scrollbar-gutter:\s*stable|scrollbar-width:\s*none/.test(rule.body))
  .flatMap((rule) => rule.selectors));

describe("スクロールバーで中身の幅を動かさない", () => {
  it("読み込む範囲に、スタイルと画面のコードがある", () => {
    expect(allRules.length).toBeGreaterThan(100);
    expect(Object.keys(components).length).toBeGreaterThan(50);
  });

  it("縦にスクロールする領域は、すべて場所を取っている", () => {
    const missing = allRules
      .filter((rule) => /overflow(-y|-block)?:\s*(auto|scroll)/.test(rule.body))
      .flatMap((rule) => rule.selectors)
      .filter((selector) => !reserving.has(selector));
    expect(missing).toEqual([]);
  });

  it("横にスクロールする領域は、バーの出入りで高さを変えない", () => {
    const moving = allRules
      .filter((rule) => /overflow-(x|inline):\s*auto/.test(rule.body))
      .flatMap((rule) => rule.selectors)
      .filter((selector) => !reserving.has(selector));
    expect(moving).toEqual([]);
  });

  it("画面のコードにスクロールの指定を直接書かない", () => {
    const inline = Object.entries(components)
      .filter(([, source]) => /overflow[XY]?\s*:\s*["'](auto|scroll)["']/.test(source))
      .map(([path]) => path);
    expect(inline).toEqual([]);
  });
});
