import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { getDemoReader } from "@/mocks/demoData";
import type { ReaderContentPage } from "@/types/library";
import ReaderPage from "./ReaderPage";

const dbApi = vi.hoisted(() => ({
  getReaderMetadata: vi.fn(),
  getReaderContentPage: vi.fn(),
  getReaderOutline: vi.fn(),
  searchReaderContent: vi.fn(),
}));

vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  getReaderMetadata: dbApi.getReaderMetadata,
  getReaderContentPage: dbApi.getReaderContentPage,
  getReaderOutline: dbApi.getReaderOutline,
  searchReaderContent: dbApi.searchReaderContent,
}));

const PAGE_COUNT = 6;
const scrolledInto: Element[] = [];

function page(index: number, html: string): ReaderContentPage {
  return {
    page: index,
    pageCount: PAGE_COUNT,
    html,
    plainText: html.replace(/<[^>]+>/g, ""),
    totalPlainTextChars: 400,
    sourcePageStarts: Array.from({ length: PAGE_COUNT }, (_, item) => item),
  };
}

function renderReader() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<MantineProvider><QueryClientProvider client={client}><AppRouter><ReaderPage /></AppRouter></QueryClientProvider></MantineProvider>);
}

/** 本文の器。jsdom は組版をしないので、寸法だけ本物らしくしておく。 */
function stubViewport(container: HTMLElement, { scrollHeight = 3_000, clientHeight = 400 } = {}) {
  const viewport = container.querySelector<HTMLElement>(".reader-scroll .mantine-ScrollArea-viewport");
  if (!viewport) throw new Error("本文の器が見つかりません");
  Object.defineProperties(viewport, {
    scrollHeight: { configurable: true, value: scrollHeight },
    clientHeight: { configurable: true, value: clientHeight },
    scrollWidth: { configurable: true, value: 800 },
    clientWidth: { configurable: true, value: 800 },
  });
  viewport.scrollBy = vi.fn();
  return viewport;
}

describe("読み進む操作", () => {
  beforeEach(() => {
    window.localStorage.clear();
    window.sessionStorage.clear();
    window.location.hash = "#/reader/101";
    scrolledInto.length = 0;
    Element.prototype.scrollIntoView = function scrollIntoView(this: Element) { scrolledInto.push(this); };
    const demo = getDemoReader(101);
    dbApi.getReaderMetadata.mockResolvedValue({
      download: demo.download,
      versions: demo.versions,
      assetCount: demo.assets.length,
      isEdited: demo.isEdited,
      activeEditRevision: demo.activeEditRevision,
    });
    dbApi.getReaderOutline.mockResolvedValue([]);
    dbApi.searchReaderContent.mockResolvedValue([]);
    dbApi.getReaderContentPage.mockImplementation((_id: number, _version: number | null, index: number) =>
      Promise.resolve(page(index, `<h2>章${index + 1}の一</h2><p>${index + 1}ページの本文</p><h2>章${index + 1}の二</h2><p>雨あがりの静けさ</p>`)));
  });

  /**
   * 転送ページは 128KB ＝ 数万字ある。押した瞬間にそれが変わっていたころは、
   * 読んでいない十数画面を跳び越していた。まず 1 画面ぶん読み進める。
   */
  it("Space はまず画面1枚ぶん読み進める（ページは変わらない）", async () => {
    const view = renderReader();
    expect(await screen.findByText("1ページの本文")).toBeInTheDocument();
    const viewport = stubViewport(view.container);
    dbApi.getReaderContentPage.mockClear();

    fireEvent.keyDown(window, { key: " " });

    expect(viewport.scrollBy).toHaveBeenCalledWith(expect.objectContaining({ top: 352 }));
    expect(dbApi.getReaderContentPage).not.toHaveBeenCalledWith(101, null, 1);
  });

  it("ページの終わりまで来ていれば、Space が次のページへ送る", async () => {
    const view = renderReader();
    expect(await screen.findByText("1ページの本文")).toBeInTheDocument();
    const viewport = stubViewport(view.container);
    viewport.scrollTop = 3_000 - 400;

    fireEvent.keyDown(window, { key: " " });

    await waitFor(() => expect(dbApi.getReaderContentPage).toHaveBeenCalledWith(101, null, 1));
    expect(viewport.scrollBy).not.toHaveBeenCalled();
  });

  it("矢印はこれまで通り、転送ページそのものを送る", async () => {
    renderReader();
    expect(await screen.findByText("1ページの本文")).toBeInTheDocument();
    dbApi.getReaderContentPage.mockClear();

    fireEvent.keyDown(window, { key: "ArrowRight" });

    await waitFor(() => expect(dbApi.getReaderContentPage).toHaveBeenCalledWith(101, null, 1));
  });

  /**
   * 同じページに載っている見出しが、どれもページの先頭へ着いていた。
   * 転送ページは長いので、目次が目次として働いていなかった。
   */
  it("目次は、その見出しそのものへ寄せる", async () => {
    dbApi.getReaderOutline.mockResolvedValue([
      { page: 1, index: 0, title: "章1の一" },
      { page: 1, index: 1, title: "章1の二" },
    ]);
    const view = renderReader();
    expect(await screen.findByText("1ページの本文")).toBeInTheDocument();
    stubViewport(view.container);

    fireEvent.click(await screen.findByRole("button", { name: "目次（2章）" }));
    fireEvent.click(await screen.findByRole("button", { name: /章1の二/ }));

    await waitFor(() => expect(scrolledInto.length).toBeGreaterThan(0));
    expect(scrolledInto.map((element) => element.textContent)).toEqual(["章1の二"]);
  });

  /**
   * 隣のページしか見ていなかったころは、そこに一致が無いと前後ボタンが
   * どちらも押せなくなり、探した語の続きへ行く手立てが無くなっていた。
   */
  it("次の一致は、隣ではなく次に一致のあるページへ送る", async () => {
    dbApi.searchReaderContent.mockResolvedValue([
      { page: 1, count: 1, snippet: "1ページ目の抜き書き" },
      { page: 5, count: 2, snippet: "5ページ目の抜き書き" },
    ]);
    const view = renderReader();
    expect(await screen.findByText("1ページの本文")).toBeInTheDocument();
    stubViewport(view.container);

    fireEvent.click(screen.getByRole("button", { name: "本文内を検索" }));
    fireEvent.change(await screen.findByLabelText("本文内の検索語"), { target: { value: "雨" } });
    fireEvent.click(await screen.findByText("1ページ目の抜き書き"));

    // 1ページ目の一致は1件。その先は5ページ目にある。
    await screen.findByLabelText("本文内検索の移動");
    dbApi.getReaderContentPage.mockClear();
    fireEvent.click(screen.getByRole("button", { name: "次の一致へ" }));

    await waitFor(() => expect(dbApi.getReaderContentPage).toHaveBeenCalledWith(101, null, 4));
  });
});
