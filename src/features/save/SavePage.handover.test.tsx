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

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
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

  it("serializes provider initialization so a stale slow WebView cannot win", async () => {
    const firstOpen = deferred<void>();
    const originalElementFromPoint = document.elementFromPoint;
    Object.defineProperty(document, "elementFromPoint", { configurable: true, value: () => null });
    browserApi.openEmbeddedBrowser
      .mockImplementationOnce(() => firstOpen.promise)
      .mockResolvedValue(undefined);
    const bounds = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({
      x: 0, y: 0, top: 0, left: 0, right: 900, bottom: 600, width: 900, height: 600, toJSON: () => ({}),
    });
    renderSavePage();
    await waitFor(() => expect(browserApi.openEmbeddedBrowser).toHaveBeenCalledTimes(1));

    fireEvent.click(screen.getByText("FANBOX"));
    await waitFor(() => expect(window.location.hash).toBe("#/save/fanbox"));
    // The second provider waits for the non-cancellable first IPC to settle.
    expect(browserApi.openEmbeddedBrowser).toHaveBeenCalledTimes(1);

    firstOpen.resolve();
    await waitFor(() => expect(browserApi.openEmbeddedBrowser).toHaveBeenCalledTimes(2));
    const lastOpen = browserApi.openEmbeddedBrowser.mock.calls[browserApi.openEmbeddedBrowser.mock.calls.length - 1];
    expect(lastOpen?.[0]).toBe("https://www.fanbox.cc/");
    bounds.mockRestore();
    if (originalElementFromPoint) Object.defineProperty(document, "elementFromPoint", { configurable: true, value: originalElementFromPoint });
    else delete (document as Partial<Document>).elementFromPoint;
  });
});

/**
 * 大きいウィンドウは、この画面より長生きする。
 *
 * 切り離したまま別の画面へ行くと、片付けで閉じるのは埋め込みだけで、大きい
 * ウィンドウは残る。切り離していたという記憶は画面と一緒に消えるので、戻って
 * きたときに埋め込みまで開き、**同じサイトが二つの窓に出ていた**。
 */
describe("returning to the save screen", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    browserApi.openStandaloneBrowser.mockResolvedValue(false);
    browserApi.setEmbeddedBrowserBounds.mockResolvedValue(true);
    browserApi.setEmbeddedBrowserVisible.mockResolvedValue(true);
    browserApi.getEmbeddedBrowserUrl.mockResolvedValue("https://www.pixiv.net/");
  });

  it("adopts the large window that is still open instead of opening a second one", async () => {
    browserApi.getStandaloneBrowserUrl.mockResolvedValue("https://www.pixiv.net/users/15884098");
    window.location.hash = "#/save/pixiv";
    renderSavePage();

    await waitFor(() => expect(browserApi.getStandaloneBrowserUrl).toHaveBeenCalled());
    // 埋め込みは開かない。開くと同じサイトが二つ出る。
    await waitFor(() => expect(browserApi.setEmbeddedBrowserVisible).toHaveBeenCalledWith(false));
    expect(browserApi.openEmbeddedBrowser).not.toHaveBeenCalled();
    // 大きいウィンドウが見ている頁を、この画面も引き継ぐ。
    expect(await screen.findByDisplayValue("https://www.pixiv.net/users/15884098")).toBeInTheDocument();
  });

  it("opens the in-app browser when no large window is standing", async () => {
    browserApi.getStandaloneBrowserUrl.mockResolvedValue(null);
    // 埋め込みは枠の寸法が取れないと作らない。jsdom は 0 を返すので与える。
    const originalElementFromPoint = document.elementFromPoint;
    Object.defineProperty(document, "elementFromPoint", { configurable: true, value: () => null });
    const bounds = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({
      x: 0, y: 0, top: 0, left: 0, right: 900, bottom: 600, width: 900, height: 600, toJSON: () => ({}),
    });
    window.location.hash = "#/save/pixiv";
    const page = renderSavePage();

    await waitFor(() => expect(browserApi.openEmbeddedBrowser).toHaveBeenCalled());
    page.unmount();
    bounds.mockRestore();
    if (originalElementFromPoint) Object.defineProperty(document, "elementFromPoint", { configurable: true, value: originalElementFromPoint });
    else delete (document as Partial<Document>).elementFromPoint;
  });
});
