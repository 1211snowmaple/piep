import { describe, expect, it } from "vitest";
import { downloadTargetKey, pageListSource, pageWorkItems, detectDownloadTarget, extractSavedSourceTarget, getFanboxCreatorId, normalizeContentLinkUrl } from "@/features/browser/downloadCandidates";

describe("download candidate URL detection", () => {
  it.each([
    ["https://www.pixiv.net/novel/show.php?id=123", "pixiv_single"],
    ["https://www.pixiv.net/en/novel/show.php?lang=ja&id=123", "pixiv_single"],
    ["https://www.pixiv.net/novel/series/456", "pixiv_series"],
    ["https://www.pixiv.net/novel/series/show.php?id=456", "pixiv_series"],
    ["https://www.pixiv.net/users/789/novels", "pixiv_user"],
    ["https://official.fanbox.cc/posts/100", "fanbox_single"],
    ["https://www.fanbox.cc/@official/posts/100", "fanbox_single"],
    ["https://official.fanbox.cc/", "fanbox_creator"],
  ])("detects %s", (url, expected) => expect(detectDownloadTarget(url)).toBe(expected));

  it("rejects unrelated and illustration pages", () => {
    expect(detectDownloadTarget("https://example.com/posts/1")).toBe("unsupported");
    expect(detectDownloadTarget("https://www.pixiv.net/artworks/123")).toBe("unsupported");
    expect(detectDownloadTarget("https://pixiv.net.evil.example/novels/123")).toBe("unsupported");
    expect(detectDownloadTarget("https://evil.example/www.pixiv.net/novels/123")).toBe("unsupported");
    expect(detectDownloadTarget("https://www.pixiv.net@evil.example/novels/123")).toBe("unsupported");
    expect(detectDownloadTarget("https://fanbox.cc.evil.example/posts/100")).toBe("unsupported");
    expect(detectDownloadTarget("https://official.fanbox.cc/posts/not-an-id")).toBe("unsupported");
    expect(detectDownloadTarget("https://www.pixiv.net/novel/show.php?id=not-an-id")).toBe("unsupported");
    expect(detectDownloadTarget("https://www.pixiv.net/novel/show.php?id=123abc")).toBe("unsupported");
    expect(detectDownloadTarget("https://www.pixiv.net/novels/123/unknown")).toBe("unsupported");
    expect(detectDownloadTarget("javascript:https://www.pixiv.net/novels/123")).toBe("unsupported");
    expect(extractSavedSourceTarget("https://evil.example/www.pixiv.net/novels/123")).toBeNull();
    expect(getFanboxCreatorId("https://fanbox.cc.evil.example/@official")).toBeNull();
  });

  it("normalizes pixiv deep links", () => {
    expect(normalizeContentLinkUrl("pixiv://novels/55")).toBe("https://www.pixiv.net/novel/show.php?id=55");
    expect(extractSavedSourceTarget("pixiv://novels/55")).toEqual({ source: "pixiv", sourceId: "55" });
    expect(extractSavedSourceTarget("pixiv://novels/55?lang=ja")).toEqual({ source: "pixiv", sourceId: "55" });
    expect(normalizeContentLinkUrl("https://evil.example/?next=pixiv://novels/55")).toBe("https://evil.example/?next=pixiv://novels/55");
  });

  it("extracts both FANBOX creator URL styles", () => {
    expect(getFanboxCreatorId("https://artist.fanbox.cc/posts/1")).toBe("artist");
    expect(getFanboxCreatorId("https://www.fanbox.cc/@artist")).toBe("artist");
  });
});

describe("一覧ページからの読み取り", () => {
  it("URL で全件を取れないページだけを一覧として扱う", () => {
    expect(pageListSource("https://www.pixiv.net/novel/bookmark_new.php")).toBe("pixiv");
    expect(pageListSource("https://www.pixiv.net/users/1/bookmarks/novels")).toBe("pixiv");
    expect(pageListSource("https://www.pixiv.net/tags/x/novels?p=2")).toBe("pixiv");
    expect(pageListSource("https://www.fanbox.cc/home")).toBe("fanbox");
    expect(pageListSource("https://www.pixiv.net/users/1/novels")).toBeNull();
    expect(pageListSource("https://www.pixiv.net/novel/show.php?id=1")).toBeNull();
    expect(pageListSource("http://www.pixiv.net/novel/ranking.php")).toBeNull();
    expect(pageListSource("https://evil.example/www.pixiv.net/")).toBeNull();
  });

  it("別のページへ進むと、読み取った一覧は古くなる", () => {
    const first = downloadTargetKey("https://www.pixiv.net/en/tags/x/novels?p=1");
    expect(first).toBe("page:www.pixiv.net/tags/x/novels?p=1");
    expect(downloadTargetKey("https://www.pixiv.net/tags/x/novels?p=2")).not.toBe(first);
    expect(downloadTargetKey("https://evil.example/")).toBe("");
  });

  it("棚にある作品は選ばず、題名が読めなければ ID で名乗る", () => {
    const [unsaved, saved] = pageWorkItems("pixiv", [
      { sourceId: "1", title: "", author: "", saved: false },
      { sourceId: "2", title: "題名", author: "作者", saved: true },
    ]);
    expect(unsaved).toMatchObject({ id: "1", title: "作品 1", selected: true });
    expect(unsaved.status).toBeUndefined();
    expect(unsaved.subtitle).toBeUndefined();
    expect(saved).toMatchObject({ id: "2", title: "題名", subtitle: "作者", selected: false, status: "skipped" });
  });

  it("FANBOX の行に投稿の権限を決めつける値を持たせない", () => {
    const [item] = pageWorkItems("fanbox", [{ sourceId: "5", title: "投稿", author: "", saved: false }]);
    expect("isRestricted" in item.originalData).toBe(false);
  });
});
