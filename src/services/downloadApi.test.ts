import { beforeEach, expect, it, vi } from "vitest";
const api = vi.hoisted(()=>({invoke:vi.fn(),credentials:vi.fn()}));
vi.mock("@tauri-apps/api/core",()=>({invoke:api.invoke}));
vi.mock("@/services/updateJobApi",()=>({getUpdateJobCredentials:api.credentials}));
import { fetchPixivNovelByUrl } from "./downloadApi";
beforeEach(()=>{
  vi.clearAllMocks();
  api.invoke.mockResolvedValue({});
  api.credentials.mockResolvedValue({pixivRefreshToken:"download-token",pixivCookie:"download-cookie",pixivUserAgent:"download-ua"});
});
it("uses the saved download connection for illustration fallbacks",async()=>{
  await fetchPixivNovelByUrl("https://www.pixiv.net/novel/show.php?id=1","download-token");
  expect(api.invoke).toHaveBeenCalledWith("fetch_pixiv_novel_by_url",{url:"https://www.pixiv.net/novel/show.php?id=1",refreshToken:"download-token",pixivCookie:"download-cookie",pixivUserAgent:"download-ua"});
});
it("does not mix a different token with the saved account's web session",async()=>{
  await fetchPixivNovelByUrl("https://www.pixiv.net/novel/show.php?id=1","different-token");
  expect(api.invoke).toHaveBeenCalledWith("fetch_pixiv_novel_by_url",expect.objectContaining({pixivCookie:null,pixivUserAgent:null}));
});
