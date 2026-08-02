import { describe, expect, it } from "vitest";
import { externalBrand, getProvider, sourceUrl } from "@/lib/providers";

describe("provider registry", () => {
  it("keeps researched provider colors and capabilities", () => {
    expect(getProvider("pixiv").color).toBe("#0096FA");
    expect(getProvider("fanbox").capability).toBe("available");
    expect(getProvider("fanbox").label).toBe("FANBOX");
    expect(getProvider("skeb").description).toBe("外部ソース");
    expect(externalBrand("https://skeb.jp/@creator").label).toBe("Skeb");
    expect(externalBrand("https://x.com/creator").label).toBe("X");
  });

  it("constructs the correct source URL for every entity kind", () => {
    expect(sourceUrl("pixiv", "42")).toBe("https://www.pixiv.net/novel/show.php?id=42");
    expect(sourceUrl("pixiv", "42", "person")).toBe("https://www.pixiv.net/users/42");
    expect(sourceUrl("pixiv", "42", "series")).toBe("https://www.pixiv.net/novel/series/42");
    expect(sourceUrl("fanbox", "creator", "person")).toBe("https://www.fanbox.cc/@creator");
    expect(sourceUrl("fanbox", "100", "article", "creator")).toBe("https://www.fanbox.cc/@creator/posts/100");
    expect(sourceUrl("fanbox", "100")).toBe("https://www.fanbox.cc/posts/100");
  });
});
