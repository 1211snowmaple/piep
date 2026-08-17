import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import SavePage from "./SavePage";

const browserApi = vi.hoisted(() => ({
  openEmbeddedBrowser: vi.fn().mockResolvedValue(undefined),
  setEmbeddedBrowserBounds: vi.fn().mockResolvedValue(true),
  setEmbeddedBrowserVisible: vi.fn().mockResolvedValue(true),
  navigateEmbeddedBrowser: vi.fn().mockResolvedValue(undefined),
  getEmbeddedBrowserUrl: vi.fn().mockResolvedValue("https://www.pixiv.net/"),
  closeEmbeddedBrowser: vi.fn().mockResolvedValue(undefined),
  destroyEmbeddedBrowser: vi.fn().mockResolvedValue(undefined),
  goBackEmbeddedBrowser: vi.fn().mockResolvedValue(undefined),
  goForwardEmbeddedBrowser: vi.fn().mockResolvedValue(undefined),
  reloadEmbeddedBrowser: vi.fn().mockResolvedValue(undefined),
  openStandaloneBrowser: vi.fn().mockResolvedValue(false),
  closeStandaloneBrowser: vi.fn().mockResolvedValue(true),
  getStandaloneBrowserUrl: vi.fn().mockResolvedValue(null),
}));

vi.mock("@/services/browserApi", () => browserApi);
vi.mock("@/services/eventBus", () => ({ subscribeTauriEvent: vi.fn(() => () => undefined) }));
vi.mock("@/store", () => ({ store: { get: vi.fn().mockResolvedValue(null), set: vi.fn(), save: vi.fn() } }));
vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  getDownloadBySource: vi.fn().mockResolvedValue(null),
}));

function renderSavePage() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <MantineProvider>
      <QueryClientProvider client={client}>
        <AppRouter><SavePage /></AppRouter>
      </QueryClientProvider>
    </MantineProvider>,
  );
}

describe("SavePage handover to the large window", () => {
  beforeEach(() => {
    window.location.hash = "#/save/pixiv";
    Object.values(browserApi).forEach((mock) => mock.mockClear());
    browserApi.getStandaloneBrowserUrl.mockResolvedValue(null);
  });

  it("stands the in-app pane down while the large window holds the session", async () => {
    renderSavePage();

    fireEvent.click(await screen.findByRole("button", { name: "大きいウィンドウで開く" }));

    await waitFor(() => expect(browserApi.openStandaloneBrowser).toHaveBeenCalled());
    // Hidden, not destroyed: the login session and the current page survive.
    await waitFor(() => expect(browserApi.setEmbeddedBrowserVisible).toHaveBeenCalledWith(false));
    expect(await screen.findByText("大きいウィンドウで表示中")).toBeInTheDocument();
    // The candidate sidebar is the whole point of the handover, so it stays.
    expect(screen.getByRole("button", { name: "候補を取得" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "大きいウィンドウで開く" })).toBeNull();
  });

  it("brings the page back into the app when the large window is dismissed", async () => {
    renderSavePage();
    fireEvent.click(await screen.findByRole("button", { name: "大きいウィンドウで開く" }));
    const back = await screen.findByRole("button", { name: "アプリ内に戻す" });

    fireEvent.click(back);

    await waitFor(() => expect(browserApi.closeStandaloneBrowser).toHaveBeenCalledWith("pixiv"));
    await waitFor(() => expect(browserApi.setEmbeddedBrowserVisible).toHaveBeenCalledWith(true));
    await waitFor(() => expect(screen.queryByText("大きいウィンドウで表示中")).toBeNull());
  });

  it("picks the handover back up when the page mounts with the window already open", async () => {
    browserApi.getStandaloneBrowserUrl.mockResolvedValue("https://www.pixiv.net/novel/show.php?id=1");
    renderSavePage();

    expect(await screen.findByText("大きいウィンドウで表示中")).toBeInTheDocument();
    await waitFor(() => expect(browserApi.setEmbeddedBrowserVisible).toHaveBeenCalledWith(false));
  });
});
