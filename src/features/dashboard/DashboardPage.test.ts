import { describe, expect, it } from "vitest";
import { quickSaveSource, searchIndexNote } from "@/features/dashboard/DashboardPage";

describe("quickSaveSource", () => {
  it("accepts provider hosts and their subdomains", () => {
    expect(quickSaveSource("https://www.pixiv.net/novel/show.php?id=123")).toBe("pixiv");
    expect(quickSaveSource("https://creator.fanbox.cc/posts/123")).toBe("fanbox");
  });

  it("does not infer a provider from a path, query, or lookalike hostname", () => {
    expect(quickSaveSource("https://evil.example/?next=fanbox.cc")).toBeNull();
    expect(quickSaveSource("https://pixiv.net.evil.example/novel/123")).toBeNull();
    expect(quickSaveSource("https://notpixiv.net/novel/123")).toBeNull();
  });

  it("rejects malformed and non-http URLs", () => {
    expect(quickSaveSource("not a url")).toBeNull();
    expect(quickSaveSource("javascript:alert(1)")).toBeNull();
  });
});

describe("searchIndexNote", () => {
  const complete = { pendingDownloads: 0, isComplete: true, semanticIndexedChunks: 4192, semanticPendingDownloads: 0 };

  it("reports the full-text backlog while it is still catching up", () => {
    expect(searchIndexNote({ ...complete, pendingDownloads: 42, isComplete: false })).toBe("42件を処理中");
  });

  it("does not claim semantic search is current when it is behind", () => {
    const note = searchIndexNote({ ...complete, semanticPendingDownloads: 318 });
    expect(note).toContain("全文検索は最新です");
    expect(note).toContain("318件が未反映");
  });

  it("says nothing about semantic search on a shelf that has never built it", () => {
    expect(searchIndexNote({ ...complete, semanticIndexedChunks: 0, semanticPendingDownloads: 1284 })).toBe("全文検索は最新です");
  });

  it("claims both only when both are caught up", () => {
    expect(searchIndexNote(complete)).toBe("全文・意味検索は最新です");
  });
});
