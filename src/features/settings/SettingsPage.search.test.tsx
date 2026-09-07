import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { theme } from "@/theme";

/**
 * 検索の設定。守るのは、**意味ベクトルが押しついでに消えないこと**である。
 *
 * この画面のスイッチは「その回だけの指定」ではなく機能そのものの入切で、切って
 * 作り直すと作った埋め込みを捨てる。9千件ぶんが消え、入れ直すには全作品の
 * 作り直しが要る。実際に消えたことがある。
 */
const db = vi.hoisted(() => ({ getSearchIndexStatus: vi.fn(), startSearchRebuildIndex: vi.fn() }));

vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  getStats: vi.fn().mockResolvedValue({ totalDownloads: 12, totalAssets: 1, totalSizeBytes: 1 }),
  getSearchIndexStatus: db.getSearchIndexStatus,
  scanAndReimportDownloads: vi.fn().mockResolvedValue({ imported: 0, skipped: [] }),
}));
vi.mock("@/services/searchApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/searchApi")>()),
  startSearchRebuildIndex: db.startSearchRebuildIndex,
  cancelSearchRebuildIndex: vi.fn(),
}));
vi.mock("@/services/archiveApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/archiveApi")>()),
  getStoragePath: vi.fn().mockResolvedValue("C:\\piep\\downloads"),
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

function status(overrides: Record<string, unknown> = {}) {
  return {
    totalDownloads: 9370,
    indexedDownloads: 9370,
    pendingDownloads: 0,
    isComplete: true,
    phase: "ready",
    semanticEnabled: true,
    semanticIndexedChunks: 30_000,
    semanticIndexedDownloads: 9364,
    semanticPendingDownloads: 6,
    semanticModelReady: true,
    embeddingProvider: "DirectML",
    gpuEnabled: true,
    throughputPerSec: null,
    ...overrides,
  };
}

function renderSearchSettings() {
  window.location.hash = "#/settings?section=search";
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(
    <MantineProvider theme={theme}>
      <QueryClientProvider client={client}>
        <ModalsProvider><AppRouter><SettingsPage /></AppRouter></ModalsProvider>
      </QueryClientProvider>
    </MantineProvider>,
  );
}

describe("SettingsPage の検索", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    db.startSearchRebuildIndex.mockResolvedValue("job-1");
  });

  it("意味検索を切る作り直しは、捨てる件数を見せて確認を取る", async () => {
    db.getSearchIndexStatus.mockResolvedValue(status());
    renderSearchSettings();

    await userEvent.click(await screen.findByRole("switch", { name: /意味検索を使う/ }));
    await userEvent.click(screen.getByRole("button", { name: /インデックスを再構築/ }));

    // 確認を出すまでは何も始めない。
    expect(db.startSearchRebuildIndex).not.toHaveBeenCalled();
    expect(await screen.findByText(/9,364件ぶんの意味ベクトルを削除します/)).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "無効にして作り直す" }));
    await waitFor(() => expect(db.startSearchRebuildIndex).toHaveBeenCalledWith({ includeSemantic: false }));
  });

  it("やめれば、意味ベクトルは消えない", async () => {
    db.getSearchIndexStatus.mockResolvedValue(status());
    renderSearchSettings();

    await userEvent.click(await screen.findByRole("switch", { name: /意味検索を使う/ }));
    await userEvent.click(screen.getByRole("button", { name: /インデックスを再構築/ }));
    await userEvent.click(await screen.findByRole("button", { name: "やめる" }));

    expect(db.startSearchRebuildIndex).not.toHaveBeenCalled();
  });

  it("入れたままの作り直しは、確認を挟まない", async () => {
    db.getSearchIndexStatus.mockResolvedValue(status());
    renderSearchSettings();

    await userEvent.click(await screen.findByRole("button", { name: /インデックスを再構築/ }));
    await waitFor(() => expect(db.startSearchRebuildIndex).toHaveBeenCalledWith({ includeSemantic: true }));
  });

  /**
   * 状態が届く前の `includeSemantic` の既定は「切」なので、その間に押せると
   * 切る側へ倒れる。**画面は届くまで作り直しを出さない。** ボタン側にも
   * `!status` の錠を掛けてあるが、そちらは二重の備えである。
   */
  it("状態が届くまで、作り直しの操作を出さない", async () => {
    let settle: (value: unknown) => void = () => undefined;
    db.getSearchIndexStatus.mockImplementation(() => new Promise((resolve) => { settle = resolve; }));
    renderSearchSettings();

    expect(await screen.findByText("検索インデックスを確認しています")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /インデックスを再構築|未反映分を索引する/ })).toBeNull();
    expect(screen.queryByRole("switch", { name: /意味検索を使う/ })).toBeNull();

    settle(status());
    await waitFor(() => expect(screen.getByRole("button", { name: /インデックスを再構築/ })).toBeEnabled());
  });
});
