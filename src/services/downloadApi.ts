import { invoke } from "@tauri-apps/api/core";

// 鍵は渡さない。バックエンドが保存してある接続から自分で読む。

export function fetchPixivNovelByUrl<T>(url: string): Promise<T> {
  return invoke<T>("fetch_pixiv_novel_by_url", { url });
}

export interface CandidateList<T> {
  items: T[];
  truncated: boolean;
}

export function fetchPixivSeriesNovels<T>(seriesId: string): Promise<CandidateList<T>> {
  return invoke<CandidateList<T>>("fetch_pixiv_series_novels", { seriesId });
}

export function fetchPixivUserNovels<T>(userId: string): Promise<CandidateList<T>> {
  return invoke<CandidateList<T>>("fetch_pixiv_user_novels", { userId });
}

interface FanboxCandidates<T> {
  accountKey: string;
  data: T;
}

export function fetchFanboxPost<T>(postId: string): Promise<FanboxCandidates<T>> {
  return invoke<FanboxCandidates<T>>("fetch_fanbox_post", { postId });
}

export function fetchFanboxCreatorPosts<T>(creatorId: string): Promise<FanboxCandidates<CandidateList<T>>> {
  return invoke<FanboxCandidates<CandidateList<T>>>("fetch_fanbox_creator_posts", { creatorId });
}
