import { describe, expect, it } from "vitest";
import { quickSaveSource } from "@/features/dashboard/DashboardPage";

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
