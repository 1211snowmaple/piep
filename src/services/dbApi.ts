import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type {
  UpdateTarget,
  WatchGroup,
  WatchId,
  SearchIndexStatus,
} from "@/types/library";
export type { SearchIndexStatus } from "@/types/library";

export function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export interface EntityProfileRepairStatus {
  running?: boolean;
  progress?: EntityProfileRepairProgress | null;
  personCount: number;
  seriesCount: number;
  totalCount: number;
}

export interface EntityProfileRepairProgress {
  phase: "running" | "checked" | "complete" | "canceled" | "paused";
  completed: number;
  total: number;
  repaired: number;
  failed: number;
  activeLabel: string | null;
  error: string | null;
}

export interface EntityProfileRepairResult {
  stoppedReason?: string | null;
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

export function getEntityProfileRepairStatus(): Promise<EntityProfileRepairStatus> {
  return invoke<EntityProfileRepairStatus>("db_get_entity_profile_repair_status");
}

export function repairIncompleteEntityProfiles(): Promise<EntityProfileRepairResult> {
  return invoke<EntityProfileRepairResult>("repair_incomplete_entity_profiles");
}

export function cancelEntityProfileRepair(): Promise<boolean> {
  return invoke<boolean>("cancel_entity_profile_repair");
}

/** 縮小版で保存された pixiv の表紙・挿絵の数と、取り直しの進み具合。 */
export interface OriginalImageStatus {
  covers: number;
  illustrations: number;
  running: boolean;
  progress: OriginalImageProgress | null;
}

export interface OriginalImageProgress {
  phase: "running" | "checked" | "complete" | "canceled" | "paused";
  completed: number;
  total: number;
  upgraded: number;
  /** 公開元に原寸が無かった画像。縮小版のまま残る。 */
  unavailable: number;
  failed: number;
  activeLabel: string | null;
  error: string | null;
}

export interface OriginalImageResult {
  attempted: number;
  upgraded: number;
  unavailable: number;
  failed: number;
  canceled: boolean;
  stoppedReason: string | null;
  remaining: number;
}

export function cancelOriginalImageUpgrade(): Promise<boolean> {
  return invoke<boolean>("cancel_original_image_upgrade");
}

/** 保存フォルダーを走査した結果。読めなかったものは飛ばして続ける。 */
export interface ReimportOutcome {
  imported: number;
  /** 飛ばした作品と理由。空なら全部読めた。 */
  skipped: string[];
}

export async function getSearchIndexStatus(): Promise<SearchIndexStatus> {
  return invoke<SearchIndexStatus>("db_get_search_index_status");
}

export async function refreshEntityProfile<T>(params: Record<string, unknown>): Promise<T> {
  return invoke<T>("refresh_entity_profile", { params });
}

export async function upsertUpdateTarget(target: Record<string, unknown>): Promise<void> {
  const before = await getUpdateTarget(String(target.targetType), String(target.source), String(target.sourceKey));
  await invoke("db_upsert_update_target", { target, requestId: crypto.randomUUID(), expectedGeneration: before?.watchGeneration ?? null });
}

export async function setUpdateTargetEnabled(targetType: string, source: string, sourceKey: string, enabled: boolean): Promise<void> {
  const before = await getUpdateTarget(targetType, source, sourceKey);
  return invoke<void>("db_set_update_target_enabled", { targetType, source, sourceKey, enabled, targetId: before?.id ?? null, requestId: crypto.randomUUID(), expectedGeneration: before?.watchGeneration ?? null });
}

export async function deleteUpdateTarget(targetType: string, source: string, sourceKey: string): Promise<void> {
  const before = await getUpdateTarget(targetType, source, sourceKey);
  return invoke<void>("db_delete_update_target", { targetType, source, sourceKey, targetId: before?.id ?? null, requestId: crypto.randomUUID(), expectedGeneration: before?.watchGeneration ?? null });
}

export async function listUpdateTargets<T>(targetType: string | null = null, enabledOnly = false): Promise<T[]> {
  return invoke<T[]>("db_list_update_targets", { targetType, enabledOnly });
}

export async function listWatchGroups(): Promise<WatchGroup[]> {
  return invoke<WatchGroup[]>("db_list_watch_groups");
}

/** 新しい監視グループを作り、その ID を返す。同じ名前があれば失敗する。 */
export async function createWatchGroup(name: string): Promise<WatchId> {
  const groups = await listWatchGroups();
  return invoke<WatchId>("db_create_watch_group", { name, requestId: crypto.randomUUID(), expectedGeneration: groups[0]?.watchGeneration ?? null });
}

export async function renameWatchGroup(groupId: WatchId, name: string): Promise<void> {
  const group = (await listWatchGroups()).find((item) => item.id === groupId || item.legacyId === groupId);
  return invoke<void>("db_rename_watch_group", { groupId: group?.id ?? groupId, name, requestId: crypto.randomUUID(), expectedGeneration: group?.watchGeneration ?? null });
}

/** グループだけを消す。中の監視対象は残る。 */
export async function deleteWatchGroup(groupId: WatchId): Promise<void> {
  const group = (await listWatchGroups()).find((item) => item.id === groupId || item.legacyId === groupId);
  return invoke<void>("db_delete_watch_group", { groupId: group?.id ?? groupId, requestId: crypto.randomUUID(), expectedGeneration: group?.watchGeneration ?? null });
}

/** 監視対象をグループに入れる（add）か外す。変わった件数を返す。 */
export async function setWatchGroupMembers(groupId: WatchId, targetIds: WatchId[], add: boolean): Promise<number> {
  const group = (await listWatchGroups()).find((item) => item.id === groupId || item.legacyId === groupId);
  return invoke<number>("db_set_watch_group_members", { groupId: group?.id ?? groupId, targetIds, add, requestId: crypto.randomUUID(), expectedGeneration: group?.watchGeneration ?? null });
}

/** Fetch one target without transferring every watched entity over IPC. */
export async function getUpdateTarget<T = UpdateTarget>(targetType: string, source: string, sourceKey: string): Promise<T | null> {
  return invoke<T | null>("db_get_update_target", { targetType, source, sourceKey });
}

export interface PdfPageImage {
  /** `data:image/webp;base64,...`。そのまま `img` の `src` に渡せる。 */
  image: string;
  width: number;
  height: number;
  pageCount: number;
}

export function getAssetUrl(path: string | null | undefined): string | null {
  if (!path) return null;
  if (!isTauriRuntime()) {
    // ブラウザプレビューのデモデータだけは埋め込み画像や公開ダミー画像を使う。
    return path.startsWith("data:") || path.startsWith("http://") || path.startsWith("https://")
      ? path
      : null;
  }
  if (/^shelf:[0-9a-f]{64}$/.test(path)) return convertFileSrc(path.slice(6), "piep-library");
  // native の値はローカル管理ファイルだけ。DBやimportにURLが紛れ込んでも、棚を
  // 開いただけで外部へ画像requestを送らない。
  if (/^(?:data|https?|asset):/i.test(path)) return null;
  return convertFileSrc(path);
}
