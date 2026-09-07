import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { useWorkspace, WorkspaceProvider } from "@/app/WorkspaceContext";
import { demoWorks } from "@/mocks/demoData";
import { readExportSettings, writeExportSettings } from "./exportSettings";
import { demoTemplates } from "./templateStudioDemo";
import type { ExportBatchResult } from "@/types/epub";
import EpubPage from "./EpubPage";

const api = vi.hoisted(() => ({ exportEpubBatch: vi.fn() }));
vi.mock("@/services/dbApi", async (original) => ({
  ...(await original<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  getDownloads: async (ids: number[]) => demoWorks.filter((work) => ids.includes(work.id)),
}));
vi.mock("@/services/epubApi", () => ({
  exportEpubBatch: api.exportEpubBatch,
  listEpubTemplates: async () => demoTemplates,
  cancelEpubExport: vi.fn(),
}));
vi.mock("@/services/eventBus", () => ({ subscribeTauriEvent: () => () => undefined }));

function QueueProbe() {
  const { epubQueue, addToEpubQueue } = useWorkspace();
  return <><button onClick={() => addToEpubQueue(103)}>次の作品を追加</button><output aria-label="書き出し待ち">{epubQueue.join(",")}</output></>;
}

beforeEach(() => {
  window.localStorage.clear();
  window.localStorage.setItem("piep.epub-queue.v2", "[101,108]");
  window.location.hash = "#/epub";
  writeExportSettings({ ...readExportSettings(), outputDir: "C:/exports" });
  api.exportEpubBatch.mockReset();
});

it.each(["success", "failed", "invalid", "canceled"])("keeps later additions and unfinished works after a %s export", async (outcome) => {
  let finish!: (result: ExportBatchResult) => void;
  api.exportEpubBatch.mockReturnValue(new Promise<ExportBatchResult>((resolve) => { finish = resolve; }));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<MantineProvider><ModalsProvider><QueryClientProvider client={client}><AppRouter><WorkspaceProvider><QueueProbe /><EpubPage /></WorkspaceProvider></AppRouter></QueryClientProvider></ModalsProvider></MantineProvider>);
  fireEvent.click(await screen.findByRole("button", { name: "2冊を書き出す" }));
  await waitFor(() => expect(api.exportEpubBatch).toHaveBeenCalledOnce());
  expect(api.exportEpubBatch).toHaveBeenCalledWith(expect.objectContaining({ downloadIds: [101, 108] }));

  fireEvent.click(screen.getByRole("button", { name: "次の作品を追加" }));
  await screen.findByRole("button", { name: "3冊を書き出す" });
  await act(async () => finish({
    successCount: outcome === "success" ? 2 : 1,
    failedCount: outcome === "failed" ? 1 : 0,
    failedIds: outcome === "failed" ? [108] : [],
    invalidIds: outcome === "invalid" ? [108] : [],
    outputFiles: ["C:/exports/101.epub"],
    invalidCount: outcome === "invalid" ? 1 : 0,
    issues: [], canceled: outcome === "canceled", skippedIds: outcome === "canceled" ? [108] : [],
  }));

  await waitFor(() => expect(screen.getByLabelText("書き出し待ち")).toHaveTextContent(outcome === "success" ? /^103$/ : /^108,103$/));
});
