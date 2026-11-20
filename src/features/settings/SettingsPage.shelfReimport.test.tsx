import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { theme } from "@/theme";

const shelf = vi.hoisted(() => ({ switched: true }));
const reimport = vi.hoisted(() => ({
  shelf: vi.fn().mockResolvedValue({ imported: 2, skipped: ["pixiv/3（読めません）"] }),
  old: vi.fn().mockResolvedValue({ imported: 0, skipped: [] }),
}));
const operationSpies = vi.hoisted(() => ({ complete: vi.fn(), log: vi.fn() }));

vi.mock("@/features/library/shelfV2", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/features/library/shelfV2")>()),
  useShelfMode: () => ({ shelfMode: true, deciding: false }),
  useLibraryV2Status: () => ({ data: { state: "ready", path: "C:\\shelf", unresolved: [], switched: shelf.switched } }),
}));
vi.mock("@/services/shelfHealthApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/shelfHealthApi")>()),
  libraryV2Dashboard: vi.fn().mockResolvedValue({ stats: { totalDownloads: 12, totalAssets: 108, totalSizeBytes: 32_000_000 } }),
}));
vi.mock("@/services/shelfBackupApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/shelfBackupApi")>()),
  libraryV2RestoreState: vi.fn().mockResolvedValue({ pending: null, last: null }),
  libraryV2ReimportFolders: reimport.shelf,
}));
vi.mock("@/services/archiveApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/archiveApi")>()),
  backupExportRunning: vi.fn().mockResolvedValue(false),
}));
vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  getSearchIndexStatus: vi.fn().mockResolvedValue({ totalDownloads: 12, indexedDownloads: 12, pendingDownloads: 0, isComplete: true, phase: "ready", semanticIndexedChunks: 0, semanticIndexedDownloads: 0, semanticPendingDownloads: 12, semanticModelReady: false, embeddingProvider: "CPU", gpuEnabled: false, throughputPerSec: null }),
  scanAndReimportDownloads: reimport.old,
}));
vi.mock("@/store", () => ({
  store: { get: vi.fn().mockResolvedValue(null), set: vi.fn(), delete: vi.fn(), save: vi.fn() },
}));
vi.mock("@/features/search/searchIndexProgress", () => ({ useSearchIndexProgress: () => null }));
vi.mock("@/features/jobs/operationJobs", () => ({
  useOperationJobs: () => [],
  startOperation: () => ({
    id: "test-operation",
    progress: vi.fn(),
    log: operationSpies.log,
    complete: operationSpies.complete,
    fail: vi.fn(),
    cancel: vi.fn(),
    isCancelRequested: () => false,
  }),
  requestOperationCancel: vi.fn(),
}));

import SettingsPage from "./SettingsPage";

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

describe("SettingsPage folder reimport on the new shelf", () => {
  beforeEach(() => {
    shelf.switched = true;
    reimport.shelf.mockClear();
    reimport.old.mockClear();
    operationSpies.complete.mockClear();
    operationSpies.log.mockClear();
  });

  it("brings save folders into the switched shelf and says what it skipped", async () => {
    renderLibrarySettings();
    const button = await screen.findByRole("button", { name: "保存フォルダーを調べる" });
    expect(button).not.toHaveAttribute("aria-disabled", "true");
    fireEvent.click(button);

    await waitFor(() => expect(reimport.shelf).toHaveBeenCalledTimes(1));
    expect(reimport.old).not.toHaveBeenCalled();
    await waitFor(() => expect(operationSpies.complete).toHaveBeenCalledWith("2件を再取り込みし、1件は読めずに飛ばしました"));
    expect(operationSpies.log).toHaveBeenCalledWith("pixiv/3（読めません）", "warn");
  });

  it("keeps the scan locked with the reason while the shelf is not switched", async () => {
    shelf.switched = false;
    renderLibrarySettings();
    const button = await screen.findByRole("button", { name: "保存フォルダーを調べる（新しい棚では変えられません）" });
    expect(button).toHaveAttribute("aria-disabled", "true");
    fireEvent.click(button);
    expect(reimport.shelf).not.toHaveBeenCalled();
    expect(reimport.old).not.toHaveBeenCalled();
  });
});
