import { describe, expect, it } from "vitest";
import { compactTextDiff, createTextDiff, sideBySideRows, type SideBySideRow } from "./textDiff";

describe("createTextDiff", () => {
  it("reconstructs both Japanese versions and counts added and removed characters", () => {
    const before = "第一段落。古い言葉です。\n最後の段落。";
    const after = "第一段落。新しい言葉です。\n追記です。\n最後の段落。";
    const diff = createTextDiff(before, after);

    expect(diff.parts.filter((part) => part.kind !== "added").map((part) => part.value).join("")).toBe(before);
    expect(diff.parts.filter((part) => part.kind !== "removed").map((part) => part.value).join("")).toBe(after);
    expect(diff.changed).toBe(true);
    expect(diff.addedCharacters).toBeGreaterThan(0);
    expect(diff.removedCharacters).toBeGreaterThan(0);
  });

  it("keeps a revision swept across every paragraph as local edits", () => {
    // Thousands of small edits spread over the work exceed one global edit
    // budget; they must not collapse into "the whole work was replaced".
    const paragraphs = Array.from({ length: 3_000 }, (_, index) => `第${index}段落、彼女は振り返って……そう言った。`);
    const before = paragraphs.join("\n\n");
    const after = paragraphs.map((line) => line.replace("……", "......")).join("\n\n");
    const diff = createTextDiff(before, after);

    expect(diff.granularity).toBe("word");
    expect(diff.removedCharacters).toBe(3_000 * 2);
    expect(diff.addedCharacters).toBe(3_000 * 6);
    expect(diff.parts.filter((part) => part.kind !== "added").map((part) => part.value).join("")).toBe(before);
    expect(diff.parts.filter((part) => part.kind !== "removed").map((part) => part.value).join("")).toBe(after);
  });

  it("refines every paragraph on its own when no unchanged line separates them", () => {
    const paragraph = "雨上がりの図書室で、彼女は振り返った。".repeat(20);
    const before = Array.from({ length: 200 }, () => paragraph).join("\n");
    const after = Array.from({ length: 200 }, () => paragraph.replace(/、/g, "......")).join("\n");
    const diff = createTextDiff(before, after);

    expect(diff.granularity).toBe("word");
    expect(diff.removedCharacters).toBe(200 * 20);
    expect(diff.parts.filter((part) => part.kind !== "removed").map((part) => part.value).join("")).toBe(after);
  });

  it("refines edits that join or split paragraphs", () => {
    const before = "最初の段落。\n二つ目の段落。\n三つ目の段落。";
    const after = "最初の段落。二つ目の段落。\n三つ目の段落。";
    const diff = createTextDiff(before, after);
    expect(diff).toMatchObject({ granularity: "word", addedCharacters: 0, removedCharacters: 1 });
  });

  it("falls back to a range replacement for a wholly rewritten long work", () => {
    const before = Array.from({ length: 4_000 }, (_, index) => `旧${index}`).join("。");
    const after = Array.from({ length: 4_000 }, (_, index) => `新${index}`).join("、");
    const diff = createTextDiff(before, after);

    expect(diff.granularity).toBe("range");
    expect(diff.parts.filter((part) => part.kind !== "added").map((part) => part.value).join("")).toBe(before);
    expect(diff.parts.filter((part) => part.kind !== "removed").map((part) => part.value).join("")).toBe(after);
  });

  it("reports identical text without a false revision", () => {
    const diff = createTextDiff("同じ本文", "同じ本文");
    expect(diff).toMatchObject({ changed: false, addedCharacters: 0, removedCharacters: 0 });
    expect(diff.parts).toEqual([{ kind: "equal", value: "同じ本文" }]);
  });

  it("collapses distant unchanged prose while retaining context around changes", () => {
    const compact = compactTextDiff([
      { kind: "equal", value: "前".repeat(500) },
      { kind: "removed", value: "旧" },
      { kind: "added", value: "新" },
      { kind: "equal", value: "後".repeat(500) },
    ], 20);

    expect(compact.some((part) => part.kind === "omitted")).toBe(true);
    expect(compact.find((part) => part.kind === "removed")?.value).toBe("旧");
    expect(compact.find((part) => part.kind === "added")?.value).toBe("新");
  });
});

describe("sideBySideRows", () => {
  const text = (row: SideBySideRow, side: "before" | "after") => row.kind === "text" ? row[side].map((cell) => cell.value).join("") : null;

  it("両方の版に共通する改行で行を切り、左右の段落を揃える", () => {
    const rows = sideBySideRows([
      { kind: "equal", value: "同じ\nここから" },
      { kind: "removed", value: "──" },
      { kind: "added", value: "……" },
      { kind: "equal", value: "続き\n\n最後" },
    ]);
    expect(rows.map((row) => text(row, "before"))).toEqual(["同じ", "ここから──続き", "", "最後"]);
    expect(rows.map((row) => text(row, "after"))).toEqual(["同じ", "ここから……続き", "", "最後"]);
  });

  it("追加の中の改行では行を切らず、その列の中で折り返す", () => {
    const rows = sideBySideRows([
      { kind: "equal", value: "前\n" },
      { kind: "added", value: "一行目\n二行目\n" },
      { kind: "equal", value: "後" },
    ]);
    expect(rows.map((row) => text(row, "before"))).toEqual(["前", "後"]);
    expect(rows.map((row) => text(row, "after"))).toEqual(["前", "一行目\n二行目\n後"]);
  });

  it("行の先頭に来た改行では、共通の本文を片側だけ押し下げない", () => {
    const rows = sideBySideRows([
      { kind: "equal", value: "前\n" },
      { kind: "added", value: "\n" },
      { kind: "equal", value: "後" },
    ]);
    expect(rows.map((row) => text(row, "before"))).toEqual(["前", "", "後"]);
    expect(rows.map((row) => text(row, "after"))).toEqual(["前", "", "後"]);
    expect(rows[2]).toMatchObject({ before: [{ kind: "equal" }], after: [{ kind: "equal" }] });
  });

  it("省略は左右にまたがる1行にする", () => {
    const rows = sideBySideRows([
      { kind: "omitted", value: "… 100字省略 …", characters: 100 },
      { kind: "removed", value: "古い" },
    ]);
    expect(rows[0]).toEqual({ kind: "omitted", value: "… 100字省略 …" });
    expect(text(rows[1], "before")).toBe("古い");
    expect(text(rows[1], "after")).toBe("");
  });

  it("変更の目印に元の parts の位置を残す", () => {
    const rows = sideBySideRows([{ kind: "equal", value: "a" }, { kind: "removed", value: "b" }, { kind: "added", value: "c" }]);
    expect(rows[0]).toMatchObject({ before: [{ index: 0 }, { index: 1 }], after: [{ index: 0 }, { index: 2 }] });
  });
});
