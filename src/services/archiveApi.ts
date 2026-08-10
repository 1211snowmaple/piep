import { invoke } from "@tauri-apps/api/core";

export function exportSingle(downloadId: number, destDir: string): Promise<string> {
  return invoke<string>("export_single", { downloadId, destDir });
}

export function exportAllZip(zipPath: string): Promise<void> {
  return invoke<void>("export_all_zip", { zipPath });
}

export function exportEntityZip(entityType: string, source: string, sourceKey: string, zipPath: string): Promise<void> {
  return invoke<void>("export_entity_zip", { entityType, source, sourceKey, zipPath });
}

export function importZip(zipPath: string): Promise<number> {
  return invoke<number>("import_zip", { zipPath });
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

export function inspectBackup(zipPath: string): Promise<BackupInspection> {
  return invoke<BackupInspection>("inspect_backup", { zipPath });
}

export function getStoragePath(): Promise<string> {
  return invoke<string>("get_storage_path");
}
