import { describe, expect, it } from "vitest";
import { readVisualSettings, writeVisualSettings } from "./EpubPage";

const settings = {
  fontFamily: "sans" as const,
  fontSize: 18,
  lineHeight: 2,
  pagePadding: 24,
  textColor: "#222222",
  backgroundColor: "#ffffff",
  accentColor: "#0096fa",
  coverWidth: 64,
  coverRadius: 12,
  titleAlign: "center" as const,
};

describe("EPUB visual template settings", () => {
  it("writes a managed CSS override without destroying template code", () => {
    const result = writeVisualSettings("{% include \"_base_style.css.j2\" %}\n.custom { display: block; }", settings);
    expect(result).toContain("{% include \"_base_style.css.j2\" %}");
    expect(result).toContain("font-size: 18px");
    expect(result).toContain("text-align: center");
    expect(result).toContain("max-width: 64%");
  });

  it("replaces the previous managed block instead of duplicating it", () => {
    const first = writeVisualSettings("body {}", settings);
    const second = writeVisualSettings(first, { ...settings, fontSize: 20 });
    expect(second.match(/piep-visual:start/g)).toHaveLength(1);
    expect(second).toContain("font-size: 20px");
  });

  it("round-trips body, accent and cover values independently", () => {
    const result = readVisualSettings(writeVisualSettings(":root { --tag-color: #ff0000; }", settings));
    expect(result).toEqual(settings);
  });
});
