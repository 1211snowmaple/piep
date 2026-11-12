import { describe, expect, it } from "vitest";
import css from "./tokens.css?raw";
import { FONT_FAMILY, leaf, piep } from "./tokens";

function value(name: string): string | undefined {
  return new RegExp(`${name}: ([^;]+);`).exec(css)?.[1];
}

describe("見た目の土台", () => {
  it("CSS と JavaScript が同じ色の段階と書体を持つ", () => {
    // Mantine のテーマは JavaScript の値から、携帯と素の CSS は変数から色を引く。
    // 片方だけ直すと、デスクトップと携帯で青が変わる。
    piep.forEach((color, index) => expect(value(`--piep-blue-${index}`), `piep ${index}`).toBe(color));
    leaf.forEach((color, index) => expect(value(`--piep-leaf-${index}`), `leaf ${index}`).toBe(color));
    expect(value("--piep-font")).toBe(FONT_FAMILY);
  });

  it("暗い配色の二つの書き方が同じ値を持ち、明るい配色の全部を置き換える", () => {
    const blocks = [...css.matchAll(/\{([^{}]*)\}/g)].map((match) => match[1]);
    const names = (block: string) => [...block.matchAll(/(--piep-[a-z-]+): ([^;]+);/g)].map((match) => `${match[1]}=${match[2]}`);
    const [light, chosen, followsDevice] = blocks.map(names);
    expect(chosen.length).toBeGreaterThan(10);
    expect(followsDevice).toEqual(chosen);
    const semantic = light.map((entry) => entry.split("=")[0]).filter((name) => !/-\d$/.test(name) && name !== "--piep-font");
    expect(chosen.map((entry) => entry.split("=")[0])).toEqual(semantic);
  });

  it("根の要素には当たらない light-dark() を使わない", () => {
    expect(css.replace(/\/\*[\s\S]*?\*\//g, "")).not.toContain("light-dark(");
  });
});
