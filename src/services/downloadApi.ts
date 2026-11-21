import { invoke } from "@tauri-apps/api/core";

// 鍵は渡さない。バックエンドが保存してある接続から自分で読む。

export function fetchPixivNovelByUrl<T>(url: string): Promise<T> {
  return invoke<T>("fetch_pixiv_novel_by_url", { url });
}

export function fetchPixivSeriesNovels<T>(seriesId: string): Promise<T> {
  return invoke<T>("fetch_pixiv_series_novels", { seriesId });
}

export function fetchPixivUserNovels<T>(userId: string): Promise<T> {
  return invoke<T>("fetch_pixiv_user_novels", { userId });
}

export function fetchFanboxPost<T>(postId: string): Promise<T> {
  return invoke<T>("fetch_fanbox_post", { postId });
}

export function fetchFanboxCreatorPosts<T>(creatorId: string): Promise<T> {
  return invoke<T>("fetch_fanbox_creator_posts", { creatorId });
}
