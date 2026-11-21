import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { useWorkspace, WorkspaceProvider } from "@/app/WorkspaceContext";
import { clearCompletedOperations, getOperationJobs, requestOperationCancel, retryOperation } from "@/features/jobs/operationJobs";
import { demoWorks } from "@/mocks/demoData";
import { readExportSettings, writeExportSettings } from "./exportSettings";
import { demoTemplates } from "./templateStudioDemo";
import type { ExportBatchResult } from "@/types/epub";
import EpubPage from "./EpubPage";

const api = vi.hoisted(() => ({ exportEpubBatch: vi.fn(), cancelEpubExport: vi.fn() }));
const progressSubscription = vi.hoisted(() => ({ listen: vi.fn(), unlisten: vi.fn() }));
const opener = vi.hoisted(() => ({ openFilesystemPath: vi.fn() }));
vi.mock("@/services/dbApi", async (original) => ({
  ...(await original<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
}));
vi.mock("@/services/epubApi", () => ({
  exportEpubBatch: api.exportEpubBatch,
  listEpubTemplates: async () => demoTemplates,
  cancelEpubExport: api.cancelEpubExport,
}));
vi.mock("@/services/eventBus", () => ({
  subscribeTauriEvent: () => () => undefined,
  onTauriEvent: async (event: string, handler: unknown) => {
    progressSubscription.listen(event, handler);
    return progressSubscription.unlisten;
  },
}));
vi.mock("@/services/openerApi", () => ({ openFilesystemPath: opener.openFilesystemPath }));
const shelf = vi.hoisted(() => ({ byIds: vi.fn() }));
/** 棚の作品は UUID で積まれる。見本の作品を `w-番号` の ID で棚の作品として返す。 */
async function shelfWorks(ids: string[]) {
  return ids.flatMap((id) => {
    const work = demoWorks.find((item) => `w-${item.id}` === id);
    return work ? [{ ...work, id, shelf: "v2" as const }] : [];
  });
}
vi.mock("@/features/library/shelfV2", async (original) => ({
  ...(await original<typeof import("@/features/library/shelfV2")>()),
  shelfEntriesByIds: shelf.byIds,
}));

function QueueProbe() {
  const { epubQueue, addToEpubQueue } = useWorkspace();
  return <><button onClick={() => addToEpubQueue("w-103")}>次の作品を追加</button><output aria-label="書き出し待ち">{epubQueue.join(",")}</output></>;
}

beforeEach(() => {
  clearCompletedOperations();
  window.localStorage.clear();
  window.localStorage.setItem("piep.epub-queue.v2", JSON.stringify(["w-101", "w-108"]));
  shelf.byIds.mockReset().mockImplementation(shelfWorks);
  window.location.hash = "#/epub";
  writeExportSettings({ ...readExportSettings(), outputDir: "C:/exports" });
  api.exportEpubBatch.mockReset();
  api.cancelEpubExport.mockReset();
  api.cancelEpubExport.mockResolvedValue(undefined);
  progressSubscription.listen.mockReset();
  progressSubscription.unlisten.mockReset();
  opener.openFilesystemPath.mockReset();
});

it("reports an output folder that can no longer be opened", async () => {
  api.exportEpubBatch.mockResolvedValue({ successCount: 1, failedCount: 1, failedIds: ["w-108"], invalidIds: [], outputFiles: ["C:/exports/101.epub"], invalidCount: 0, issues: [], canceled: false, skippedIds: [] } satisfies ExportBatchResult);
  opener.openFilesystemPath.mockRejectedValue(new Error("folder missing"));
  const show = vi.spyOn(notifications, "show");
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<MantineProvider><ModalsProvider><QueryClientProvider client={client}><AppRouter><WorkspaceProvider><EpubPage /></WorkspaceProvider></AppRouter></QueryClientProvider></ModalsProvider></MantineProvider>);

  fireEvent.click(await screen.findByRole("button", { name: "2冊を書き出す" }));
  fireEvent.click(await screen.findByRole("button", { name: "出力先を開く" }));
  await waitFor(() => expect(show).toHaveBeenCalledWith(expect.objectContaining({ color: "red", title: "出力先を開けません", message: "folder missing" })));
  expect(opener.openFilesystemPath).toHaveBeenCalledWith("C:/exports");
  show.mockRestore();
});

it("keeps the successful export result visible after the queue empties", async () => {
  api.exportEpubBatch.mockResolvedValue({ successCount: 2, failedCount: 0, failedIds: [], invalidIds: [], outputFiles: ["C:/exports/101.epub", "C:/exports/108.epub"], invalidCount: 0, issues: [], canceled: false, skippedIds: [] } satisfies ExportBatchResult);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<MantineProvider><ModalsProvider><QueryClientProvider client={client}><AppRouter><WorkspaceProvider><EpubPage /></WorkspaceProvider></AppRouter></QueryClientProvider></ModalsProvider></MantineProvider>);

  fireEvent.click(await screen.findByRole("button", { name: "2冊を書き出す" }));
  expect(await screen.findByText("EPUBキューは空です")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "出力先を開く" })).toBeInTheDocument();
});

it("marks failed EPUB batches as failed and retries only unfinished works", async () => {
  api.exportEpubBatch
    .mockResolvedValueOnce({ successCount: 1, failedCount: 1, failedIds: ["w-108"], invalidIds: [], outputFiles: ["C:/exports/101.epub"], invalidCount: 0, issues: [], canceled: false, skippedIds: [] } satisfies ExportBatchResult)
    .mockResolvedValueOnce({ successCount: 1, failedCount: 0, failedIds: [], invalidIds: [], outputFiles: ["C:/exports/108.epub"], invalidCount: 0, issues: [], canceled: false, skippedIds: [] } satisfies ExportBatchResult);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<MantineProvider><ModalsProvider><QueryClientProvider client={client}><AppRouter><WorkspaceProvider><QueueProbe /><EpubPage /></WorkspaceProvider></AppRouter></QueryClientProvider></ModalsProvider></MantineProvider>);

  fireEvent.click(await screen.findByRole("button", { name: "2冊を書き出す" }));
  expect(await screen.findByText("書き出せない作品がありました")).toBeInTheDocument();
  expect(screen.getByText(/書き出せなかった作品はキューに残っています/)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "出力先を開く" })).toBeInTheDocument();
  const failedJob = getOperationJobs().find((job) => job.kind === "epub" && job.label === "2冊をEPUBへ書き出し");
  expect(failedJob).toMatchObject({ status: "failed", canRetry: true });
  expect(screen.getByLabelText("書き出し待ち")).toHaveTextContent(/^w-108$/);

  await act(async () => retryOperation(failedJob!.id));
  await waitFor(() => expect(api.exportEpubBatch).toHaveBeenCalledWith(expect.objectContaining({ downloadIds: ["w-108"] })));
  await waitFor(() => expect(screen.getByLabelText("書き出し待ち")).toHaveTextContent(/^$/));
  expect(getOperationJobs().find((job) => job.id === failedJob!.id)).toMatchObject({ status: "failed", canRetry: false });
  expect(getOperationJobs().find((job) => job.kind === "epub" && job.label === "1冊をEPUBへ書き出し")).toMatchObject({ status: "completed" });
  expect(progressSubscription.listen).toHaveBeenCalledTimes(2);
  expect(progressSubscription.unlisten).toHaveBeenCalledTimes(2);
});

it("uses the same operation ID for EPUB start and cancel", async () => {
  let finish!: (result: ExportBatchResult) => void;
  api.exportEpubBatch.mockReturnValue(new Promise<ExportBatchResult>((resolve) => { finish = resolve; }));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<MantineProvider><ModalsProvider><QueryClientProvider client={client}><AppRouter><WorkspaceProvider><EpubPage /></WorkspaceProvider></AppRouter></QueryClientProvider></ModalsProvider></MantineProvider>);

  fireEvent.click(await screen.findByRole("button", { name: "2冊を書き出す" }));
  await waitFor(() => expect(api.exportEpubBatch).toHaveBeenCalledOnce());
  const job = getOperationJobs().find((entry) => entry.kind === "epub" && entry.status === "running");
  expect(job).toBeDefined();
  expect(api.exportEpubBatch).toHaveBeenCalledWith(expect.objectContaining({ operationId: job!.id }));

  await act(async () => requestOperationCancel(job!.id));
  expect(api.cancelEpubExport).toHaveBeenCalledWith(job!.id);

  await act(async () => finish({
    successCount: 0, failedCount: 0, failedIds: [], invalidIds: [], outputFiles: [],
    invalidCount: 0, issues: [], canceled: true, skippedIds: ["w-101", "w-108"],
  }));
  await waitFor(() => expect(getOperationJobs().find((entry) => entry.id === job!.id)?.status).toBe("canceled"));
});

it("does not suggest opening an output folder when every EPUB fails", async () => {
  api.exportEpubBatch.mockResolvedValue({ successCount: 0, failedCount: 2, failedIds: ["w-101", "w-108"], invalidIds: [], outputFiles: [], invalidCount: 0, issues: [], canceled: false, skippedIds: [] } satisfies ExportBatchResult);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<MantineProvider><ModalsProvider><QueryClientProvider client={client}><AppRouter><WorkspaceProvider><EpubPage /></WorkspaceProvider></AppRouter></QueryClientProvider></ModalsProvider></MantineProvider>);

  fireEvent.click(await screen.findByRole("button", { name: "2冊を書き出す" }));
  expect(await screen.findByText("書き出せない作品がありました")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "出力先を開く" })).not.toBeInTheDocument();
});

it.each(["success", "failed", "invalid", "canceled"])("keeps later additions and unfinished works after a %s export", async (outcome) => {
  let finish!: (result: ExportBatchResult) => void;
  api.exportEpubBatch.mockReturnValue(new Promise<ExportBatchResult>((resolve) => { finish = resolve; }));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<MantineProvider><ModalsProvider><QueryClientProvider client={client}><AppRouter><WorkspaceProvider><QueueProbe /><EpubPage /></WorkspaceProvider></AppRouter></QueryClientProvider></ModalsProvider></MantineProvider>);
  fireEvent.click(await screen.findByRole("button", { name: "2冊を書き出す" }));
  await waitFor(() => expect(api.exportEpubBatch).toHaveBeenCalledOnce());
  expect(api.exportEpubBatch).toHaveBeenCalledWith(expect.objectContaining({ downloadIds: ["w-101", "w-108"] }));

  fireEvent.click(screen.getByRole("button", { name: "次の作品を追加" }));
  await screen.findByRole("button", { name: "3冊を書き出す" });
  await act(async () => finish({
    successCount: outcome === "success" ? 2 : 1,
    failedCount: outcome === "failed" ? 1 : 0,
    failedIds: outcome === "failed" ? ["w-108"] : [],
    invalidIds: outcome === "invalid" ? ["w-108"] : [],
    outputFiles: ["C:/exports/101.epub"],
    invalidCount: outcome === "invalid" ? 1 : 0,
    issues: [], canceled: outcome === "canceled", skippedIds: outcome === "canceled" ? ["w-108"] : [],
  }));

  await waitFor(() => expect(screen.getByLabelText("書き出し待ち")).toHaveTextContent(outcome === "success" ? /^w-103$/ : /^w-108,w-103$/));
});

it("exports works of the shelf by their id, in queue order, and keeps a failed one queued", async () => {
  window.localStorage.setItem("piep.epub-queue.v2", JSON.stringify(["w-101", "w-1", "w-108"]));
  shelf.byIds.mockImplementation(async (ids: string[]) => [...await shelfWorks(ids), { ...demoWorks[0], id: "w-1", shelf: "v2" as const, title: "棚の一冊" }]);
  api.exportEpubBatch.mockResolvedValue({ successCount: 2, failedCount: 1, failedIds: ["w-1"], invalidIds: [], outputFiles: ["C:/exports/101.epub", "C:/exports/108.epub"], invalidCount: 0, issues: [], canceled: false, skippedIds: [] } satisfies ExportBatchResult);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<MantineProvider><ModalsProvider><QueryClientProvider client={client}><AppRouter><WorkspaceProvider><EpubPage /><QueueProbe /></WorkspaceProvider></AppRouter></QueryClientProvider></ModalsProvider></MantineProvider>);

  fireEvent.click(await screen.findByRole("button", { name: "3冊を書き出す" }));
  await waitFor(() => expect(api.exportEpubBatch).toHaveBeenCalledWith(expect.objectContaining({ downloadIds: ["w-101", "w-1", "w-108"] })));
  expect(shelf.byIds).toHaveBeenCalledWith(["w-101", "w-1", "w-108"]);
  // 書けなかった棚の作品だけがキューに残る。
  await waitFor(() => expect(screen.getByLabelText("書き出し待ち")).toHaveTextContent(/^w-1$/));
});
