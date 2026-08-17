import { describe, expect, it } from "vitest";
import { readVisualSettings, visualTemplateDefaults, writeVisualSettings, type VisualTemplateValues } from "./visualTemplate";

const settings: VisualTemplateValues = {
  fontFamily: "sans",
  fontSize: 18,
  lineHeight: 2,
  paragraphSpacing: 0.8,
  textIndent: 1,
  pagePadding: 24,
  justify: true,
  verticalWriting: true,
  textColor: "#222222",
  backgroundColor: "#fffdf6",
  accentColor: "#0096fa",
  mutedColor: "#888888",
  titleAlign: "center",
  titleSize: 1.8,
  headingAlign: "center",
  headingSize: 1.4,
  headingRule: false,
  coverWidth: 64,
  coverRadius: 12,
  illustrationWidth: 80,
  rubySize: 0.45,
};

describe("EPUB visual template settings", () => {
  it("writes a managed block without destroying the template code around it", () => {
    const result = writeVisualSettings('{% include "_base_style.css.j2" %}\n.custom { display: block; }', settings);
    expect(result).toContain('{% include "_base_style.css.j2" %}');
    expect(result).toContain(".custom { display: block; }");
    expect(result).toContain("--piep-font-size: 18px");
    expect(result).toContain("--piep-writing-mode: vertical-rl");
  });

  it("replaces the previous managed block instead of stacking copies", () => {
    const first = writeVisualSettings("body {}", settings);
    const second = writeVisualSettings(first, { ...settings, fontSize: 20 });
    expect(second.match(/piep-visual:start/g)).toHaveLength(1);
    expect(second).toContain("--piep-font-size: 20px");
    expect(second).not.toContain("--piep-font-size: 18px");
  });

  it("round-trips every control value", () => {
    expect(readVisualSettings(writeVisualSettings(":root { --tag-color: #ff0000; }", settings))).toEqual(settings);
    expect(readVisualSettings(writeVisualSettings("", visualTemplateDefaults))).toEqual(visualTemplateDefaults);
  });

  it("falls back to the defaults for a stylesheet it has never touched", () => {
    // ここを取り違えると、開いただけのテンプレートが既定値で上書きされる。
    expect(readVisualSettings("body { font-size: 99px; }")).toEqual(visualTemplateDefaults);
  });

  it("keeps a hand edit inside the managed block", () => {
    const edited = writeVisualSettings("body {}", settings).replace("--piep-font-size: 18px", "--piep-font-size: 21px");
    expect(readVisualSettings(edited).fontSize).toBe(21);
  });
});
