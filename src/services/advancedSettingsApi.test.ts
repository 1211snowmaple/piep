import rustSource from "../../src-tauri/src/advanced_settings.rs?raw";
import { describe, expect, it } from "vitest";
import { PREVIEW_ADVANCED_SPECS } from "./advancedSettingsApi";

/**
 * プレビュー用の写しが Rust の表とずれていないか。
 *
 * 項目・既定・範囲は Rust が持つ。プレビューは Rust に届かないので写しを持つが、
 * 片方だけ直すと、画面の確認で見た範囲と本物の範囲が食い違う。
 */
function rustSpecs() {
  const body = rustSource.slice(rustSource.indexOf("settings! {"));
  const pattern = /^\s*\w+ = "(\w+)", "([^"]+)", (\w+), (\w+), ([\d_]+), ([\d_]+), ([\d_]+);/gm;
  const number = (text: string) => Number(text.replace(/_/g, ""));
  const lowerFirst = (text: string) => text[0].toLowerCase() + text.slice(1);
  return [...body.matchAll(pattern)].map(([, key, label, group, unit, value, min, max]) => ({
    key, label, group: lowerFirst(group), unit: lowerFirst(unit), default: number(value), min: number(min), max: number(max),
  }));
}

describe("preview copy of the advanced settings", () => {
  it("matches the table in Rust", () => {
    const rust = rustSpecs();
    expect(rust.length).toBeGreaterThan(10);
    expect(PREVIEW_ADVANCED_SPECS).toEqual(rust);
  });
});
