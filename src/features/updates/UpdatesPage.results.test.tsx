import { MantineProvider } from "@mantine/core";
import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { JobResultsPanel } from "./UpdatesPage";
import type { UpdateJobItemState, UpdateJobSnapshot } from "@/services/updateJobApi";

const job = {
  jobId: "job-history-review", status: "completed", scope: "author", mode: "check_only",
  totals: 20, processed: 20, candidateCount: 0, savedCount: 0, errorCount: 0, heldCount: 7,
  startedAt: "2026-10-07T10:32:14Z", finishedAt: "2026-10-07T10:32:25Z",
} as UpdateJobSnapshot;

it("shows job-only missing posts in this run's results without sending users to deferred candidates", () => {
  const items: UpdateJobItemState[] = [
    ...Array.from({ length: 7 }, (_, index) => ({ source: "fanbox", sourceId: `repost-${index}`, itemType: "work", title: `再掲 ${index}`, status: "held", error: "公開元で投稿が見つからないため確認を保留しました" })),
    ...Array.from({ length: 12 }, (_, index) => ({ source: "fanbox", sourceId: `old-${index}`, itemType: "work", title: `旧投稿 ${index}`, status: "skipped", error: null })),
    { source: "fanbox", sourceId: "creator", itemType: "target", title: "氷砂糖", status: "done", error: null },
  ];
  const onShowLogs = vi.fn();
  render(<MantineProvider><JobResultsPanel job={job} subject="氷砂糖 · FANBOX · 作者 · 確認のみ" items={items} loading={false} error={null} onRetry={vi.fn()} onShowLogs={onShowLogs} onShowDeferred={vi.fn()} /></MantineProvider>);
  expect(screen.getByText("氷砂糖 · FANBOX · 作者 · 確認のみ")).toBeInTheDocument();
  expect(screen.getByText(/変更なし・スキップ 12件 · 公開元で見つからない 7件/)).toBeInTheDocument();
  expect(screen.getByText("この回で保留になった作品（7件）")).toBeInTheDocument();
  expect(screen.getAllByText("公開元で投稿が見つからないため確認を保留しました")).toHaveLength(7);
  expect(screen.queryByRole("button", { name: "保留・非表示の作品を開く" })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "すべてのログを見る" }));
  expect(onShowLogs).toHaveBeenCalledOnce();
});

it("shows a missing pixiv work as held with its source ID and no job error", () => {
  render(<MantineProvider><JobResultsPanel
    job={{ ...job, totals: 2, processed: 2, heldCount: 1 }}
    items={[
      { source: "pixiv", sourceId: "45168334", itemType: "target", title: "背徳亭無題", status: "done", error: null },
      { source: "pixiv", sourceId: "29225697", itemType: "work", title: "公開停止のお知らせ", status: "held", error: "公開元で投稿が見つからないため確認を保留しました。保存済みの作品ID 29225697 は保持しています" },
    ]}
    loading={false} error={null} onRetry={vi.fn()} onShowLogs={vi.fn()} onShowDeferred={vi.fn()}
  /></MantineProvider>);
  expect(screen.getByText(/公開元で見つからない 1件/)).toBeInTheDocument();
  expect(screen.getByText("公開停止のお知らせ")).toBeInTheDocument();
  expect(screen.getByText(/保存済みの作品ID 29225697 は保持しています/)).toBeInTheDocument();
  expect(screen.queryByText(/エラー 1件/)).not.toBeInTheDocument();
});

it("offers the deferred shelf only for an access restriction", () => {
  const onShowDeferred = vi.fn();
  render(<MantineProvider><JobResultsPanel
    job={{ ...job, totals: 1, processed: 1, heldCount: 1 }}
    items={[{ source: "fanbox", sourceId: "restricted", itemType: "work", title: "支援作品", status: "held", error: "[閲覧制限] 月額1000円以上" }]}
    loading={false} error={null} onRetry={vi.fn()} onShowLogs={vi.fn()} onShowDeferred={onShowDeferred}
  /></MantineProvider>);
  expect(screen.getByText(/閲覧条件待ち 1件/)).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "保留・非表示の作品を開く" }));
  expect(onShowDeferred).toHaveBeenCalledOnce();
  expect(screen.queryByText(/保存済みの内容は、手元に残っています/)).not.toBeInTheDocument();
});
