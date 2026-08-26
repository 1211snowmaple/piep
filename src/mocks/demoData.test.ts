import { describe, expect, it } from "vitest";
import { getDemoCollection, getDemoWork } from "./demoData";

describe("demo entity lookup", () => {
  it("does not substitute an unrelated first record for an unknown id", () => {
    expect(() => getDemoWork(999_999)).toThrow("作品が見つかりません");
    expect(() => getDemoCollection("missing")).toThrow("コレクションが見つかりません");
  });
});
