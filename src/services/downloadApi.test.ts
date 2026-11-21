import { beforeEach, expect, it, vi } from "vitest";
const api = vi.hoisted(()=>({invoke:vi.fn()}));
vi.mock("@tauri-apps/api/core",()=>({invoke:api.invoke}));
import { fetchFanboxPost, fetchPixivNovelByUrl } from "./downloadApi";
beforeEach(()=>{
  vi.clearAllMocks();
  api.invoke.mockResolvedValue({});
});
// 鍵はバックエンドが保存してある接続から読む。画面から渡すものは無い。
it("sends no keys from the page",async()=>{
  await fetchPixivNovelByUrl("https://www.pixiv.net/novel/show.php?id=1");
  expect(api.invoke).toHaveBeenCalledWith("fetch_pixiv_novel_by_url",{url:"https://www.pixiv.net/novel/show.php?id=1"});
  await fetchFanboxPost("9");
  expect(api.invoke).toHaveBeenCalledWith("fetch_fanbox_post",{postId:"9"});
});
