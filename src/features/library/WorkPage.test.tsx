import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { WorkspaceProvider } from "@/app/WorkspaceContext";
import { getDemoReader } from "@/mocks/demoData";
import type { ReaderContentPage } from "@/types/library";
import WorkPage from "./WorkPage";

const dbApi = vi.hoisted(() => ({
  getReaderMetadata: vi.fn(),
  getReaderContentPage: vi.fn(),
}));

vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  getReaderMetadata: dbApi.getReaderMetadata,
  getReaderContentPage: dbApi.getReaderContentPage,
}));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

function content(page: number, html: string): ReaderContentPage {
  return { page, pageCount: 2, html, plainText: html.replace(/<[^>]+>/g, ""), totalPlainTextChars: 200 };
}

describe("WorkPage content preview", () => {
  beforeEach(() => {
    window.location.hash = "#/works/101?tab=content";
    const demo = getDemoReader(101);
    dbApi.getReaderMetadata.mockResolvedValue({
      download: demo.download,
      versions: demo.versions,
      assetCount: demo.assets.length,
      isEdited: demo.isEdited,
      activeEditRevision: demo.activeEditRevision,
    });
  });

  it("does not display or operate the previous page while the requested page loads", async () => {
    const pageTwo = deferred<ReaderContentPage>();
    dbApi.getReaderContentPage.mockImplementation((_id: number, _version: number | null, page: number) => (
      page === 0 ? Promise.resolve(content(0, '<a href="https://example.com/old">1ページ本文</a>')) : pageTwo.promise
    ));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><WorkPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    expect(await screen.findByText("1ページ本文")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "2" }));
    await waitFor(() => expect(dbApi.getReaderContentPage).toHaveBeenCalledWith(101, null, 1));
    expect(screen.queryByText("1ページ本文")).toBeNull();
    expect(screen.getByText("本文を読み込んでいます")).toBeInTheDocument();

    await act(async () => { pageTwo.resolve(content(1, "<p>2ページ本文</p>")); });
    await waitFor(() => expect(screen.getByText("2ページ本文")).toBeInTheDocument());
  });

  it("shows the complete primary title on the detail page", async () => {
    const longTitle = "マイナーキャラ妄想短編集　陵辱物　とても長い正式な作品名";
    const demo = getDemoReader(101);
    dbApi.getReaderMetadata.mockResolvedValue({
      download: { ...demo.download, title: longTitle },
      versions: demo.versions,
      assetCount: demo.assets.length,
      isEdited: demo.isEdited,
      activeEditRevision: demo.activeEditRevision,
    });
    dbApi.getReaderContentPage.mockResolvedValue(content(0, "<p>本文</p>"));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><WorkPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    const heading = await screen.findByRole("heading", { name: longTitle });
    expect(heading).toHaveClass("work-hero__title");
    expect(heading).not.toHaveClass("line-clamp-2");
  });
});
