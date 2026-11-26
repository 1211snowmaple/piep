import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { theme } from "@/theme";

const authApi = vi.hoisted(() => ({
  getConnectionStatus: vi.fn(),
  disconnectAccount: vi.fn(),
  activateAccount: vi.fn(),
  loginPixivWebview: vi.fn(),
  connectPixivToken: vi.fn(),
  loginFanboxWebview: vi.fn(),
  connectFanboxSession: vi.fn(),
}));

vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  getSearchIndexStatus: vi.fn().mockResolvedValue({ totalWorks: 12, indexedWorks: 12, pendingWorks: 0, isComplete: true, phase: "ready", semanticIndexedChunks: 0, semanticIndexedWorks: 0, semanticPendingWorks: 12, semanticModelReady: false, embeddingProvider: "CPU", gpuEnabled: false, throughputPerSec: null }),
}));
vi.mock("@/services/shelfApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/shelfApi")>()),
  shelfStatus: vi.fn().mockResolvedValue({ state: "ready", path: "C:/piep/library" }),
}));
vi.mock("@/services/shelfHealthApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/shelfHealthApi")>()),
  shelfDashboard: vi.fn().mockResolvedValue({ stats: { totalWorks: 12, pixivCount: 12, fanboxCount: 0, totalAssets: 108, totalSizeBytes: 32_000_000 } }),
}));
vi.mock("@/services/authApi", () => authApi);
vi.mock("@/features/search/searchIndexProgress", () => ({ useSearchIndexProgress: () => null }));

import SettingsPage from "./SettingsPage";

function renderConnectionSettings() {
  window.location.hash = "#/settings";
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return render(
    <MantineProvider theme={theme}>
      <QueryClientProvider client={client}>
        <ModalsProvider><AppRouter><SettingsPage /></AppRouter></ModalsProvider>
      </QueryClientProvider>
    </MantineProvider>,
  );
}

describe("SettingsPage connection flow", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    authApi.getConnectionStatus.mockResolvedValue({ pixiv: null, fanbox: { name: "FANBOX利用者" }, accounts: { pixiv: [], fanbox: [{ key: "f1", name: "FANBOX利用者", active: true }] } });
    authApi.disconnectAccount.mockResolvedValue({ pixiv: null, fanbox: null, accounts: { pixiv: [], fanbox: [] } });
  });

  it("asks the backend to forget the FANBOX connection", async () => {
    renderConnectionSettings();

    fireEvent.click(await screen.findByRole("button", { name: "FANBOX利用者の接続を解除" }));
    expect(await screen.findByRole("dialog", { name: "FANBOX利用者の接続を解除しますか？" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "接続を解除" }));

    await waitFor(() => expect(authApi.disconnectAccount).toHaveBeenCalledWith("fanbox", "f1"));
    await waitFor(() => expect(screen.queryByRole("button", { name: "FANBOX利用者の接続を解除" })).not.toBeInTheDocument());
  });

  it("switches the account used for saving without asking", async () => {
    authApi.getConnectionStatus.mockResolvedValue({
      pixiv: { name: "表", webSession: true },
      fanbox: null,
      accounts: { pixiv: [{ key: "p1", name: "表", active: true }, { key: "p2", name: "裏", active: false }], fanbox: [] },
    });
    authApi.activateAccount.mockResolvedValue({
      pixiv: { name: "裏", webSession: true },
      fanbox: null,
      accounts: { pixiv: [{ key: "p1", name: "表", active: false }, { key: "p2", name: "裏", active: true }], fanbox: [] },
    });
    renderConnectionSettings();

    expect(await screen.findByText("使用中")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "アカウントを追加" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "裏を使う" }));

    await waitFor(() => expect(authApi.activateAccount).toHaveBeenCalledWith("pixiv", "p2"));
    expect(await screen.findByRole("button", { name: "表を使う" })).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("says which saved connection could not be read, and keeps the other one", async () => {
    authApi.getConnectionStatus.mockResolvedValue({ pixiv: { name: "pixiv利用者", webSession: true }, fanbox: null, unreadable: ["fanbox"] });
    renderConnectionSettings();

    expect(await screen.findByText("pixiv利用者")).toBeInTheDocument();
    expect(screen.getByText("保存された接続を読み取れませんでした。接続し直してください")).toBeInTheDocument();
  });

  it("offers a browser reconnect when the pixiv connection lacks a web session", async () => {
    authApi.getConnectionStatus.mockResolvedValue({ pixiv: { name: "pixiv利用者", webSession: false }, fanbox: null });
    renderConnectionSettings();

    expect(await screen.findByText("pixiv利用者")).toBeInTheDocument();
    expect(screen.getByText("一覧を使った高速確認には、ブラウザでの再接続が必要です")).toBeInTheDocument();
  });
});
