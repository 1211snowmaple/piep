import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { hasUnsavedWork } from "@/lib/unsavedGuard";
import SavePage from "./SavePage";

const browserApi = vi.hoisted(() => ({
  openEmbeddedBrowser: vi.fn().mockResolvedValue(undefined),
  setEmbeddedBrowserBounds: vi.fn().mockResolvedValue(true),
  setEmbeddedBrowserVisible: vi.fn().mockResolvedValue(true),
  navigateEmbeddedBrowser: vi.fn().mockResolvedValue(undefined),
  getEmbeddedBrowserUrl: vi.fn().mockResolvedValue(null),
  closeEmbeddedBrowser: vi.fn().mockResolvedValue(undefined),
  destroyEmbeddedBrowser: vi.fn().mockResolvedValue(undefined),
  goBackEmbeddedBrowser: vi.fn().mockResolvedValue(undefined),
  goForwardEmbeddedBrowser: vi.fn().mockResolvedValue(undefined),
  reloadEmbeddedBrowser: vi.fn().mockResolvedValue(undefined),
  openStandaloneBrowser: vi.fn().mockResolvedValue(false),
  closeStandaloneBrowser: vi.fn().mockResolvedValue(true),
  getStandaloneBrowserUrl: vi.fn().mockResolvedValue(null),
}));

const api = vi.hoisted(() => ({
  fetchPixivSeriesNovels: vi.fn(),
  fetchPixivNovel: vi.fn(),
  downloadAndSave: vi.fn(),
}));

vi.mock("@/services/browserApi", () => browserApi);
vi.mock("@/services/eventBus", () => ({ subscribeTauriEvent: vi.fn(() => () => undefined) }));
vi.mock("@/store", () => ({ store: { get: vi.fn().mockResolvedValue("token"), set: vi.fn(), save: vi.fn() } }));
vi.mock("@/services/downloadApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/downloadApi")>()),
  ...api,
}));
vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  getDownloadBySource: vi.fn().mockResolvedValue(null),
  setWatchUpdates: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("@/features/updates/updateWorkflow", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/features/updates/updateWorkflow")>()),
  refreshEntityProfilesForEntries: vi.fn().mockResolvedValue(undefined),
}));

const SERIES = "https://www.pixiv.net/novel/series/1000";

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

async function collectCandidates() {
  const address = await screen.findByLabelText("ブラウザのアドレス");
  fireEvent.change(address, { target: { value: SERIES } });
  fireEvent.click(screen.getByRole("button", { name: "URLを開く" }));
  fireEvent.click(await screen.findByRole("button", { name: "候補を取得" }));
  expect(await screen.findByText("第一話")).toBeInTheDocument();
}

/**
 * 終了ガードが守るのは、走っている保存である。画面ではない。
 *
 * 保存はこの画面を離れても続く - そう利用者にも知らせている。ところが
 * ガードの登録を画面の後片付けで外していたころは、**守るべきものが残っている
 * のに守りだけが消えて**いた。保存中にライブラリへ移ってウィンドウを閉じると、
 * 何も訊かれずに終了し、走っていた保存がそのまま消えた。
 */
describe("SavePage の終了ガード", () => {
  beforeEach(() => {
    window.location.hash = "#/save/pixiv";
    Object.values(browserApi).forEach((mock) => mock.mockClear());
    api.fetchPixivSeriesNovels.mockReset().mockResolvedValue([
      { id: 1, title: "第一話", user: { name: "作者" } },
    ]);
    api.fetchPixivNovel.mockReset().mockResolvedValue({ detail: { title: "第一話" }, text: "本文" });
  });

  it("画面を離れても、保存が続いているあいだは閉じる前に訊く", async () => {
    let finishSave!: () => void;
    api.downloadAndSave.mockReset().mockImplementation(
      () => new Promise((resolve) => { finishSave = () => resolve({ id: 1, source: "pixiv", authorId: "7" }); }),
    );

    const { unmount } = renderSavePage();
    await collectCandidates();
    fireEvent.click(screen.getByRole("button", { name: "1件をライブラリに保存" }));

    await waitFor(() => expect(api.downloadAndSave).toHaveBeenCalledTimes(1));
    expect(hasUnsavedWork("close")).toBe(true);

    // 画面を離れる。保存は走ったままなので、守りも残っていなければならない。
    unmount();
    expect(hasUnsavedWork("close")).toBe(true);

    // 保存が終われば、もう訊く理由はない。ここが false に戻らないと、
    // 一度保存しただけでアプリが永久に「未保存あり」と言い張ることになる。
    finishSave();
    await waitFor(() => expect(hasUnsavedWork("close")).toBe(false));
  });
});
