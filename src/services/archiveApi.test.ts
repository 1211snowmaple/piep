import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import {
  backupFormatFromPath,
  importBackupFile,
  inspectBackupFile,
  multipartManifestPath,
} from "@/services/archiveApi";

describe("backup file routing", () => {
  beforeEach(() => invoke.mockReset());

  it("routes JSON manifests to multipart inspection and import", async () => {
    const inspection = { valid: true, warnings: [] };
    invoke.mockResolvedValueOnce(inspection).mockResolvedValueOnce(12);

    await expect(inspectBackupFile("C:\\Backups\\Library.JSON")).resolves.toEqual({ format: "multipart", inspection });
    expect(invoke).toHaveBeenNthCalledWith(1, "inspect_multipart_backup", { manifestPath: "C:\\Backups\\Library.JSON" });
    await expect(importBackupFile("C:\\Backups\\Library.JSON", "multipart")).resolves.toBe(12);
    expect(invoke).toHaveBeenNthCalledWith(2, "import_multipart_backup", { manifestPath: "C:\\Backups\\Library.JSON" });
  });

  it("keeps existing ZIP inspection and import compatible", async () => {
    const inspection = { valid: true, warnings: [] };
    invoke.mockResolvedValueOnce(inspection).mockResolvedValueOnce(4);

    await expect(inspectBackupFile("D:\\old-backup.ZIP")).resolves.toEqual({ format: "zip", inspection });
    expect(invoke).toHaveBeenNthCalledWith(1, "inspect_backup", { zipPath: "D:\\old-backup.ZIP" });
    await expect(importBackupFile("D:\\old-backup.ZIP", "zip")).resolves.toBe(4);
    expect(invoke).toHaveBeenNthCalledWith(2, "import_zip", { zipPath: "D:\\old-backup.ZIP" });
  });

  it("rejects an ambiguous file before invoking native restore code", async () => {
    expect(backupFormatFromPath("backup.json.zip")).toBe("zip");
    expect(backupFormatFromPath("backup.txt")).toBeNull();
    await expect(inspectBackupFile("backup.txt")).rejects.toThrow("JSONマニフェストまたはZIPバックアップ");
    expect(invoke).not.toHaveBeenCalled();
  });

  it("adds the manifest extension only to extensionless save paths", () => {
    expect(multipartManifestPath("C:\\Backups\\piep-2026-08-12")).toBe("C:\\Backups\\piep-2026-08-12.json");
    expect(multipartManifestPath("C:\\Backups\\piep.JSON")).toBe("C:\\Backups\\piep.JSON");
    expect(() => multipartManifestPath("C:\\Backups\\piep.zip")).toThrow(".json で保存");
  });
});
