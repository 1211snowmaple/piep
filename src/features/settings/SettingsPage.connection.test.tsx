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
  loginPixivWebview: vi.fn(),
  connectPixivToken: vi.fn(),
  loginFanboxWebview: vi.fn(),
  connectFanboxSession: vi.fn(),
}));

vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  getSearchIndexStatus: vi.fn().mockResolvedValue({ totalDownloads: 12, indexedDownloads: 12, pendingDownloads: 0, isComplete: true, phase: "ready", semanticIndexedChunks: 0, semanticIndexedDownloads: 0, semanticPendingDownloads: 12, semanticModelReady: false, embeddingProvider: "CPU", gpuEnabled: false, throughputPerSec: null }),
}));
vi.mock("@/services/shelfApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/shelfApi")>()),
  shelfStatus: vi.fn().mockResolvedValue({ state: "ready", path: "C:/piep/library" }),
}));
vi.mock("@/services/shelfHealthApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/shelfHealthApi")>()),
  shelfDashboard: vi.fn().mockResolvedValue({ stats: { totalDownloads: 12, pixivCount: 12, fanboxCount: 0, totalAssets: 108, totalSizeBytes: 32_000_000 } }),
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
    authApi.getConnectionStatus.mockResolvedValue({ pixiv: null, fanbox: { name: "FANBOX利用者" } });
    authApi.disconnectAccount.mockResolvedValue({ pixiv: null, fanbox: null });
  });

  it("asks the backend to forget the FANBOX connection", async () => {
    renderConnectionSettings();

    expect(await screen.findByText("FANBOX利用者")).toBeInTheDocument();
    fireEvent.click(await screen.findByRole("button", { name: "接続解除" }));
    expect(await screen.findByRole("dialog", { name: "FANBOXとの接続を解除しますか？" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "接続を解除" }));

    await waitFor(() => expect(authApi.disconnectAccount).toHaveBeenCalledWith("fanbox"));
    await waitFor(() => expect(screen.queryByRole("button", { name: "接続解除" })).not.toBeInTheDocument());
  });

  it("offers a browser reconnect when the pixiv connection lacks a web session", async () => {
    authApi.getConnectionStatus.mockResolvedValue({ pixiv: { name: "pixiv利用者", webSession: false }, fanbox: null });
    renderConnectionSettings();

    expect(await screen.findByText("pixiv利用者")).toBeInTheDocument();
    expect(screen.getByText("一覧を使った高速確認には、ブラウザでの再接続が必要です")).toBeInTheDocument();
  });
});
