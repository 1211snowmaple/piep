import { describe, expect, it } from "vitest";
import { formatReadingTime, readingMinutes } from "./ReaderNextUp";

describe("reading time", () => {
  it("never says zero minutes for a short piece", () => {
    expect(readingMinutes(0)).toBe(1);
    expect(formatReadingTime(120)).toBe("約1分");
  });

  it("switches to hours so a long novel reads as an amount", () => {
    expect(formatReadingTime(11_000)).toBe("約20分");
    expect(formatReadingTime(550 * 60)).toBe("約1時間");
    expect(formatReadingTime(550 * 80)).toBe("約1時間20分");
  });
});
