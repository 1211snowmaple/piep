import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type {
  AssetEntry,
  DashboardSummary,
  DbStats,
  EditorDocument,
  DownloadEntry,
  DownloadVersion,
  ReaderDocument,
  SearchV2Params,
  SearchV2Result,
  WorkBlockInput,
  WorkEditRevision,
  BulkMutationResult,
  FilterFacets,
} from "@/types/library";

export function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export async function getDashboardSummary(): Promise<DashboardSummary> {
  return invoke<DashboardSummary>("db_get_dashboard_summary");
}

export async function getStats(): Promise<DbStats> {
  return invoke<DbStats>("db_get_stats");
}

export async function seedTestData(count: number): Promise<number> {
  return invoke<number>("db_seed_test_data", { count });
}

export async function scanAndReimportDownloads(): Promise<number> {
  return invoke<number>("scan_and_reimport_downloads");
}

export async function searchDownloadsV2(params: SearchV2Params): Promise<SearchV2Result> {
  return invoke<SearchV2Result>("search_downloads_v2", { params });
}

export interface SearchIndexStatus {
  totalDownloads: number;
  indexedDownloads: number;
  pendingDownloads: number;
  isComplete: boolean;
  phase: string;
  indexedChunks: number;
  semanticIndexedChunks: number;
  semanticModelReady: boolean;
  embeddingProvider: string;
  gpuEnabled: boolean;
  throughputPerSec?: number | null;
}

export async function getFilterFacets(): Promise<FilterFacets> {
  return invoke<FilterFacets>("db_get_filter_facets");
}

export async function getSearchIndexStatus(): Promise<SearchIndexStatus> {
  return invoke<SearchIndexStatus>("db_get_search_index_status");
}

export async function searchFilterFacets(kind: string, query: string | null, limit = 30): Promise<{ name: string; count: number }[]> {
  return invoke<{ name: string; count: number }[]>("db_search_filter_facets", { kind, query, limit });
}

export async function getDownload<T = DownloadEntry>(id: number): Promise<T> {
  return invoke<T>("db_get_download", { id });
}

export async function getDownloadBySource<T = DownloadEntry>(source: string, sourceId: string): Promise<T | null> {
  return invoke<T | null>("db_get_download_by_source", { source, sourceId });
}

export async function getAssets<T = AssetEntry[]>(downloadId: number): Promise<T> {
  return invoke<T>("db_get_assets", { downloadId });
}

export async function getVersions<T = DownloadVersion[]>(downloadId: number): Promise<T> {
  return invoke<T>("db_get_versions", { downloadId });
}

export async function deleteDownload(id: number): Promise<void> {
  return invoke<void>("db_delete_download", { id });
}

export async function deleteDownloads(ids: number[]): Promise<BulkMutationResult> {
  return invoke<BulkMutationResult>("db_delete_downloads", { ids });
}

export async function deleteDownloadsForSearch(params: SearchV2Params): Promise<BulkMutationResult> {
  return invoke<BulkMutationResult>("db_delete_downloads_for_search", { params });
}

export async function deleteVersion(downloadId: number, version: number): Promise<void> {
  return invoke<void>("db_delete_version", { downloadId, version });
}

export async function setWatchUpdates(downloadId: number, watch: boolean): Promise<void> {
  return invoke<void>("db_set_watch_updates", { downloadId, watch });
}

export async function setWatchUpdatesForSearch(params: SearchV2Params, watch: boolean): Promise<BulkMutationResult> {
  return invoke<BulkMutationResult>("db_set_watch_updates_for_search", { params, watch });
}

export async function setFavorite(downloadId: number, favorite: boolean): Promise<void> {
  return invoke<void>("db_set_favorite", { downloadId, favorite });
}

export async function readFileContent(path: string): Promise<string> {
  return invoke<string>("read_file_content", { path });
}

export async function openLocalAsset(path: string): Promise<void> {
  return invoke<void>("open_local_asset", { path });
}

export async function getDownloadHtml(downloadId: number, version: number): Promise<string> {
  return invoke<string>("db_get_download_html", { downloadId, version });
}

export async function getPerson<T>(source: string, sourceKey: string): Promise<T> {
  return invoke<T>("db_get_person", { source, sourceKey });
}

export async function getSeries<T>(source: string, sourceKey: string): Promise<T> {
  return invoke<T>("db_get_series", { source, sourceKey });
}

export async function listEntityVersions<T>(entityType: string, source: string, sourceKey: string): Promise<T> {
  return invoke<T>("db_list_entity_versions", { entityType, source, sourceKey });
}

export async function getLatestEntityProfileJson<T>(entityType: string, source: string, sourceKey: string): Promise<T | null> {
  return invoke<T | null>("db_get_latest_entity_profile_json", { entityType, source, sourceKey });
}

export async function refreshEntityProfile<T>(params: Record<string, unknown>): Promise<T> {
  return invoke<T>("refresh_entity_profile", { params });
}

export async function upsertUpdateTarget(target: Record<string, unknown>): Promise<void> {
  await invoke("db_upsert_update_target", { target });
}

export async function markUpdateTargetChecked(payload: Record<string, unknown>): Promise<void> {
  await invoke("db_mark_update_target_checked", payload);
}

export async function setUpdateTargetEnabled(targetType: string, source: string, sourceKey: string, enabled: boolean): Promise<void> {
  return invoke<void>("db_set_update_target_enabled", { targetType, source, sourceKey, enabled });
}

export async function deleteUpdateTarget(targetType: string, source: string, sourceKey: string): Promise<void> {
  return invoke<void>("db_delete_update_target", { targetType, source, sourceKey });
}

export async function listUpdateTargets<T>(targetType: string | null = null, enabledOnly = false): Promise<T[]> {
  return invoke<T[]>("db_list_update_targets", { targetType, enabledOnly });
}

export async function getWatchedDownloads<T = DownloadEntry>(): Promise<T[]> {
  return invoke<T[]>("db_get_watched_downloads");
}

export async function listDownloadRelations<T>(relationType: string | null = null): Promise<T[]> {
  return invoke<T[]>("db_list_download_relations", { relationType });
}

export async function getReaderDocument(downloadId: number, version?: number | null): Promise<ReaderDocument> {
  return invoke<ReaderDocument>("db_get_reader_document", { downloadId, version: version ?? null });
}

export async function getEditorDocument(downloadId: number): Promise<EditorDocument> {
  return invoke<EditorDocument>("db_get_editor_document", { downloadId });
}

export async function saveWorkDraft(
  downloadId: number,
  baseVersion: number,
  blocks: WorkBlockInput[],
): Promise<WorkEditRevision> {
  return invoke<WorkEditRevision>("db_save_work_draft", { downloadId, baseVersion, blocks });
}

export async function activateWorkEdit(editRevisionId: number): Promise<WorkEditRevision> {
  return invoke<WorkEditRevision>("db_activate_work_edit", { editRevisionId });
}

export async function importWorkAsset(downloadId: number, sourcePath: string): Promise<AssetEntry> {
  return invoke<AssetEntry>("import_work_asset", { downloadId, sourcePath });
}

export function getAssetUrl(path: string | null | undefined): string | null {
  if (!path || !isTauriRuntime()) return null;
  return convertFileSrc(path);
}
