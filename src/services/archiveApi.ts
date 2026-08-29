import { invoke } from "@tauri-apps/api/core";

export function exportSingle(downloadId: number, destDir: string): Promise<string> {
  return invoke<string>("export_single", { downloadId, destDir });
}

export function exportAllMultipart(manifestPath: string): Promise<void> {
  return invoke<void>("export_all_multipart", { manifestPath });
}

export function exportEntityZip(entityType: string, source: string, sourceKey: string, zipPath: string): Promise<void> {
  return invoke<void>("export_entity_zip", { entityType, source, sourceKey, zipPath });
}

export function importZip(zipPath: string): Promise<number> {
  return invoke<number>("import_zip", { zipPath });
}

export function importMultipartBackup(manifestPath: string): Promise<number> {
  return invoke<number>("import_multipart_backup", { manifestPath });
}

export interface BackupInspection {
  valid: boolean;
  error: string | null;
  backupVersion: string | null;
  entryCount: number;
  compressedBytes: number;
  expandedBytes: number;
  requiredFreeBytes: number;
  availableFreeBytes: number | null;
  workCount: number;
  personCount: number;
  seriesCount: number;
  versionCount: number;
  assetCount: number;
  warnings: string[];
}

export type BackupFormat = "multipart" | "zip";

/** Only the two formats exposed by the file picker are accepted. */
export function backupFormatFromPath(path: string): BackupFormat | null {
  const normalized = path.toLowerCase();
  if (normalized.endsWith(".json")) return "multipart";
  if (normalized.endsWith(".zip")) return "zip";
  return null;
}

export function multipartManifestPath(path: string): string {
  if (!path.trim()) throw new Error("バックアップの保存先を指定してください");
  const segments = path.split(/[\\/]/);
  const filename = segments[segments.length - 1] ?? "";
  if (!filename) throw new Error("バックアップのファイル名を指定してください");
  const extensionAt = filename.lastIndexOf(".");
  if (extensionAt > 0 && filename.slice(extensionAt).toLowerCase() === ".json") return path;
  // Native save dialogs normally add the selected extension. Keep manually
  // entered extensionless names equally predictable.
  if (extensionAt <= 0) return `${path}.json`;
  throw new Error("分割バックアップのマニフェストは .json で保存してください");
}

export function inspectBackup(zipPath: string): Promise<BackupInspection> {
  return invoke<BackupInspection>("inspect_backup", { zipPath });
}

export function inspectMultipartBackup(manifestPath: string): Promise<BackupInspection> {
  return invoke<BackupInspection>("inspect_multipart_backup", { manifestPath });
}

export async function inspectBackupFile(path: string): Promise<{ format: BackupFormat; inspection: BackupInspection }> {
  const format = backupFormatFromPath(path);
  if (!format) throw new Error("JSONマニフェストまたはZIPバックアップを選択してください");
  const inspection = format === "multipart"
    ? await inspectMultipartBackup(path)
    : await inspectBackup(path);
  return { format, inspection };
}

export function importBackupFile(path: string, format: BackupFormat): Promise<number> {
  return format === "multipart" ? importMultipartBackup(path) : importZip(path);
}

/** 復元の進み具合。`archive-progress` で届く。 */
export interface ArchiveProgress {
  jobId: string;
  /** `extract` / `part` / `database`。 */
  phase: string;
  processed: number;
  total: number;
  label: string | null;
}

/**
 * 走っている復元を止める。
 *
 * **止まるのはライブラリを書き換える前まで。** 書き換えが始まったあとは、
 * 途中で降りるほうが危ないので受け付けない。返り値は「止める相手が居たか」。
 */
export function cancelArchiveRestore(): Promise<boolean> {
  return invoke<boolean>("cancel_archive_restore");
}

export function getStoragePath(): Promise<string> {
  return invoke<string>("get_storage_path");
}
