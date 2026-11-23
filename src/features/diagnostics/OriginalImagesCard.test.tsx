import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { OriginalImagesCard } from "./OriginalImagesCard";

const api = vi.hoisted(() => ({ status: vi.fn(), upgrade: vi.fn(), cancel: vi.fn(), listen: vi.fn() }));
vi.mock("@/services/dbApi", async (original) => ({
  ...await original<typeof import("@/services/dbApi")>(),
  cancelOriginalImageUpgrade: api.cancel,
}));
vi.mock("@/services/eventBus", () => ({ subscribeTauriEvent: () => () => undefined, onTauriEvent: api.listen }));
vi.mock("@/services/shelfHealthApi", async (original) => ({
  ...await original<typeof import("@/services/shelfHealthApi")>(),
  shelfOriginalImageStatus: api.status,
  shelfUpgradeImagesToOriginal: api.upgrade,
}));

beforeEach(() => {
  vi.clearAllMocks();
  api.cancel.mockResolvedValue(true);
  api.listen.mockResolvedValue(() => undefined);
});
const renderCard = () => render(<MantineProvider><QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><AppRouter><OriginalImagesCard runtime disabled={false} /></AppRouter></QueryClientProvider></MantineProvider>);

it("stays out of the way once every image is the original", async () => {
  api.status.mockResolvedValue({ covers: 0, illustrations: 0, running: false, progress: null });
  renderCard();
  await waitFor(() => expect(api.status).toHaveBeenCalled());
  expect(screen.queryByText("画像の画質")).not.toBeInTheDocument();
});

it("counts the reduced images and starts the upgrade", async () => {
  api.status.mockResolvedValue({ covers: 26168, illustrations: 443, running: false, progress: null });
  api.upgrade.mockResolvedValue({ attempted: 26611, upgraded: 26500, unavailable: 111, failed: 0, canceled: false, stoppedReason: null, remaining: 111 });
  renderCard();
  expect(await screen.findByText("26,611枚 縮小版")).toBeInTheDocument();
  expect(screen.getByText("表紙 26,168 · 挿絵 443")).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "原寸に取り直す" }));
  await waitFor(() => expect(api.upgrade).toHaveBeenCalledOnce());
});

it("names the works that stayed reduced, with the reason for a failure", async () => {
  api.status.mockResolvedValue({ covers: 2, illustrations: 0, running: false,
    progress: { phase: "complete", completed: 10, total: 10, upgraded: 8, unavailable: 1, failed: 1, activeLabel: null, error: null, misses: [
      { workId: "w1", title: "消えた投稿", provider: "fanbox", externalKey: "1", unavailable: 1, failed: 0, reason: null },
      { workId: "w2", title: "落ちた投稿", provider: "fanbox", externalKey: "2", unavailable: 0, failed: 1, reason: "保存できませんでした" },
    ] } });
  renderCard();
  expect(await screen.findByText("原寸にできなかった作品")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "消えた投稿" })).toBeInTheDocument();
  expect(screen.getByText("公開元に無し")).toBeInTheDocument();
  expect(screen.getByText("保存できませんでした")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "失敗の詳細" })).not.toBeInTheDocument();
});

it("shows a running upgrade with a cancel button instead of a second start", async () => {
  api.status.mockResolvedValue({ covers: 20, illustrations: 0, running: true,
    progress: { phase: "running", completed: 5, total: 25, upgraded: 4, unavailable: 1, failed: 0, activeLabel: "いま取り直している作品", error: null, misses: [] } });
  renderCard();
  expect(await screen.findByText("確認中: いま取り直している作品")).toBeInTheDocument();
  expect(screen.getByText("5 / 25")).toBeInTheDocument();
  expect(screen.getByText("原寸 4枚 · 公開元に無し 1枚 · 失敗 0枚")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "原寸に取り直す" })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "中止" }));
  await waitFor(() => expect(api.cancel).toHaveBeenCalledOnce());
});
