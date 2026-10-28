import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import DiagnosticsPage from "./DiagnosticsPage";

const api = vi.hoisted(() => ({ status: vi.fn(), cancel: vi.fn(), gaps: vi.fn() }));
vi.mock("@/services/dbApi", async (original) => ({
  ...await original<typeof import("@/services/dbApi")>(),
  isTauriRuntime: () => true,
  getEntityProfileRepairStatus: api.status,
  cancelEntityProfileRepair: api.cancel,
  scanFanboxAssetGaps: api.gaps,
}));
vi.mock("@/services/eventBus", () => ({ subscribeTauriEvent: () => () => undefined }));

beforeEach(() => {
  api.cancel.mockResolvedValue(undefined);
  api.gaps.mockResolvedValue({ workIds: [], missingFiles: 0, restrictedWorks: 0, scannedWorks: 0 });
});
const renderPage = () => render(<MantineProvider><QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><ModalsProvider><AppRouter><DiagnosticsPage /></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

it("resumes the running repair display when returning to the screen", async () => {
  api.status.mockResolvedValue({ personCount: 0, seriesCount: 221, totalCount: 221, running: true,
    progress: { phase: "running", completed: 15, total: 236, repaired: 15, failed: 0, activeLabel: "いま取得しているシリーズ", error: null } });
  renderPage();
  expect(await screen.findByText("221件 未取得")).toBeInTheDocument();
  expect(screen.getByText("確認中: いま取得しているシリーズ")).toBeInTheDocument();
  expect(screen.getByText("15 / 236")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "残件を修復" })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "中止" }));
  await waitFor(() => expect(api.cancel).toHaveBeenCalledOnce());
});

it("retains a paused result instead of presenting it as a completed repair", async () => {
  api.status.mockResolvedValue({ personCount: 0, seriesCount: 221, totalCount: 221, running: false,
    progress: { phase: "paused", completed: 16, total: 236, repaired: 15, failed: 1, activeLabel: null, error: "再接続が必要です" } });
  renderPage();
  expect(await screen.findByText("一時停止しました")).toBeInTheDocument();
  expect(screen.getByText("再接続が必要です")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "残件を修復" })).toBeEnabled();
  expect(screen.queryByText("今回の修復が完了しました")).not.toBeInTheDocument();
});

it("keeps attachment audit failures visible and allows retrying the local check", async () => {
  api.status.mockResolvedValue({ personCount: 0, seriesCount: 0, totalCount: 0, running: false, progress: null });
  api.gaps.mockRejectedValue(new Error("保存先を読み取れません"));
  renderPage();
  expect(await screen.findByText("保存先を読み取れません")).toBeInTheDocument();
  api.gaps.mockResolvedValue({ workIds: [], missingFiles: 0, restrictedWorks: 0, scannedWorks: 0 });
  fireEvent.click(screen.getByRole("button", { name: "再確認" }));
  await waitFor(() => expect(screen.queryByText("保存先を読み取れません")).not.toBeInTheDocument());
});
