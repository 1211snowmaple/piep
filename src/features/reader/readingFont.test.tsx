import { act, render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { READER_FONTS } from "@shared/readerType";
import { READER_SETTINGS_KEY, READING_FONT_VARIABLE, readingFontFamily, useReadingFontVariable } from "./readingFont";

function Probe() {
  useReadingFontVariable();
  return null;
}

const variable = () => document.documentElement.style.getPropertyValue(READING_FONT_VARIABLE);

describe("reading font", () => {
  it("uses the font chosen on the reader, and the reader's default when nothing is kept", () => {
    expect(readingFontFamily({ font: "ud-serif" })).toBe(READER_FONTS["ud-serif"].family);
    expect(readingFontFamily({ font: "nope" })).toBe(READER_FONTS.sans.family);
    expect(readingFontFamily(null)).toBe(READER_FONTS.sans.family);
  });

  it("puts the reader's font where the other screens that show the body read it, and follows changes", () => {
    window.localStorage.setItem(READER_SETTINGS_KEY, JSON.stringify({ font: "serif" }));
    render(<Probe />);
    expect(variable()).toBe(READER_FONTS.serif.family);
    act(() => {
      window.dispatchEvent(new CustomEvent("mantine-local-storage", { detail: { key: READER_SETTINGS_KEY, value: { font: "ud-sans" } } }));
    });
    expect(variable()).toBe(READER_FONTS["ud-sans"].family);
  });
});
