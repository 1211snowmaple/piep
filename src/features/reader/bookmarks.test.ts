import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { useBookmarks } from "./bookmarks";

const STORAGE_KEY = "piep.bookmarks.v1";
const values = new Map<string, string>();

Object.defineProperty(globalThis, "localStorage", {
  configurable: true,
  value: {
    clear: () => values.clear(),
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
    removeItem: (key: string) => values.delete(key),
  },
});

describe("reader bookmarks", () => {
  beforeEach(() => localStorage.clear());

  it("ignores corrupt work buckets and bookmark rows", () => {
    localStorage.setItem(STORAGE_KEY, JSON.stringify({
      101: "not-an-array",
      102: [null, { id: "", page: 0, top: -1, label: 42 }],
    }));
    const { result } = renderHook(() => useBookmarks(101));
    expect(result.current.bookmarks).toEqual([]);
    expect(() => act(() => result.current.add({ page: 1, top: 0, label: "先頭" }))).not.toThrow();
    expect(result.current.bookmarks).toHaveLength(1);
  });

  it("keeps valid rows while dropping corrupt siblings", () => {
    const valid = { id: "bookmark-1", page: 2, top: 120, label: "2ページ · 20%", createdAt: "2026-08-09T00:00:00.000Z" };
    localStorage.setItem(STORAGE_KEY, JSON.stringify({ 101: [valid, { id: 2 }] }));
    const { result } = renderHook(() => useBookmarks(101));
    expect(result.current.bookmarks).toEqual([valid]);
  });
});
