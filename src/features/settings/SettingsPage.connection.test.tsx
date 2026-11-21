import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { theme } from "@/theme";

const storeApi = vi.hoisted(() => ({
  get: vi.fn().mockResolvedValue(null),
  set: vi.fn().mockResolvedValue(undefined),
  delete: vi.fn().mockResolvedValue(undefined),
  save: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  getSearchIndexStatus: vi.fn().mockResolvedValue({ totalDownloads: 12, indexedDownloads: 12, pendingDownloads: 0, isComplete: true, phase: "ready", semanticIndexedChunks: 0, semanticIndexedDownloads: 0, semanticPendingDownloads: 12, semanticModelReady: false, embeddingProvider: "CPU", gpuEnabled: false, throughputPerSec: null }),
}));
vi.mock("@/services/libraryV2Api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/libraryV2Api")>()),
  libraryV2Status: vi.fn().mockResolvedValue({ state: "ready", path: "C:/piep/library" }),
}));
vi.mock("@/services/shelfHealthApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/shelfHealthApi")>()),
  libraryV2Dashboard: vi.fn().mockResolvedValue({ stats: { totalDownloads: 12, pixivCount: 12, fanboxCount: 0, totalAssets: 108, totalSizeBytes: 32_000_000 } }),
}));
vi.mock("@/store", () => ({
  store: storeApi,
}));
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
    storeApi.get.mockResolvedValue(null);
  });

  it("removes the FANBOX user agent when disconnecting", async () => {
    storeApi.get.mockImplementation(async (key: string) => key === "fanbox_user"
      ? { userId: "fanbox-1", name: "FANBOX利用者" }
      : null);
    renderConnectionSettings();

    fireEvent.click(await screen.findByRole("button", { name: "接続解除" }));
    expect(await screen.findByRole("dialog", { name: "FANBOXとの接続を解除しますか？" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "接続を解除" }));

    await waitFor(() => expect(storeApi.save).toHaveBeenCalledOnce());
    expect(storeApi.delete).toHaveBeenCalledWith("fanbox_session_id");
    expect(storeApi.delete).toHaveBeenCalledWith("fanbox_user");
    expect(storeApi.delete).toHaveBeenCalledWith("fanbox_user_agent");
  });
});
