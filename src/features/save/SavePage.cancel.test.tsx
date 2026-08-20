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
// 待ちは本物のまま、十分に長く取る。中止で断ち切れないなら、ここのテストは
// 時間切れになる - それが直したかった症状そのものである。
vi.mock("@/lib/sourcePacing", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/sourcePacing")>();
  return { ...actual, createSourcePacer: () => actual.createSourcePacer(30_000) };
});
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
  render(
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
 * 中止は、押した瞬間に返事が要る。
 *
 * 取得制限に当たったあとの待ちは30秒近くまで伸びる。そのあいだ中止を握り
 * 潰していたころは、押しても何も起きず、押せていないのと変わらなかった。
 */
describe("SavePage の中止", () => {
  beforeEach(() => {
    window.location.hash = "#/save/pixiv";
    Object.values(browserApi).forEach((mock) => mock.mockClear());
    api.fetchPixivSeriesNovels.mockReset().mockResolvedValue([
      { id: 1, title: "第一話", user: { name: "作者" } },
      { id: 2, title: "第二話", user: { name: "作者" } },
      { id: 3, title: "第三話", user: { name: "作者" } },
    ]);
    api.fetchPixivNovel.mockReset().mockResolvedValue({ detail: { title: "第一話" }, text: "本文" });
    api.downloadAndSave.mockReset().mockImplementation(async () => ({ id: 1, source: "pixiv", authorId: "7" }));
  });

  it("stops while it is waiting between works, instead of after the wait", async () => {
    renderSavePage();
    await collectCandidates();

    fireEvent.click(screen.getByRole("button", { name: "3件をライブラリに保存" }));
    // 1件目が通り、次の1件までの間合いに入ったところ。
    await waitFor(() => expect(api.downloadAndSave).toHaveBeenCalledTimes(1));

    fireEvent.click(await screen.findByRole("button", { name: "中止" }));

    // 待ちの残りを待たずに終わる。残りの2件へは行かない。
    await waitFor(() => expect(screen.getByRole("button", { name: "2件をライブラリに保存" })).toBeInTheDocument());
    expect(api.downloadAndSave).toHaveBeenCalledTimes(1);
  });

  it("stops while it is backing off from a refusal", async () => {
    api.fetchPixivNovel.mockRejectedValue(new Error("アクセス制限（レートリミット）に達しました"));

    renderSavePage();
    await collectCandidates();

    fireEvent.click(screen.getByRole("button", { name: "3件をライブラリに保存" }));
    await waitFor(() => expect(api.fetchPixivNovel).toHaveBeenCalledTimes(1));

    fireEvent.click(await screen.findByRole("button", { name: "中止" }));

    // 断られただけの項目は、まだ失敗ではない。中止で失敗に落とさない。
    await waitFor(() => expect(screen.getByRole("button", { name: "3件をライブラリに保存" })).toBeInTheDocument());
    expect(api.fetchPixivNovel).toHaveBeenCalledTimes(1);
    expect(api.downloadAndSave).not.toHaveBeenCalled();
  });

  // 通信の最中は待ちを断ち切っても止まれない。それでも、押されたことは返す。
  it("says it heard the click even while a request is still in flight", async () => {
    let release = () => undefined as void;
    api.fetchPixivNovel.mockImplementation(() => new Promise((resolve) => {
      release = () => resolve({ detail: { title: "第一話" }, text: "本文" });
    }));

    renderSavePage();
    await collectCandidates();

    fireEvent.click(screen.getByRole("button", { name: "3件をライブラリに保存" }));
    fireEvent.click(await screen.findByRole("button", { name: "中止" }));

    const acknowledged = await screen.findByRole("button", { name: "中止しています" });
    expect(acknowledged).toBeDisabled();
    expect(screen.getByRole("button", { name: "中止待ち" })).toBeInTheDocument();

    release();
    await waitFor(() => expect(screen.getByRole("button", { name: "2件をライブラリに保存" })).toBeInTheDocument());
  });
});
