import { describe, expect, it } from "vitest";
import { externalBrand, getProvider, providers, sourceUrl } from "@/lib/providers";
import { cleanProfileLink, profileLinkList } from "@/features/library/EntityPage";

describe("service marks", () => {
  it("gives a provider one identity wherever its link appears", () => {
    // FANBOX once had one colour on the save screen and another on a profile
    // link, so the same service looked like two.
    for (const id of ["pixiv", "fanbox"]) {
      const provider = getProvider(id);
      const brand = externalBrand(`https://example.${id === "pixiv" ? "pixiv.net" : "fanbox.cc"}/whatever`);
      expect(brand.provider, id).toBe(id);
      expect(brand.label, id).toBe(provider.label);
      expect(brand.color, id).toBe(provider.color);
    }
  });

  it("recognises a provider on its bare host as well as a subdomain", () => {
    expect(externalBrand("https://www.pixiv.net/users/1").provider).toBe("pixiv");
    expect(externalBrand("https://creator.fanbox.cc/posts/1").provider).toBe("fanbox");
  });

  it("names the services that are only ever links", () => {
    expect(externalBrand("https://x.com/name").label).toBe("X");
    expect(externalBrand("https://twitter.com/name").label).toBe("X");
    expect(externalBrand("https://skeb.jp/@name").label).toBe("Skeb");
    expect(externalBrand("https://peing.net/ja/name").label).toBe("peing.net");
  });

  it("falls back to the host rather than inventing a brand", () => {
    expect(externalBrand("https://example.com/x").label).toBe("example.com");
    expect(externalBrand("not a url").label).toBe("Web");
  });

  it("gives every provider the fields a mark needs", () => {
    for (const [id, provider] of Object.entries(providers)) {
      expect(provider.label, id).toBeTruthy();
      expect(provider.color, id).toMatch(/^#[0-9a-f]{6}$/i);
      expect(provider.shortLabel, id).toBeTruthy();
    }
  });
});

describe("provider registry", () => {
  it("keeps researched provider colors and capabilities", () => {
    expect(getProvider("pixiv").color).toBe("#0096FA");
    expect(getProvider("fanbox").capability).toBe("available");
    expect(getProvider("fanbox").label).toBe("FANBOX");
    expect(getProvider("skeb").description).toBe("外部ソース");
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

describe("profile links", () => {
  it("cuts the prose a link was written inside", () => {
    // Profiles write links inline, and the extractor keeps going to the next
    // space: `twitter【https://twitter.com/name】※進捗報告` came out as one URL.
    expect(cleanProfileLink("https://twitter.com/trysepss】※小説の進捗報告や個人的なこと"))
      .toBe("https://twitter.com/trysepss");
    expect(cleanProfileLink("https://peing.net/ja/trysepss】※匿名で直接質問できます"))
      .toBe("https://peing.net/ja/trysepss");
    expect(cleanProfileLink("https://www.pixiv.net/users/8056363】")).toBe("https://www.pixiv.net/users/8056363");
  });

  it("rejects anything that is not a link", () => {
    expect(cleanProfileLink("小説の進捗報告")).toBeNull();
    expect(cleanProfileLink("javascript:alert(1)")).toBeNull();
    expect(cleanProfileLink("")).toBeNull();
  });

  it("keeps one entry per account, however it was written", () => {
    const links = profileLinkList([
      "https://www.pixiv.net/users/8056363",
      "https://www.pixiv.net/users/8056363】",
      "https://twitter.com/trysepss",
      "https://twitter.com/trysepss】※小説の進捗報告",
      "https://skeb.jp/@trysepss】※リクエスト小説のメイン募集サイト",
    ], "https://www.pixiv.net/users/8056363");

    // The clean form wins; the tailed copies collapse into it.
    expect(links).toEqual([
      "https://www.pixiv.net/users/8056363",
      "https://twitter.com/trysepss",
      "https://skeb.jp/@trysepss",
    ]);
  });

  it("treats a trailing slash and a www prefix as the same account", () => {
    const links = profileLinkList([
      "https://skeb.jp/@name/",
      "https://www.skeb.jp/@name",
    ], null);
    expect(links).toHaveLength(1);
  });
});
