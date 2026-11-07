import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { OriginalImagesCard } from "./OriginalImagesCard";

const api = vi.hoisted(() => ({ status: vi.fn(), upgrade: vi.fn(), cancel: vi.fn(), listen: vi.fn() }));
vi.mock("@/services/dbApi", async (original) => ({
  ...await original<typeof import("@/services/dbApi")>(),
  getOriginalImageStatus: api.status,
  upgradePixivImagesToOriginal: api.upgrade,
  cancelOriginalImageUpgrade: api.cancel,
}));
vi.mock("@/services/eventBus", () => ({ subscribeTauriEvent: () => () => undefined, onTauriEvent: api.listen }));

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
  expect(screen.queryByText("pixiv の画像の画質")).not.toBeInTheDocument();
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

it("shows a running upgrade with a cancel button instead of a second start", async () => {
  api.status.mockResolvedValue({ covers: 20, illustrations: 0, running: true,
    progress: { phase: "running", completed: 5, total: 25, upgraded: 4, unavailable: 1, failed: 0, activeLabel: "いま取り直している作品", error: null } });
  renderCard();
  expect(await screen.findByText("確認中: いま取り直している作品")).toBeInTheDocument();
  expect(screen.getByText("5 / 25")).toBeInTheDocument();
  expect(screen.getByText("原寸 4枚 · 公開元に無し 1枚 · 失敗 0枚")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "原寸に取り直す" })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "中止" }));
  await waitFor(() => expect(api.cancel).toHaveBeenCalledOnce());
});
