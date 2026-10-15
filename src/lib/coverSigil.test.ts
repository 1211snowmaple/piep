import { describe, expect, it } from "vitest";
import { authorHue, coverSigil, sigilCells, titleScale } from "@/lib/coverSigil";

describe("coverSigil", () => {
  it("draws the same tile for the same work every time", () => {
    const work = { source: "fanbox", sourceId: "10920", authorName: "mizu atelier" };
    expect(coverSigil(work)).toEqual(coverSigil({ ...work }));
  });

  it("gives one author one colour and each work its own mark", () => {
    const first = coverSigil({ source: "pixiv", sourceId: "8842013", authorName: "青葉しおり" });
    const second = coverSigil({ source: "pixiv", sourceId: "8851004", authorName: "青葉しおり" });
    expect(second.hue).toBe(first.hue);
    expect(second.cells).not.toEqual(first.cells);
  });

  // Colour groups authors; it does not identify them. Twelve calm steps keep a
  // shelf from turning into a paint chart, so two authors may share one - the
  // mark and the title are what tell their works apart.
  it("spreads authors evenly over the twelve hue steps", () => {
    const names = Array.from({ length: 240 }, (_, index) => `作者${index}`);
    const counts = new Map<number, number>();
    for (const name of names) {
      const hue = authorHue(name);
      expect((hue - 12) % 30).toBe(0);
      counts.set(hue, (counts.get(hue) ?? 0) + 1);
    }
    expect(counts.size).toBe(12);
    // No step may swallow the shelf: even spread is well under a third each.
    expect(Math.max(...counts.values())).toBeLessThan(names.length / 4);
  });

  // The person's name wins over the raw author string so a creator who was
  // resolved to a profile keeps one colour across both of their marks.
  it("keys the colour to the resolved person when there is one", () => {
    const base = { source: "fanbox", sourceId: "1", authorName: "mizu_atelier" };
    expect(coverSigil({ ...base, personName: "mizu atelier" }).hue).toBe(authorHue("mizu atelier"));
    expect(coverSigil(base).hue).toBe(authorHue("mizu_atelier"));
  });

  it("mirrors the mark so it reads as a shape, not as noise", () => {
    const cells = sigilCells("pixiv:8842013");
    expect(cells).toHaveLength(25);
    for (let row = 0; row < 5; row += 1) {
      expect(cells[row * 5]).toBe(cells[row * 5 + 4]);
      expect(cells[row * 5 + 1]).toBe(cells[row * 5 + 3]);
    }
  });

  it("steps the title down instead of clipping it", () => {
    expect(titleScale("海へ")).toBe("lg");
    expect(titleScale("制作ノート #25 — 線を減らす")).toBe("md");
    expect(titleScale("星を編む人 第十四話 まだ見ぬ港と、そこで交わした約束の話")).toBe("sm");
  });
});
