import { beforeEach, expect, it, vi } from "vitest";
import { contentLinkTarget, followContentLink } from "./contentLinks";
import { safePageHtml } from "./readerDocument";
import type { AppNavigation } from "./appHistory";

const api = vi.hoisted(() => ({ lookupPcWork: vi.fn(), getPcEntityDetail: vi.fn(), listOfflineWorks: vi.fn(), openUrl: vi.fn() }));
vi.mock("./pcApi", () => api);
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: api.openUrl }));
beforeEach(() => { vi.resetAllMocks(); api.listOfflineWorks.mockResolvedValue([]); });
const navigation = () => ({ push: vi.fn(), openEntity: vi.fn(), canBrowseServer: true }) as unknown as AppNavigation;

it("pixiv と FANBOX の作品・作者・シリーズだけを取得元の宛先にする", () => {
  expect(contentLinkTarget("https://www.pixiv.net/novel/show.php?id=123")).toEqual({ kind: "work", source: "pixiv", sourceId: "123" });
  expect(contentLinkTarget("https://name.fanbox.cc/posts/9")).toEqual({ kind: "work", source: "fanbox", sourceId: "9" });
  expect(contentLinkTarget("https://www.fanbox.cc/@name")).toEqual({ kind: "person", source: "fanbox", sourceKey: "name" });
  expect(contentLinkTarget("https://www.pixiv.net/novel/series/3")).toEqual({ kind: "series", source: "pixiv", sourceKey: "3" });
  for (const url of ["http://name.fanbox.cc/posts/9", "https://name.fanbox.cc:444/posts/9", "https://user:pass@name.fanbox.cc/posts/9", "https://fanbox.cc.example.org/posts/9"]) expect(contentLinkTarget(url)).toBeNull();
});
it("ライブラリ内の作品は読書画面へ開き、ない作品は外部へ開く", async () => {
  const nav = navigation();
  const work = { source: "pixiv", sourceId: "123", id: 7 };
  api.lookupPcWork.mockResolvedValueOnce(work).mockResolvedValueOnce(null);
  await followContentLink("https://www.pixiv.net/novel/show.php?id=123", nav);
  expect(nav.push).toHaveBeenCalledWith({ detail: null, reader: { kind: "online", work } });
  expect(api.openUrl).not.toHaveBeenCalled();
  await followContentLink("https://www.pixiv.net/novel/show.php?id=456", nav);
  expect(api.openUrl).toHaveBeenCalledWith("https://www.pixiv.net/novel/show.php?id=456");
});
it("圏外でも端末にある作品と作者へ移動できる", async () => {
  const nav = { ...navigation(), canBrowseServer: false };
  const saved = { work: { source: "fanbox", sourceId: "9", authorId: "name" } };
  api.listOfflineWorks.mockResolvedValue([saved]);
  await followContentLink("https://name.fanbox.cc/posts/9", nav);
  expect(nav.push).toHaveBeenCalledWith({ detail: null, reader: { kind: "offline", work: saved } });
  await followContentLink("https://name.fanbox.cc", nav);
  expect(nav.push).toHaveBeenLastCalledWith({ page: { name: "offline", tab: "people", groupId: "fanbox:name" }, detail: null, reader: null });
  expect(api.lookupPcWork).not.toHaveBeenCalled();
});
it("本文のリンクを安全な https に直し、危険な href や属性を残さない", () => {
  const clean = safePageHtml('<a href="pixiv://novels/123">前編</a><a href="javascript:alert(1)" onclick="alert(1)">危険</a><iframe src="https://example.org"></iframe>');
  expect(clean).toContain('href="https://www.pixiv.net/novel/show.php?id=123"');
  expect(clean).not.toMatch(/javascript:|onclick|iframe|pixiv:\/\//);
});
