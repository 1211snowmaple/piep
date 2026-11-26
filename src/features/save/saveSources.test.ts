import { describe, expect, it, vi } from "vitest";
import { fetchFanboxPost, fetchPixivNovelByUrl } from "@/services/downloadApi";
import { SAVE_SOURCES, targetLabel } from "./saveSources";

vi.mock("@/services/downloadApi");

describe("保存候補の取得元", () => {
  it("取得元の識別子は表のキーに持たせる", () => {
    for (const profile of Object.values(SAVE_SOURCES)) {
      expect(profile).not.toHaveProperty("source");
    }
  });

  it("題名のない小説の候補と案内を日本語で表示する", async () => {
    const url = "https://www.pixiv.net/novels/123";
    vi.mocked(fetchPixivNovelByUrl).mockResolvedValue({ id: "123", user: { name: "作者" } });
    const rows = await SAVE_SOURCES.pixiv.targets.pixiv_single!.fetch("123", url);
    expect(fetchPixivNovelByUrl).toHaveBeenCalledWith(url);
    expect(rows).toMatchObject([{ id: "123", title: "題名のない小説", subtitle: "作者" }]);
    expect(targetLabel("pixiv", "pixiv_single")).toBe("小説を保存できます");
  });

  it("題名のない投稿の候補と案内を日本語で表示する", async () => {
    vi.mocked(fetchFanboxPost).mockResolvedValue({ accountKey: "使用中", data: { id: "456", user: { name: "作者" } } });
    const rows = await SAVE_SOURCES.fanbox.targets.fanbox_single!.fetch("456", "https://www.fanbox.cc/posts/456");
    expect(fetchFanboxPost).toHaveBeenCalledWith("456");
    expect(rows).toMatchObject([{ id: "456", title: "題名のない投稿", subtitle: "作者", restrictionAccountKey: "使用中" }]);
    expect(targetLabel("fanbox", "fanbox_single")).toBe("投稿を保存できます");
  });
});
