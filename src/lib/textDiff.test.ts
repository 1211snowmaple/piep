import { describe, expect, it } from "vitest";
import { compactTextDiff, createTextDiff } from "./textDiff";

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
