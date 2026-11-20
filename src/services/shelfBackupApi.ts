import { invoke } from "@tauri-apps/api/core";

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

/** 予約した復元と、前回の入れ替えの結果。 */
export function libraryV2RestoreState(): Promise<ShelfRestoreState> {
  return invoke<ShelfRestoreState>("library_v2_restore_state");
}

/** 予約した復元を取り消す。取り消すものが無ければ偽。 */
export function libraryV2CancelRestore(): Promise<boolean> {
  return invoke<boolean>("library_v2_cancel_restore");
}
