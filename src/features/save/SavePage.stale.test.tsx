import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen } from "@testing-library/react";
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
const pixivApi = vi.hoisted(() => ({
  fetchPixivSeriesNovels: vi.fn().mockResolvedValue([
    { id: 1, title: "第一話", user: { name: "作者" } },
    { id: 2, title: "第二話", user: { name: "作者" } },
  ]),
}));

vi.mock("@/services/browserApi", () => browserApi);
vi.mock("@/services/eventBus", () => ({ subscribeTauriEvent: vi.fn(() => () => undefined) }));
vi.mock("@/store", () => ({ store: { get: vi.fn().mockResolvedValue("token"), set: vi.fn(), save: vi.fn() } }));
vi.mock("@/services/downloadApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/downloadApi")>()),
  ...pixivApi,
}));
vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  getDownloadBySource: vi.fn().mockResolvedValue(null),
}));

const SERIES = "https://www.pixiv.net/novel/series/1000";
const OTHER = "https://www.pixiv.net/novel/series/2000";
/** 同じシリーズを見たまま、URLだけが動いた形。 */
const SAME_SERIES_DRIFT = "https://www.pixiv.net/en/novel/series/1000?p=2";

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

async function goTo(url: string) {
  const address = await screen.findByLabelText("ブラウザのアドレス");
  fireEvent.change(address, { target: { value: url } });
  fireEvent.click(screen.getByRole("button", { name: "URLを開く" }));
}

/**
 * 取得したあとにページを移ると、一覧は「さっきのページのもの」になる。
 *
 * これを黄色い帯で割り込ませていたときは、パネルの真ん中に一段生えて全体が
 * 下へずれた。帯は無くし、取り直すよう頼む相手（取得ボタン）と、古くなった
 * もの（一覧）に語らせる。
 */
describe("SavePage stale candidates", () => {
  beforeEach(() => {
    window.location.hash = "#/save/pixiv";
    Object.values(browserApi).forEach((mock) => mock.mockClear());
  });

  it("moves the warning onto the controls it concerns instead of a band", async () => {
    renderSavePage();
    await goTo(SERIES);
    fireEvent.click(await screen.findByRole("button", { name: "候補を取得" }));
    expect(await screen.findByText("第一話")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "2件をライブラリに保存" })).toBeEnabled();
    expect(document.querySelector(".candidate-list[data-stale]")).toBeNull();

    await goTo(OTHER);

    // 取り直す先は取得ボタンなので、そこが言う。
    expect(await screen.findByRole("button", { name: "候補を再取得" })).toBeEnabled();
    // 古いのは一覧なので、一覧の見出しに印が付き、中身は退がる。
    expect(screen.getByText("古い")).toBeInTheDocument();
    expect(document.querySelector(".candidate-list[data-stale]")).not.toBeNull();
    // 選んだものは消さない。取り直せば戻ってくる話ではないので。
    expect(screen.getByText("第一話")).toBeInTheDocument();

    // 帯そのものが無い。
    expect(screen.queryByText("ページが変わりました。候補を再取得してください。")).toBeNull();
  });

  /**
   * 目の前に一覧があり、選んであり、どこから取ったかも分かっている。保存を
   * 断る理由がない。取っておいて押させないのは、二度手間を強いるだけだった。
   */
  it("still saves the list it already has after the page moves on", async () => {
    renderSavePage();
    await goTo(SERIES);
    fireEvent.click(await screen.findByRole("button", { name: "候補を取得" }));
    expect(await screen.findByText("第一話")).toBeInTheDocument();

    await goTo(OTHER);

    expect(await screen.findByText("古い")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "2件をライブラリに保存" })).toBeEnabled();
    expect(screen.queryByRole("button", { name: "取り直すと保存できます" })).toBeNull();
  });

  /**
   * 取得元はどれもSPAで、同じページを見ているあいだにもURLだけが動く。文字列
   * を突き合わせていたころは、取った直後に「古い」が点いて保存できなくなった。
   */
  it("does not call the list stale while the same page is still open", async () => {
    renderSavePage();
    await goTo(SERIES);
    fireEvent.click(await screen.findByRole("button", { name: "候補を取得" }));
    expect(await screen.findByText("第一話")).toBeInTheDocument();

    await goTo(SAME_SERIES_DRIFT);

    expect(await screen.findByRole("button", { name: "候補を取得" })).toBeEnabled();
    expect(screen.queryByText("古い")).toBeNull();
    expect(document.querySelector(".candidate-list[data-stale]")).toBeNull();
    expect(screen.getByRole("button", { name: "2件をライブラリに保存" })).toBeEnabled();
  });
});
