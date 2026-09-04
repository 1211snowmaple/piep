import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type {
  AssetEntry,
  DashboardSummary,
  DbStats,
  EditorDocument,
  DownloadEntry,
  ReaderMetadata,
  ReaderContentPage,
  ReaderOutlineEntry,
  ReaderSearchHit,
  LibraryDiagnostics,
  LibraryMaintenanceResult,
  SearchIndexOptimizationResult,
  SearchV2Params,
  SearchV2Result,
  WorkBlockInput,
  WorkEditRevision,
  BulkMutationResult,
  EntityFacet,
  EntityFacetScope,
  EntitySortBy,
  FilterFacets,
  FacetCount,
  UpdateTarget,
  SearchIndexStatus,
} from "@/types/library";
import type { UpdateJobCredentials } from "@/services/updateJobApi";
export type { SearchIndexStatus } from "@/types/library";

const MAX_FACET_PAGE_SIZE = 200;

function normalizePageSize(value: number, fallback: number): number {
  return Number.isFinite(value) ? Math.min(MAX_FACET_PAGE_SIZE, Math.max(1, Math.trunc(value))) : fallback;
}

function normalizeOffset(value: number): number {
  return Number.isFinite(value) ? Math.max(0, Math.trunc(value)) : 0;
}

function uniqueDownloadIds(ids: readonly number[]): number[] {
  const unique = new Set<number>();
  for (const id of ids) {
    if (!Number.isSafeInteger(id) || id <= 0) throw new RangeError(`Invalid download id: ${id}`);
    unique.add(id);
  }
  return [...unique];
}

export function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export async function getDashboardSummary(): Promise<DashboardSummary> {
  return invoke<DashboardSummary>("db_get_dashboard_summary");
}

export async function getStats(): Promise<DbStats> {
  return invoke<DbStats>("db_get_stats");
}

export async function getLibraryDiagnostics(): Promise<LibraryDiagnostics> {
  return invoke<LibraryDiagnostics>("db_get_library_diagnostics");
}

export interface EntityProfileRepairStatus {
  personCount: number;
  seriesCount: number;
  totalCount: number;
}

export interface EntityProfileRepairProgress {
  phase: "running" | "complete" | "canceled";
  completed: number;
  total: number;
  repaired: number;
  failed: number;
  activeLabel: string | null;
  error: string | null;
}

export interface EntityProfileRepairResult {
  attempted: number;
  repaired: number;
  failed: number;
  canceled: boolean;
  remaining: number;
}

/** 取りこぼした FANBOX の添付を数えた結果。読むだけで、棚には何も書かない。 */
export interface FanboxRepairScan {
  /** 取り直しの対象になる作品の id。そのまま更新ジョブへ渡せる。 */
  workIds: number[];
  /** 手元に無い添付の総数。 */
  missingFiles: number;
  /** そのうち、支援していないと取れない投稿の数。 */
  restrictedWorks: number;
  /** 走査した FANBOX の行数。 */
  scannedWorks: number;
}

/**
 * 原本JSONが求める添付を、実際に持っているかどうかで数える。
 *
 * 旧プログラムから取り込んだ投稿には、JSONに画像があるのに実ファイルが一つも
 * 無いものがある。本文も更新日時も同じなので、従来の更新確認は素通りしていた。
 */
export function scanFanboxAssetGaps(): Promise<FanboxRepairScan> {
  return invoke<FanboxRepairScan>("db_scan_fanbox_asset_gaps");
}

export function getEntityProfileRepairStatus(): Promise<EntityProfileRepairStatus> {
  return invoke<EntityProfileRepairStatus>("db_get_entity_profile_repair_status");
}

export function repairIncompleteEntityProfiles(credentials: UpdateJobCredentials): Promise<EntityProfileRepairResult> {
  return invoke<EntityProfileRepairResult>("repair_incomplete_entity_profiles", { credentials });
}

export function cancelEntityProfileRepair(): Promise<boolean> {
  return invoke<boolean>("cancel_entity_profile_repair");
}

export async function maintainLibrary(compact = false): Promise<LibraryMaintenanceResult> {
  return invoke<LibraryMaintenanceResult>("db_maintain_library", { compact });
}

export async function optimizeSearchIndex(): Promise<SearchIndexOptimizationResult> {
  return invoke<SearchIndexOptimizationResult>("db_optimize_search_index");
}

/** 保存フォルダーを走査した結果。読めなかったものは飛ばして続ける。 */
export interface ReimportOutcome {
  imported: number;
  /** 飛ばした作品と理由。空なら全部読めた。 */
  skipped: string[];
}

export async function scanAndReimportDownloads(): Promise<ReimportOutcome> {
  return invoke<ReimportOutcome>("scan_and_reimport_downloads");
}

export async function searchDownloadsV2(params: SearchV2Params): Promise<SearchV2Result> {
  return invoke<SearchV2Result>("search_downloads_v2", { params });
}

/**
 * The author/series aggregates are expensive and only the filter drawer's tag
 * and content-type lists are needed for browsing; `searchEntityFacets` serves
 * the entity tabs instead.
 */
export async function getFilterFacets(includeEntities = false): Promise<FilterFacets> {
  return invoke<FilterFacets>("db_get_filter_facets", { includeEntities });
}

export async function getSearchIndexStatus(): Promise<SearchIndexStatus> {
  return invoke<SearchIndexStatus>("db_get_search_index_status");
}

export async function searchFilterFacets(kind: "tag" | "tags" | "author" | "authors", query: string | null, limit = 30): Promise<FacetCount[]> {
  const normalizedQuery = query?.trim() || null;
  return invoke<FacetCount[]>("db_search_filter_facets", { kind, query: normalizedQuery, limit: normalizePageSize(limit, 30) });
}

/**
 * Paginated, searchable author/series listing. `db_get_filter_facets` only
 * returns the top 60 of each, which hides most of a large library.
 */
/**
 * How many authors or series there are, which the paged listing cannot say.
 *
 * Asked separately because it is a second pass over the same grouping: worth
 * doing once per set of conditions so the pager can name the last page, not
 * worth doing again for every page turned.
 */
export async function countEntityFacets(
  kind: "person" | "series",
  query: string | null,
  filters: SearchV2Params | null = null,
  scope: EntityFacetScope | null = null,
): Promise<number> {
  return invoke<number>("db_count_entity_facets", { kind, query: query?.trim() || null, filters, scope });
}

/**
 * `filters` narrows which works are grouped, using the same conditions the
 * works listing takes: an author only appears when they have a work that
 * passes, and their count is how many of their works do.
 */
export async function searchEntityFacets(
  kind: "person" | "series",
  query: string | null,
  limit = 60,
  offset = 0,
  filters: SearchV2Params | null = null,
  sortBy: EntitySortBy | null = null,
  sortOrder: "asc" | "desc" | null = null,
  scope: EntityFacetScope | null = null,
): Promise<EntityFacet[]> {
  return invoke<EntityFacet[]>("db_search_entity_facets", {
    kind,
    query: query?.trim() || null,
    limit: normalizePageSize(limit, 60),
    offset: normalizeOffset(offset),
    filters,
    sortBy,
    sortOrder,
    scope,
  });
}

/**
 * Fetches many works in one call, preserving the requested order. Missing ids
 * are omitted, which is how callers detect entries deleted since queueing.
 */
export async function getDownloads(ids: number[]): Promise<DownloadEntry[]> {
  const unique = uniqueDownloadIds(ids);
  if (!unique.length) return [];
  return invoke<DownloadEntry[]>("db_get_downloads", { ids: unique });
}

export async function getDownloadBySource<T = DownloadEntry>(source: string, sourceId: string): Promise<T | null> {
  return invoke<T | null>("db_get_download_by_source", { source, sourceId });
}

export async function getAssets<T = AssetEntry[]>(downloadId: number): Promise<T> {
  return invoke<T>("db_get_assets", { downloadId });
}

export async function deleteDownload(id: number): Promise<void> {
  return invoke<void>("db_delete_download", { id });
}

export async function deleteDownloads(ids: number[]): Promise<BulkMutationResult> {
  const unique = uniqueDownloadIds(ids);
  if (!unique.length) return { matchedCount: 0, changedCount: 0 };
  return invoke<BulkMutationResult>("db_delete_downloads", { ids: unique });
}

export async function deleteDownloadsForSearch(params: SearchV2Params): Promise<BulkMutationResult> {
  return invoke<BulkMutationResult>("db_delete_downloads_for_search", { params });
}

export async function setWatchUpdates(downloadId: number, watch: boolean): Promise<void> {
  return invoke<void>("db_set_watch_updates", { downloadId, watch });
}

export async function setFavorite(downloadId: number, favorite: boolean): Promise<void> {
  return invoke<void>("db_set_favorite", { downloadId, favorite });
}

/** Applies favourite/watch flags to many works in one transaction. */
export async function setFlagsForIds(ids: number[], flags: { favorite?: boolean; watch?: boolean }): Promise<BulkMutationResult> {
  const unique = uniqueDownloadIds(ids);
  if (!unique.length || (flags.favorite === undefined && flags.watch === undefined)) {
    return { matchedCount: unique.length, changedCount: 0 };
  }
  return invoke<BulkMutationResult>("db_set_flags_for_ids", { ids: unique, favorite: flags.favorite ?? null, watch: flags.watch ?? null });
}

export async function readFileContent(path: string): Promise<string> {
  return invoke<string>("read_file_content", { path });
}

export async function openLocalAsset(path: string): Promise<void> {
  return invoke<void>("open_local_asset", { path });
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

export async function setUpdateTargetEnabled(targetType: string, source: string, sourceKey: string, enabled: boolean): Promise<void> {
  return invoke<void>("db_set_update_target_enabled", { targetType, source, sourceKey, enabled });
}

export async function deleteUpdateTarget(targetType: string, source: string, sourceKey: string): Promise<void> {
  return invoke<void>("db_delete_update_target", { targetType, source, sourceKey });
}

export async function listUpdateTargets<T>(targetType: string | null = null, enabledOnly = false): Promise<T[]> {
  return invoke<T[]>("db_list_update_targets", { targetType, enabledOnly });
}

/** Fetch one target without transferring every watched entity over IPC. */
export async function getUpdateTarget<T = UpdateTarget>(targetType: string, source: string, sourceKey: string): Promise<T | null> {
  return invoke<T | null>("db_get_update_target", { targetType, source, sourceKey });
}

export async function getReaderMetadata(downloadId: number): Promise<ReaderMetadata> {
  return invoke<ReaderMetadata>("db_get_reader_metadata", { downloadId });
}

/**
 * 本文の 1 ページ。
 *
 * `includePlainText` は既定で落とす。読書画面は HTML しか使わないのに、
 * ページを繰るたび同じ本文を平文でも運んでいた。
 */
export async function getReaderContentPage(downloadId: number, version?: number | null, page = 0, includePlainText = false): Promise<ReaderContentPage> {
  return invoke<ReaderContentPage>("db_get_reader_content_page", { downloadId, version: version ?? null, page, includePlainText });
}

/** 作品全体の見出しと、それが載っているページ。読書画面の目次が使う。 */
export async function getReaderOutline(downloadId: number, version?: number | null): Promise<ReaderOutlineEntry[]> {
  return invoke<ReaderOutlineEntry[]>("db_get_reader_outline", { downloadId, version: version ?? null });
}

export async function searchReaderContent(downloadId: number, query: string, version?: number | null, limit = 50): Promise<ReaderSearchHit[]> {
  return invoke<ReaderSearchHit[]>("db_search_reader_content", { downloadId, version: version ?? null, query, limit });
}

export async function getEditorDocument(downloadId: number): Promise<EditorDocument> {
  return invoke<EditorDocument>("db_get_editor_document", { downloadId });
}

/**
 * 下書きを保存する。
 *
 * `title` は、取得元の題を書き換えたいときだけ渡す。元の題と同じものや
 * 空文字は端末側で落とすので、上書きは残らない。
 */
export async function saveWorkDraft(
  downloadId: number,
  baseVersion: number,
  title: string | null,
  blocks: WorkBlockInput[],
): Promise<WorkEditRevision> {
  return invoke<WorkEditRevision>("db_save_work_draft", { downloadId, baseVersion, title, blocks });
}

/**
 * 書きかけを捨てて、取り込んだままの本文へ戻す。
 *
 * 自動保存は触りはじめて6秒で下書きを作る。捨てる口が無かったので、一度でも
 * 編集画面を触れば、そこから先はいつ開いても書きかけが出てきた。
 */
export async function discardWorkDraft(downloadId: number): Promise<void> {
  return invoke<void>("db_discard_work_draft", { downloadId });
}

export async function activateWorkEdit(editRevisionId: number): Promise<WorkEditRevision> {
  return invoke<WorkEditRevision>("db_activate_work_edit", { editRevisionId });
}

/** 反映した編集を下ろし、取り込んだままの本文へ戻す。版は履歴に残る。 */
export async function deactivateWorkEdit(downloadId: number): Promise<void> {
  return invoke<void>("db_deactivate_work_edit", { downloadId });
}

export async function importWorkAsset(downloadId: number, sourcePath: string): Promise<AssetEntry> {
  return invoke<AssetEntry>("import_work_asset", { downloadId, sourcePath });
}

export function getAssetUrl(path: string | null | undefined): string | null {
  if (!path) return null;
  if (!isTauriRuntime()) {
    // ブラウザプレビューのデモデータだけは埋め込み画像や公開ダミー画像を使う。
    return path.startsWith("data:") || path.startsWith("http://") || path.startsWith("https://")
      ? path
      : null;
  }
  // native の値はローカル管理ファイルだけ。DBやimportにURLが紛れ込んでも、棚を
  // 開いただけで外部へ画像requestを送らない。
  if (/^(?:data|https?|asset):/i.test(path)) return null;
  return convertFileSrc(path);
}
