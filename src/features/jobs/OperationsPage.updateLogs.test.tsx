import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { UpdateJobSnapshot } from "@/services/updateJobApi";

const mocks = vi.hoisted(() => ({
  getJob: vi.fn(),
  selectJob: vi.fn(),
  loadJobs: vi.fn().mockResolvedValue(undefined),
}));

const latest: UpdateJobSnapshot = {
  jobId: "save-history-1",
  status: "completed",
  scope: "save",
  mode: "save",
  totals: 2,
  processed: 2,
  candidateCount: 0,
  savedCount: 2,
  errorCount: 0,
  activeLabel: "完了しました",
  startedAt: "2026-08-29T00:00:00Z",
  updatedAt: "2026-08-29T00:01:00Z",
  finishedAt: "2026-08-29T00:01:00Z",
  logs: [
    {
      id: 6,
      logType: "success",
      message: "新しいログ",
      createdAt: "2026-08-29T00:01:00Z",
    },
  ],
  candidates: [],
  nextCandidateCursor: null,
  previousLogCursor: 5,
};

vi.mock("@/services/dbApi", () => ({ isTauriRuntime: () => true }));
// 差し替えるのは**画面が呼ぶ入口だけ**にする。状態の見分け方や色の対応表まで
// ここへ書き写すと、本体を直したときにテストだけが古い答えを持ちつづける。
vi.mock("@/features/updates/updateJobs", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/features/updates/updateJobs")>()),
  useUpdateJobs: () => ({
    jobs: [latest],
    activeSnapshot: null,
    loadJobs: mocks.loadJobs,
    selectJob: mocks.selectJob,
    pause: vi.fn(),
    resume: vi.fn(),
    cancel: vi.fn(),
  }),
}));
vi.mock("@/services/updateJobApi", () => ({
  clearFinishedUpdateJobsCommand: vi.fn().mockResolvedValue(undefined),
  getUpdateJobCommand: mocks.getJob,
}));
vi.mock("./operationJobs", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./operationJobs")>()),
  useOperationJobs: () => [{
    id: "profile-repair",
    kind: "maintenance",
    label: "作者・シリーズ情報を修復",
    detail: "残件を確認しています",
    status: "running",
    current: 28,
    total: 102,
    startedAt: "2026-08-28T23:00:00Z",
    updatedAt: "2026-08-28T23:30:00Z",
    finishedAt: null,
    canCancel: true,
    canRetry: false,
    logs: [],
  }],
}));

import OperationsPage from "./OperationsPage";

describe("OperationsPage update-job logs", () => {
  it("shows active maintenance before older persisted save jobs", () => {
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    render(
      <MantineProvider>
        <QueryClientProvider client={client}>
          <OperationsPage />
        </QueryClientProvider>
      </MantineProvider>,
    );

    const repair = screen.getByTestId("operation-maintenance");
    const save = screen.getByText("2件をライブラリに保存").closest(".mantine-Card-root");
    expect(save).not.toBeNull();
    expect(repair.compareDocumentPosition(save!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("shows persistent job logs and pages backward beyond the first page", async () => {
    mocks.selectJob.mockResolvedValue(latest);
    mocks.getJob.mockResolvedValue({
      ...latest,
      logs: [
        {
          id: 4,
          logType: "warn",
          message: "以前のログ",
          createdAt: "2026-08-28T23:59:00Z",
        },
      ],
      previousLogCursor: null,
    });
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    render(
      <MantineProvider>
        <QueryClientProvider client={client}>
          <OperationsPage />
        </QueryClientProvider>
      </MantineProvider>,
    );

    fireEvent.click(screen.getByRole("button", { name: "ログ" }));
    expect(await screen.findByText("新しいログ")).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: "以前のログを読み込む" }),
    );
    expect(await screen.findByText("以前のログ")).toBeInTheDocument();
    expect(mocks.getJob).toHaveBeenCalledWith("save-history-1", null, 5);
  });
});
