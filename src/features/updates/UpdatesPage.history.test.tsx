import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";

const mocks = vi.hoisted(() => ({
  select: vi.fn(),
  items: vi.fn(),
  loadJobs: vi.fn().mockResolvedValue(undefined),
  shelfReady: false,
  emptyQueue: false,
}));
vi.mock("@tauri-apps/api/core", async (importOriginal) => ({
  ...await importOriginal<typeof import("@tauri-apps/api/core")>(),
  invoke: () => new Promise(() => {}),
}));
vi.mock("@/features/library/shelf", async (importOriginal) => ({
  ...await importOriginal<typeof import("@/features/library/shelf")>(),
  useShelfStatus: () => ({ data: mocks.shelfReady ? { state: "ready" } : undefined }),
}));
vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  listUpdateTargets: async () => [],
  listWatchGroups: async () => [],
}));
vi.mock("@/services/updateJobApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/updateJobApi")>()),
  listUpdateJobItemStatesCommand: mocks.items,
  countDismissedUpdateCandidatesCommand: async () => 0,
}));
vi.mock("@/features/updates/updateJobs", async (importOriginal) => {
  const original = await importOriginal<typeof import("@/features/updates/updateJobs")>();
  const summary = {
    jobId: "job-history-review", status: "completed", scope: "author", mode: "check_only",
    subjectLabel: "氷砂糖", subjectSource: "fanbox", totals: 20, processed: 20,
    candidateCount: 0, savedCount: 0, errorCount: 0, heldCount: 7,
    activeLabel: "完了しました", startedAt: "2026-10-07T10:32:14Z", updatedAt: "2026-10-07T10:32:25Z", finishedAt: "2026-10-07T10:32:25Z",
  };
  return { ...original, useUpdateJobs: () => ({
    jobs: mocks.emptyQueue ? [] : [summary],
    activeSnapshot: mocks.emptyQueue ? null : { ...summary, logs: [], candidates: [], nextCandidateCursor: null, previousLogCursor: null },
    selectJob: mocks.select,
    loadJobs: mocks.loadJobs,
  }) };
});
vi.mock("@/features/updates/useUpdateScheduler", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/features/updates/useUpdateScheduler")>()),
  useUpdateJobNotifications: () => {},
}));
vi.mock("@/features/updates/DeferredCandidatesPanel", () => ({ DeferredCandidatesPanel: () => null }));
vi.mock("@/features/updates/updateSchedule", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/features/updates/updateSchedule")>()),
  loadSchedule: async () => ({ enabled: false, mode: "check_only", watchSaved: false }),
}));
import UpdatesPage from "./UpdatesPage";

it("棚を開いたら現在の履歴を読み直し、空の履歴をすぐ表示する", async () => {
  mocks.shelfReady = true;
  mocks.emptyQueue = true;
  mocks.loadJobs.mockClear();
  window.location.hash = "#/updates";
  try {
    render(<MantineProvider><QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><AppRouter><UpdatesPage /></AppRouter></QueryClientProvider></MantineProvider>);
    fireEvent.click(screen.getByRole("tab", { name: /履歴/ }));
    expect(screen.getByText("履歴はまだありません")).toBeVisible();
    await waitFor(() => expect(mocks.loadJobs).toHaveBeenCalled());
  } finally {
    mocks.shelfReady = false;
    mocks.emptyQueue = false;
  }
});

it("opens the selected history run's result and its job-only holds", async () => {
  mocks.select.mockResolvedValue({ jobId: "job-history-review" });
  mocks.items.mockResolvedValue([
    { source: "fanbox", sourceId: "repost", itemType: "work", title: "再掲作品", status: "held", workId: "019ad000-0000-7000-8000-000000000001", error: "公開元で投稿が見つからないため確認を保留しました" },
  ]);
  window.location.hash = "#/updates";
  render(<MantineProvider><QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><AppRouter><UpdatesPage /></AppRouter></QueryClientProvider></MantineProvider>);
  fireEvent.click(screen.getByRole("tab", { name: /履歴/ }));
  fireEvent.click(screen.getByRole("button", { name: /氷砂糖 · FANBOX · 作者 · 確認のみ/ }));
  await waitFor(() => expect(screen.getByRole("tab", { name: /結果/ })).toHaveAttribute("aria-selected", "true"));
  expect(mocks.select).toHaveBeenCalledWith("job-history-review");
  expect(await screen.findByText("再掲作品")).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "再掲作品の保存した詳細を開く" }));
  expect(window.location.hash).toContain("/works/019ad000-0000-7000-8000-000000000001");
});
