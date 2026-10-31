import { describe, expect, it } from "vitest";
import { overviewMarks } from "./diffOverview";

describe("overviewMarks", () => {
  it("places edits proportionally on the ruler, one lane per kind", () => {
    const marks = overviewMarks([
      { kind: "removed", top: 1_000, bottom: 1_020 },
      { kind: "added", top: 1_000, bottom: 1_020 },
      { kind: "added", top: 9_000, bottom: 9_200 },
    ], 10_000, 500);

    expect(marks).toEqual([
      { kind: "removed", top: 0.1, height: 2 / 500 },
      { kind: "added", top: 0.1, height: 2 / 500 },
      { kind: "added", top: 0.9, height: 0.02 },
    ]);
  });

  it("merges scattered edits that share a pixel so a long revision stays light", () => {
    const spans = Array.from({ length: 2_000 }, (_, index) => ({ kind: "added" as const, top: index * 50, bottom: index * 50 + 20 }));
    const marks = overviewMarks(spans, 100_000, 400);
    expect(marks).toHaveLength(1);
    expect(marks[0].top).toBe(0);
    expect(marks[0].top + marks[0].height).toBeCloseTo(1, 2);
  });

  it("keeps a change at the very end inside the ruler", () => {
    const [mark] = overviewMarks([{ kind: "removed", top: 9_999, bottom: 10_000 }], 10_000, 400);
    expect(mark.top + mark.height).toBeLessThanOrEqual(1);
    expect(mark.height).toBeGreaterThan(0);
  });

  it("draws nothing before the content has a size", () => {
    expect(overviewMarks([{ kind: "added", top: 0, bottom: 10 }], 0, 0)).toEqual([]);
  });
});
