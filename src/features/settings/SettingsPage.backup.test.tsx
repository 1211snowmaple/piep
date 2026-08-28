import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { theme } from "@/theme";
import type { BackupInspection } from "@/services/archiveApi";

const archive = vi.hoisted(() => ({
  exportAllMultipart: vi.fn(),
  getStoragePath: vi.fn(),
  importBackupFile: vi.fn(),
  inspectBackupFile: vi.fn(),
}));
const dialogs = vi.hoisted(() => ({ openSingleDialog: vi.fn(), saveDialog: vi.fn() }));

vi.mock("@/services/archiveApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/archiveApi")>()),
  ...archive,
}));
vi.mock("@/services/dialogApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dialogApi")>()),
  ...dialogs,
}));
vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  getStats: vi.fn().mockResolvedValue({ totalDownloads: 12, totalAssets: 108, totalSizeBytes: 32_000_000 }),
  getSearchIndexStatus: vi.fn().mockResolvedValue({ totalDownloads: 12, indexedDownloads: 12, pendingDownloads: 0, isComplete: true, phase: "ready", semanticIndexedChunks: 0, semanticIndexedDownloads: 0, semanticPendingDownloads: 12, semanticModelReady: false, embeddingProvider: "CPU", gpuEnabled: false, throughputPerSec: null }),
  scanAndReimportDownloads: vi.fn().mockResolvedValue(0),
}));
vi.mock("@/store", () => ({
  store: {
    get: vi.fn().mockResolvedValue(null),
    set: vi.fn().mockResolvedValue(undefined),
    delete: vi.fn().mockResolvedValue(undefined),
    save: vi.fn().mockResolvedValue(undefined),
  },
}));
vi.mock("@/features/search/searchIndexProgress", () => ({ useSearchIndexProgress: () => null }));
vi.mock("@/features/jobs/operationJobs", () => ({
  startOperation: () => ({
    id: "test-operation",
    progress: vi.fn(),
    log: vi.fn(),
    complete: vi.fn(),
    fail: vi.fn(),
    cancel: vi.fn(),
    isCancelRequested: () => false,
  }),
  requestOperationCancel: vi.fn(),
}));

import SettingsPage from "./SettingsPage";

const inspection: BackupInspection = {
  valid: true,
  error: null,
  backupVersion: "3.0",
  entryCount: 140,
  compressedBytes: 10_000_000,
  expandedBytes: 32_000_000,
  requiredFreeBytes: 64_000_000,
  availableFreeBytes: 1_000_000_000,
  workCount: 12,
  personCount: 3,
  seriesCount: 2,
  versionCount: 14,
  assetCount: 108,
  warnings: [],
};

function renderLibrarySettings() {
  window.location.hash = "#/settings?section=library";
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return render(
    <MantineProvider theme={theme}>
      <QueryClientProvider client={client}>
        <ModalsProvider><AppRouter><SettingsPage /></AppRouter></ModalsProvider>
      </QueryClientProvider>
    </MantineProvider>,
  );
}

describe("SettingsPage backup flow", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    archive.getStoragePath.mockResolvedValue("C:\\piep\\downloads");
    archive.exportAllMultipart.mockResolvedValue(undefined);
    archive.importBackupFile.mockResolvedValue(12);
  });

  it("inspects and restores a selected JSON manifest using its recorded format", async () => {
    dialogs.openSingleDialog.mockResolvedValue("C:\\Backups\\library.json");
    archive.inspectBackupFile.mockResolvedValue({ format: "multipart", inspection });
    renderLibrarySettings();

    fireEvent.click(await screen.findByRole("button", { name: "バックアップを復元" }));
    expect(await screen.findByRole("dialog", { name: "バックアップ復元ウィザード" })).toBeInTheDocument();
    expect(archive.inspectBackupFile).toHaveBeenCalledWith("C:\\Backups\\library.json");
    expect(screen.getByText("選択したJSONマニフェスト")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "検証済みバックアップを復元" }));
    await waitFor(() => expect(archive.importBackupFile).toHaveBeenCalledWith("C:\\Backups\\library.json", "multipart"));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "バックアップ復元ウィザード" })).toBeNull());
  });

  it("writes an extensionless selection as a JSON manifest", async () => {
    dialogs.saveDialog.mockResolvedValue("C:\\Backups\\piep-2026-08-12");
    renderLibrarySettings();

    fireEvent.click(await screen.findByRole("button", { name: "分割バックアップを書き出す" }));
    await waitFor(() => expect(archive.exportAllMultipart).toHaveBeenCalledWith("C:\\Backups\\piep-2026-08-12.json"));
    expect(dialogs.saveDialog).toHaveBeenCalledWith(expect.objectContaining({
      filters: [{ name: "Piep multipart backup manifest", extensions: ["json"] }],
    }));
  });

  it("keeps the legacy single-ZIP restore path and its rollback guidance", async () => {
    const notification = vi.spyOn(notifications, "show");
    dialogs.openSingleDialog.mockResolvedValue("D:\\Archives\\piep-old.zip");
    archive.inspectBackupFile.mockResolvedValue({ format: "zip", inspection });
    archive.importBackupFile.mockRejectedValueOnce(new Error("injected restore failure"));
    renderLibrarySettings();

    fireEvent.click(await screen.findByRole("button", { name: "バックアップを復元" }));
    expect(await screen.findByText("選択したZIPバックアップ")).toBeInTheDocument();
    expect(screen.getByText("単一ZIP · v3.0")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "検証済みバックアップを復元" }));

    await waitFor(() => expect(archive.importBackupFile).toHaveBeenCalledWith("D:\\Archives\\piep-old.zip", "zip"));
    await waitFor(() => expect(notification).toHaveBeenCalledWith(expect.objectContaining({
      title: "復元を完了できませんでした",
      message: expect.stringContaining("復元ジャーナルにより元の状態へ戻されます"),
    })));
    expect(notification).not.toHaveBeenCalledWith(expect.objectContaining({ message: expect.stringContaining("JSONマニフェスト") }));
    notification.mockRestore();
  });

  it("gives actionable help when a multipart set cannot be inspected", async () => {
    const notification = vi.spyOn(notifications, "show");
    dialogs.openSingleDialog.mockResolvedValue("C:\\Backups\\missing.json");
    archive.inspectBackupFile.mockRejectedValue(new Error("part-0002.zip がありません"));
    renderLibrarySettings();

    fireEvent.click(await screen.findByRole("button", { name: "バックアップを復元" }));
    await waitFor(() => expect(notification).toHaveBeenCalledWith(expect.objectContaining({
      title: "バックアップを検査できませんでした",
      message: expect.stringContaining("同じフォルダーにすべてのZIPパートが必要です"),
    })));
    notification.mockRestore();
  });
});
