import { describe, expect, it } from "vitest";
import { contentLinkTarget } from "@/lib/contentLinks";

describe("what a link inside saved text names", () => {
  it("reads pixiv's own app scheme, which is what its captions are written with", () => {
    // These carry no protocol a browser accepts, so they used to be stripped
    // entirely - leaving underlined blue text that did nothing when clicked.
    expect(contentLinkTarget("pixiv://novels/20563258")).toEqual({ kind: "work", source: "pixiv", sourceId: "20563258" });
    expect(contentLinkTarget("pixiv://users/8001234")).toEqual({ kind: "person", source: "pixiv", sourceKey: "8001234" });
  });

  it("recognises a series, which is a screen in the app rather than a work", () => {
    expect(contentLinkTarget("https://www.pixiv.net/novel/series/10848521"))
      .toEqual({ kind: "series", source: "pixiv", sourceKey: "10848521" });
  });

  it("reads every shape pixiv writes a novel address in", () => {
    for (const url of [
      "https://www.pixiv.net/novel/show.php?id=111",
      "https://www.pixiv.net/novel/show.php?id=111&mode=cover",
      "https://www.pixiv.net/en/novel/show.php?id=111",
      "https://www.pixiv.net/novels/111",
    ]) {
      expect(contentLinkTarget(url), url).toEqual({ kind: "work", source: "pixiv", sourceId: "111" });
    }
  });

  it("reads a FANBOX post from either address the service uses for it", () => {
    expect(contentLinkTarget("https://rope-less.fanbox.cc/posts/8421"))
      .toEqual({ kind: "work", source: "fanbox", sourceId: "8421" });
    expect(contentLinkTarget("https://www.fanbox.cc/@rope-less/posts/8421"))
      .toEqual({ kind: "work", source: "fanbox", sourceId: "8421" });
  });

  it("takes a FANBOX address without a post as the creator", () => {
    expect(contentLinkTarget("https://kimodebu-kun.fanbox.cc/"))
      .toEqual({ kind: "person", source: "fanbox", sourceKey: "kimodebu-kun" });
    expect(contentLinkTarget("https://www.fanbox.cc/@kimodebu-kun"))
      .toEqual({ kind: "person", source: "fanbox", sourceKey: "kimodebu-kun" });
  });

  it("leaves addresses the library cannot hold to the browser", () => {
    expect(contentLinkTarget("https://skeb.jp/@miyu_hypno")).toBeNull();
    expect(contentLinkTarget("https://www.pixiv.net/requests/12")).toBeNull();
    expect(contentLinkTarget("not a url")).toBeNull();
  });
});
