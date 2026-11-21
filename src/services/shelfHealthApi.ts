import { invoke } from "@tauri-apps/api/core";
import type { OriginalImageResult, OriginalImageStatus } from "@/services/dbApi";
import type { DashboardSummary, FacetCount, FilterFacets, LibraryMaintenanceResult, LibraryShelfCounts, SearchIndexOptimizationResult, SearchIndexStatus } from "@/types/library";

/**
 * 新しい棚の数と手入れ。`src-tauri/src/commands/library_health.rs` と対になる。
 *
 * 数える命令は開いた棚から読む（試験中の写しでも）。手入れの命令は正本に切り替えた棚だけに効く。
 */

/** 診断の数（棚の台帳・索引の大きさ）。 */
export interface ShelfDiagnostics {
  measuredAt: string;
  works: number;
  versions: number;
  textLength: number;
  media: number;
  files: number;
  logicalBytes: number;
  storedBytes: number;
  databaseBytes: number;
  walBytes: number;
  liveDatabaseBytes: number;
  fragmentationPercent: number;
  lexicalIndexBytes: number;
  lexicalIndexSegments: number;
  historyIndexBytes: number;
  semanticIndexBytes: number;
  processMemoryBytes: number | null;
  processPrivateMemoryBytes: number | null;
  gpuDedicatedMemoryBytes: number | null;
  gpuSharedMemoryBytes: number | null;
  searchIndex: SearchIndexStatus;
}

/** 回収で起きたこと。 */
export interface ShelfReclaimReport {
  released_files: number;
  quarantined_chunks: number;
  quarantined_bytes: number;
  recent_chunks: number;
  deleted_chunks: number;
  deleted_bytes: number;
  restored_chunks: number;
  removed_parts: number;
  removed_damaged: number;
}

export function shelfSearchIndexStatus(): Promise<SearchIndexStatus> {
  return invoke<SearchIndexStatus>("shelf_search_index_status");
}

/** ホームの集計。`recentDownloads` は空（最近の保存は棚の一覧で引く）。 */
export function shelfDashboard(): Promise<DashboardSummary> {
  return invoke<DashboardSummary>("shelf_dashboard");
}

/** 横の欄の件数。 */
export function shelfCounts(): Promise<LibraryShelfCounts> {
  return invoke<LibraryShelfCounts>("shelf_counts");
}

export function shelfDiagnostics(): Promise<ShelfDiagnostics> {
  return invoke<ShelfDiagnostics>("shelf_diagnostics");
}

export function shelfMaintain(compact: boolean): Promise<LibraryMaintenanceResult> {
  return invoke<LibraryMaintenanceResult>("shelf_maintain", { compact });
}

export function shelfOptimizeSearchIndex(): Promise<SearchIndexOptimizationResult> {
  return invoke<SearchIndexOptimizationResult>("shelf_optimize_search_index");
}

/** 台帳と全ファイルを照合し、確かめたファイルの数を返す（問題があれば誤り）。 */
export function shelfVerify(): Promise<number> {
  return invoke<number>("shelf_verify");
}

export function shelfReclaim(): Promise<ShelfReclaimReport> {
  return invoke<ShelfReclaimReport>("shelf_reclaim");
}

/** 一覧の絞り込みの選択肢（タグ・作者・種別・添付）。作者・シリーズの束ねは含めない。 */
export function shelfFilterFacets(): Promise<FilterFacets> {
  return invoke<FilterFacets>("shelf_filter_facets");
}

/** 絞り込みの選択肢を名前で探す。 */
export function shelfSearchFilterFacets(kind: "tags" | "authors", query: string | null, limit = 30): Promise<FacetCount[]> {
  return invoke<FacetCount[]>("shelf_search_filter_facets", { kind, query: query?.trim() || null, limit });
}

/** 新しい棚の、縮小版のまま残る pixiv の表紙・挿絵の数と、走っている取り直しの進み具合。 */
export function shelfOriginalImageStatus(): Promise<OriginalImageStatus> {
  return invoke<OriginalImageStatus>("shelf_original_image_status");
}

/** 新しい棚で、縮小版の表紙・挿絵を原寸に取り直す（版は増えない）。正本に切り替えた棚だけ。 */
export function shelfUpgradePixivImagesToOriginal(): Promise<OriginalImageResult> {
  return invoke<OriginalImageResult>("shelf_upgrade_pixiv_images_to_original");
}
