import { invoke } from "@tauri-apps/api/core";

export function fetchPixivNovelByUrl<T>(url: string, refreshToken: string): Promise<T> {
  return invoke<T>("fetch_pixiv_novel_by_url", { url, refreshToken });
}

export function fetchPixivSeriesNovels<T>(seriesId: string, refreshToken: string): Promise<T> {
  return invoke<T>("fetch_pixiv_series_novels", { seriesId, refreshToken });
}

export function fetchPixivUserNovels<T>(userId: string, refreshToken: string): Promise<T> {
  return invoke<T>("fetch_pixiv_user_novels", { userId, refreshToken });
}

export function fetchFanboxPost<T>(postId: string, cookie: string, userAgent: string): Promise<T> {
  return invoke<T>("fetch_fanbox_post", { postId, cookie, userAgent });
}

export function fetchFanboxCreatorPosts<T>(creatorId: string, cookie: string, userAgent: string): Promise<T> {
  return invoke<T>("fetch_fanbox_creator_posts", { creatorId, cookie, userAgent });
}
