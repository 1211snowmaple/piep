import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { SHELF_V2_PREFERENCE_KEY } from "@/features/library/shelfV2";
import type { SearchIndexStatus } from "@/types/library";
import DiagnosticsPage from "./DiagnosticsPage";

const status = vi.hoisted(() => ({ libraryV2Status: vi.fn(), libraryV2Search: vi.fn() }));
vi.mock("@/services/libraryV2Api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/libraryV2Api")>()),
  ...status,
}));
const health = vi.hoisted(() => ({
  libraryV2Diagnostics: vi.fn(),
  libraryV2Maintain: vi.fn(),
  libraryV2OptimizeSearchIndex: vi.fn(),
  libraryV2Reclaim: vi.fn(),
  libraryV2Verify: vi.fn(),
}));
vi.mock("@/services/shelfHealthApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/shelfHealthApi")>()),
  ...health,
}));
const legacy = vi.hoisted(() => ({
  scanFanboxAssetGaps: vi.fn(),
  getLibraryDiagnostics: vi.fn(),
  getEntityProfileRepairStatus: vi.fn(),
  getOriginalImageStatus: vi.fn(),
}));
vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  ...legacy,
}));

const index: SearchIndexStatus = {
  totalDownloads: 3,
  indexedDownloads: 3,
  pendingDownloads: 0,
  isComplete: true,
  phase: "ready",
  semanticIndexedChunks: 0,
  semanticIndexedDownloads: 0,
  semanticPendingDownloads: 0,
  semanticEnabled: false,
  semanticModelReady: false,
  embeddingProvider: "cpu",
  gpuEnabled: false,
};

function view() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <MantineProvider>
      <ModalsProvider>
        <QueryClientProvider client={client}>
          <AppRouter><DiagnosticsPage embedded /></AppRouter>
        </QueryClientProvider>
      </ModalsProvider>
    </MantineProvider>,
  );
}

describe("新しい棚の診断", () => {
  beforeEach(() => {
    localStorage.setItem(SHELF_V2_PREFERENCE_KEY, "true");
    status.libraryV2Status.mockResolvedValue({ state: "ready", path: "C:/shelf", unresolved: [], switched: true });
    status.libraryV2Search.mockResolvedValue({ page: { items: [], next: null, total: 2 }, intent: null, matches: [], cards: [] });
    legacy.getOriginalImageStatus.mockResolvedValue({ covers: 0, illustrations: 0, running: false, progress: null });
    legacy.getEntityProfileRepairStatus.mockResolvedValue({ personCount: 0, seriesCount: 0, totalCount: 0, running: false, progress: null });
    health.libraryV2Diagnostics.mockResolvedValue({
      measuredAt: "2026-11-20T00:00:00Z", works: 3, versions: 4, textLength: 120, media: 2, files: 9,
      logicalBytes: 9_000, storedBytes: 4_000, databaseBytes: 100_000, walBytes: 0, liveDatabaseBytes: 90_000,
      fragmentationPercent: 10, lexicalIndexBytes: 5_000, lexicalIndexSegments: 3, historyIndexBytes: 100,
      semanticIndexBytes: 0, processMemoryBytes: null, processPrivateMemoryBytes: null,
      gpuDedicatedMemoryBytes: null, gpuSharedMemoryBytes: null, searchIndex: index,
    });
    health.libraryV2Verify.mockResolvedValue(9);
    health.libraryV2OptimizeSearchIndex.mockResolvedValue({ optimized: true, beforeSegments: 3, afterSegments: 1, beforeBytes: 5_000, afterBytes: 4_000, reclaimedBytes: 1_000, elapsedMs: 3 });
  });
  afterEach(() => {
    localStorage.clear();
    vi.clearAllMocks();
  });

  it("棚の台帳と索引を数え、旧棚の保存フォルダーの照合は出さない", async () => {
    view();
    expect(await screen.findByText("3件")).toBeTruthy();
    expect(screen.getByText("3セグメント")).toBeTruthy();
    // 一部しか保存できていない作品は、一覧の絞り込みへ案内する。
    expect(await screen.findByText("2作品")).toBeTruthy();
    expect(legacy.scanFanboxAssetGaps).not.toHaveBeenCalled();
    expect(legacy.getLibraryDiagnostics).not.toHaveBeenCalled();
    expect(screen.queryByText("保存フォルダーから再取り込み")).toBeNull();
  });

  it("照合と索引の統合を棚に頼む", async () => {
    view();
    fireEvent.click(await screen.findByRole("button", { name: "照合する" }));
    await waitFor(() => expect(health.libraryV2Verify).toHaveBeenCalled());
    fireEvent.click(await screen.findByRole("button", { name: "検索索引を最適化" }));
    await waitFor(() => expect(health.libraryV2OptimizeSearchIndex).toHaveBeenCalled());
  });

  it("切り替える前の棚では書き換える手入れを押せず、理由を添える", async () => {
    status.libraryV2Status.mockResolvedValue({ state: "ready", path: "C:/shelf", unresolved: [], switched: false });
    view();
    const reclaim = await screen.findByRole("button", { name: "片付ける（新しい棚では変えられません）" });
    expect(reclaim.hasAttribute("disabled")).toBe(true);
    expect(screen.getByRole("button", { name: "照合する" }).hasAttribute("disabled")).toBe(false);
  });
});
