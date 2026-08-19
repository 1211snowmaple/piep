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

// 待ちを本当に待つとテストが数秒止まる。待った長さだけ記録して先へ進む。
const paced = vi.hoisted(() => ({ waits: [] as number[] }));

vi.mock("@/services/browserApi", () => browserApi);
vi.mock("@/services/eventBus", () => ({ subscribeTauriEvent: vi.fn(() => () => undefined) }));
vi.mock("@/store", () => ({ store: { get: vi.fn().mockResolvedValue("token"), set: vi.fn(), save: vi.fn() } }));
vi.mock("@/lib/sourcePacing", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/sourcePacing")>();
  return {
    ...actual,
    createSourcePacer: (delayMs = actual.SOURCE_REQUEST_DELAY_MS) =>
      actual.createSourcePacer(delayMs, async (ms) => { paced.waits.push(ms); }),
  };
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
 * pixiv は続けざまに叩けば断ってくる。89件を選んで一気に保存したときに
 * 起きたのがこれで、間隔が無かったころは途中から残り全部が取得制限で落ちた。
 */
describe("SavePage と取得元の間合い", () => {
  beforeEach(() => {
    window.location.hash = "#/save/pixiv";
    paced.waits.length = 0;
    Object.values(browserApi).forEach((mock) => mock.mockClear());
    api.fetchPixivSeriesNovels.mockReset().mockResolvedValue([
      { id: 1, title: "第一話", user: { name: "作者" } },
      { id: 2, title: "第二話", user: { name: "作者" } },
      { id: 3, title: "第三話", user: { name: "作者" } },
    ]);
    api.fetchPixivNovel.mockReset().mockResolvedValue({ detail: { title: "第一話" }, text: "本文" });
    api.downloadAndSave.mockReset().mockImplementation(async () => ({ id: 1, source: "pixiv", authorId: "7" }));
  });

  it("leaves a gap between works instead of firing them back to back", async () => {
    renderSavePage();
    await collectCandidates();

    fireEvent.click(screen.getByRole("button", { name: "3件をライブラリに保存" }));
    await waitFor(() => expect(api.downloadAndSave).toHaveBeenCalledTimes(3));

    // 3件なら間は2回。最後の1件のあとまで待たない。
    await waitFor(() => expect(paced.waits.length).toBeGreaterThanOrEqual(2));
    expect(paced.waits.every((ms) => ms > 0)).toBe(true);
  });

  // 断られただけなら、その項目はまだ失敗ではない。間を空けてやり直す。
  it("backs off and retries the same work when the source refuses", async () => {
    api.fetchPixivNovel
      .mockRejectedValueOnce(new Error('レスポンスを解析できませんでした: {"error":{"message":"Rate Limit"}}'))
      .mockResolvedValue({ detail: { title: "第一話" }, text: "本文" });

    renderSavePage();
    await collectCandidates();
    fireEvent.click(screen.getByRole("button", { name: "3件をライブラリに保存" }));

    // 断られた1件はやり直され、3件とも保存まで届く。
    await waitFor(() => expect(api.downloadAndSave).toHaveBeenCalledTimes(3));
    await waitFor(() => expect(screen.queryByText(/取得が制限されています/)).toBeNull());
    expect(api.fetchPixivNovel).toHaveBeenCalledTimes(4);
    // やり直しの前には、通常より長く下がる。
    expect(Math.max(...paced.waits)).toBeGreaterThan(Math.min(...paced.waits));
  });

  // やり直しても断られ続けるなら諦めて次へ行く。1件のために全部を止めない。
  it("gives up on one work rather than stalling the whole batch", async () => {
    api.fetchPixivNovel.mockRejectedValue(
      new Error("アクセス制限（レートリミット）に達しました。時間をおいてからやり直してください"),
    );

    renderSavePage();
    await collectCandidates();
    fireEvent.click(screen.getByRole("button", { name: "3件をライブラリに保存" }));

    // 1件あたり 1回 + やり直し2回 = 3回で見切り、3件ぶん進む。
    await waitFor(() => expect(api.fetchPixivNovel).toHaveBeenCalledTimes(9), { timeout: 5_000 });
    expect(api.downloadAndSave).not.toHaveBeenCalled();
  });
});
