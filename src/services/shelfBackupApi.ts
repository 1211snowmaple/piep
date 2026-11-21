import { invoke } from "@tauri-apps/api/core";
import type { ReimportOutcome } from "@/services/dbApi";

/** 棚の写しの中身の要約。 */
export interface ShelfBackupInspection {
  path: string;
  works: number;
  files: number;
  bytes: number;
  /** 今の棚と同じ棚の写しか。 */
  sameLibrary: boolean;
  /** 正本に切り替えた棚の写しか。 */
  switched: boolean;
  /** 作者・シリーズのアーカイブ（棚の部分写し）か。入れ替えには使えず、取り込みに使う。 */
  partial: boolean;
}

/** 作者・シリーズのアーカイブの取り込みの結果（作品の数）。 */
export interface ShelfArchiveImport {
  /** 新しく足した作品。 */
  added: number;
  /** 棚から外していた作品を、残っていた記録のまま戻した数。 */
  restored: number;
  /** すでに棚にあって、そのままにした作品。 */
  already: number;
  /** 同じ作品などが別の記録として保存し直されていて、足せなかった作品。 */
  conflicts: number;
}

export interface ShelfArchiveExport {
  path: string;
  works: number;
}

export interface ShelfRestoreRequest {
  backup: string;
  requestedAt: number;
}

export interface ShelfRestoreOutcome {
  backup: string;
  /** 退けた前の棚（入れ替えたとき）。 */
  previous: string | null;
  error: string | null;
  finishedAt: number;
}

export interface ShelfRestoreState {
  pending: ShelfRestoreRequest | null;
  last: ShelfRestoreOutcome | null;
}

/** 今の棚の写しを `destination`（まだ無いフォルダー）に作る。返すのは作った場所。 */
export function libraryV2Backup(destination: string): Promise<string> {
  return invoke<string>("library_v2_backup", { destination });
}

/** 棚の写しを開いて全体を検査し、中身を数える。 */
export function libraryV2InspectBackup(path: string): Promise<ShelfBackupInspection> {
  return invoke<ShelfBackupInspection>("library_v2_inspect_backup", { path });
}

/** 検査を通った写しで、次の起動のときに棚を入れ替えるよう予約する。 */
export function libraryV2ScheduleRestore(path: string): Promise<ShelfBackupInspection> {
  return invoke<ShelfBackupInspection>("library_v2_schedule_restore", { path });
}

/**
 * 作者・シリーズの作品を、棚の部分写し（フォルダー一つ）として `parent` の中へ書き出す。
 * フォルダーの名前は `name`（同じ名前があれば番号を足す）。
 */
export function libraryV2ExportEntityArchive(kind: "person" | "series", provider: string, key: string, parent: string, name: string): Promise<ShelfArchiveExport> {
  return invoke<ShelfArchiveExport>("library_v2_export_entity_archive", { kind, provider, key, parent, name });
}

/** 作者・シリーズのアーカイブの作品を、今の棚へ足す。今の棚にある作品・記録はそのまま。 */
export function libraryV2ImportArchive(path: string): Promise<ShelfArchiveImport> {
  return invoke<ShelfArchiveImport>("library_v2_import_archive", { path });
}

/**
 * `downloads` の保存フォルダーを調べ、棚にまだ一度も入っていない作品を版ごと棚へ戻す。
 * 棚にある作品・棚から外した作品には触れない。
 */
export function libraryV2ReimportFolders(): Promise<ReimportOutcome> {
  return invoke<ReimportOutcome>("library_v2_reimport_folders");
}

/** 予約した復元と、前回の入れ替えの結果。 */
export function libraryV2RestoreState(): Promise<ShelfRestoreState> {
  return invoke<ShelfRestoreState>("library_v2_restore_state");
}

/** 予約した復元を取り消す。取り消すものが無ければ偽。 */
export function libraryV2CancelRestore(): Promise<boolean> {
  return invoke<boolean>("library_v2_cancel_restore");
}
