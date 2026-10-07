import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";

const mocks = vi.hoisted(() => ({ select: vi.fn(), items: vi.fn() }));
vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  listUpdateTargets: async () => [],
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
  return { ...original, useUpdateJobs: () => ({ jobs: [summary], activeSnapshot: { ...summary, logs: [], candidates: [], nextCandidateCursor: null, previousLogCursor: null }, selectJob: mocks.select }) };
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

it("opens the selected history run's result and its job-only holds", async () => {
  mocks.select.mockResolvedValue({ jobId: "job-history-review" });
  mocks.items.mockResolvedValue([
    { source: "fanbox", sourceId: "repost", itemType: "work", title: "再掲作品", status: "held", error: "公開元で投稿が見つからないため確認を保留しました" },
  ]);
  window.location.hash = "#/updates";
  render(<MantineProvider><QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><AppRouter><UpdatesPage /></AppRouter></QueryClientProvider></MantineProvider>);
  fireEvent.click(screen.getByRole("tab", { name: /履歴/ }));
  fireEvent.click(screen.getByRole("button", { name: /氷砂糖 · FANBOX · 作者 · 確認のみ/ }));
  await waitFor(() => expect(screen.getByRole("tab", { name: /結果/ })).toHaveAttribute("aria-selected", "true"));
  expect(mocks.select).toHaveBeenCalledWith("job-history-review");
  expect(await screen.findByText("再掲作品")).toBeInTheDocument();
});
