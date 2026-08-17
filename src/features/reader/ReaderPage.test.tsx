import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { getDemoReader } from "@/mocks/demoData";
import type { ReaderContentPage } from "@/types/library";
import ReaderPage from "./ReaderPage";

const dbApi = vi.hoisted(() => ({
  getReaderMetadata: vi.fn(),
  getReaderContentPage: vi.fn(),
  searchReaderContent: vi.fn(),
}));

vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  getReaderMetadata: dbApi.getReaderMetadata,
  getReaderContentPage: dbApi.getReaderContentPage,
  searchReaderContent: dbApi.searchReaderContent,
}));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

function content(page: number, html: string): ReaderContentPage {
  return { page, pageCount: 2, html, plainText: html.replace(/<[^>]+>/g, ""), totalPlainTextChars: 200 };
}

function renderReader() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<MantineProvider><QueryClientProvider client={client}><AppRouter><ReaderPage /></AppRouter></QueryClientProvider></MantineProvider>);
}

describe("ReaderPage position restoration", () => {
  beforeEach(() => {
    window.localStorage.clear();
    window.sessionStorage.clear();
    window.location.hash = "#/reader/101";
    const demo = getDemoReader(101);
    dbApi.getReaderMetadata.mockResolvedValue({
      download: demo.download,
      versions: demo.versions,
      assetCount: demo.assets.length,
      isEdited: demo.isEdited,
      activeEditRevision: demo.activeEditRevision,
    });
    dbApi.searchReaderContent.mockResolvedValue([]);
  });

  it("waits for the saved source page and never exposes or persists its placeholder", async () => {
    const pageTwo = deferred<ReaderContentPage>();
    dbApi.getReaderContentPage.mockImplementation((_id: number, _version: number | null, page: number) => (
      page === 0 ? Promise.resolve(content(0, '<a href="https://example.com/old">1ページの古い本文</a>')) : pageTwo.promise
    ));
    window.localStorage.setItem("piep.reader-position.101.current", JSON.stringify({ page: 2, top: 240 }));

    const view = renderReader();
    await waitFor(() => expect(dbApi.getReaderContentPage).toHaveBeenCalledWith(101, null, 1));
    expect(await screen.findByText("読書位置のページを読み込んでいます")).toBeInTheDocument();
    expect(screen.queryByText("1ページの古い本文")).toBeNull();
    expect(screen.getByRole("button", { name: "現在の位置にしおりを挟む" })).toBeDisabled();

    const viewport = view.container.querySelector<HTMLElement>(".reader-scroll .mantine-ScrollArea-viewport");
    expect(viewport).not.toBeNull();
    Object.defineProperties(viewport!, {
      scrollHeight: { configurable: true, value: 1_000 },
      clientHeight: { configurable: true, value: 400 },
    });
    viewport!.scrollTop = 90;
    fireEvent.scroll(viewport!);
    expect(JSON.parse(window.localStorage.getItem("piep.reader-position.101.current") ?? "null")).toEqual({ page: 2, top: 240 });

    await act(async () => { pageTwo.resolve(content(1, "<p>2ページの本文</p>")); });
    await waitFor(() => expect(screen.getByText("2ページの本文")).toBeInTheDocument());
    await waitFor(() => expect(viewport!.scrollTop).toBe(240));
    expect(screen.getByRole("button", { name: "現在の位置にしおりを挟む" })).toBeEnabled();

    viewport!.scrollTop = 360;
    fireEvent.scroll(viewport!);
    expect(JSON.parse(window.localStorage.getItem("piep.reader-position.101.current") ?? "null")).toEqual({ page: 2, top: 360 });
  });

  it("hides the previous version while history navigation loads another version", async () => {
    const nextVersion = deferred<ReaderContentPage>();
    dbApi.getReaderContentPage.mockImplementation((_id: number, version: number | null) => (
      version === 2 ? nextVersion.promise : Promise.resolve(content(0, '<a href="https://example.com/old">現在版の本文</a>'))
    ));
    renderReader();
    expect(await screen.findByText("現在版の本文")).toBeInTheDocument();

    window.location.hash = "#/reader/101?version=2";
    window.dispatchEvent(new HashChangeEvent("hashchange"));
    await waitFor(() => expect(dbApi.getReaderContentPage).toHaveBeenCalledWith(101, 2, 0));
    expect(screen.queryByText("現在版の本文")).toBeNull();
    expect(screen.getByText("読書位置のページを読み込んでいます")).toBeInTheDocument();

    await act(async () => { nextVersion.resolve(content(0, "<p>履歴版の本文</p>")); });
    await waitFor(() => expect(screen.getByText("履歴版の本文")).toBeInTheDocument());
  });
});
