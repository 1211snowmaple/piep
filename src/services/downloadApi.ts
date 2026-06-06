import { invoke } from "@tauri-apps/api/core";

export function fetchPixivNovelMetadata<T>(novelId: string, refreshToken: string): Promise<T> {
  return invoke<T>("fetch_pixiv_novel_metadata", { novelId, refreshToken });
}

export function fetchPixivNovel<T>(novelId: string, refreshToken: string): Promise<T> {
  return invoke<T>("fetch_pixiv_novel", { novelId, refreshToken });
}

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

export function downloadAndSave<T>(payload: Record<string, unknown>): Promise<T> {
  return invoke<T>("download_and_save", payload);
}
