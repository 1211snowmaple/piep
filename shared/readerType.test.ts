import { describe, expect, it } from "vitest";
import { READER_TYPE_LIMITS, readerTypeFont, readerTypeNumber } from "./readerType";

describe("readerTypeNumber", () => {
  it("keeps values inside the range and on the step", () => {
    expect(readerTypeNumber(36, READER_TYPE_LIMITS.fontSize, 18)).toBe(36);
    expect(readerTypeNumber(40, READER_TYPE_LIMITS.fontSize, 18)).toBe(36);
    expect(readerTypeNumber("12", READER_TYPE_LIMITS.fontSize, 18)).toBe(14);
    expect(readerTypeNumber(1.73, READER_TYPE_LIMITS.lineHeight, 1.72)).toBe(1.74);
    expect(readerTypeNumber(1.4 + 0.02 * 16, READER_TYPE_LIMITS.lineHeight, 1.72)).toBe(1.72);
    expect(readerTypeNumber(531, READER_TYPE_LIMITS.width, 680)).toBe(540);
  });

  it("falls back when nothing usable was remembered", () => {
    for (const value of [undefined, null, "", "大きい", Number.NaN, {}]) {
      expect(readerTypeNumber(value, READER_TYPE_LIMITS.fontSize, 18)).toBe(18);
    }
  });
});

describe("readerTypeFont", () => {
  it("accepts only the offered fonts", () => {
    expect(readerTypeFont("serif", ["sans", "serif"], "sans")).toBe("serif");
    expect(readerTypeFont("ud-serif", ["sans", "serif"], "sans")).toBe("sans");
    expect(readerTypeFont(3, ["sans", "serif"], "sans")).toBe("sans");
  });
});
