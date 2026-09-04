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
const updateJobApi = vi.hoisted(() => ({
  listPendingRevisionsCommand: vi.fn(),
  previewPendingRevisionCommand: vi.fn(),
}));

vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  getReaderMetadata: dbApi.getReaderMetadata,
  getReaderContentPage: dbApi.getReaderContentPage,
}));
vi.mock("@/services/updateJobApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/updateJobApi")>()),
  listPendingRevisionsCommand: updateJobApi.listPendingRevisionsCommand,
  previewPendingRevisionCommand: updateJobApi.previewPendingRevisionCommand,
}));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

function content(page: number, html: string): ReaderContentPage {
  return { page, pageCount: 2, html, plainText: html.replace(/<[^>]+>/g, ""), totalPlainTextChars: 200, sourcePageStarts: [0, 1] };
}

describe("WorkPage content preview", () => {
  beforeEach(() => {
    window.location.hash = "#/works/101?tab=content";
    updateJobApi.listPendingRevisionsCommand.mockResolvedValue([]);
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

  it("keeps an already loaded tab ready while the same work stays open", async () => {
    dbApi.getReaderContentPage.mockResolvedValue(content(0, "<p>再表示する本文</p>"));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><WorkPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    expect(await screen.findByText("再表示する本文")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: "概要" }));
    await waitFor(() => expect(screen.queryByText("再表示する本文")).toBeNull());
    fireEvent.click(screen.getByRole("tab", { name: "本文" }));

    expect(await screen.findByText("再表示する本文")).toBeInTheDocument();
    expect(screen.queryByText("本文を読み込んでいます")).toBeNull();
  });

  it("compares the selected saved version with the version immediately before it", async () => {
    window.location.hash = "#/works/101?tab=history";
    const demo = getDemoReader(101);
    const versionOne = { ...demo.versions[0], id: 1, version: 1, textLength: 12, changeSummary: "初回保存" };
    const versionTwo = { ...demo.versions[0], id: 2, version: 2, textLength: 15, changeSummary: "本文を更新" };
    dbApi.getReaderMetadata.mockResolvedValue({
      download: { ...demo.download, currentVersion: 2 },
      versions: [versionTwo, versionOne],
      assetCount: demo.assets.length,
      isEdited: demo.isEdited,
      activeEditRevision: demo.activeEditRevision,
    });
    dbApi.getReaderContentPage.mockImplementation((_id: number, version: number) => Promise.resolve({
      page: 0,
      pageCount: 1,
      html: "",
      plainText: version === 1 ? "冒頭。古い文章。結末。" : "冒頭。新しい文章と追記。結末。",
      totalPlainTextChars: 20,
    }));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><WorkPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    expect(await screen.findByText("v1 → v2 の本文差分")).toBeInTheDocument();
    await waitFor(() => expect(Array.from(document.querySelectorAll('ins[data-kind="added"]')).map((node) => node.textContent).join("")).toContain("新しい"));
    expect(Array.from(document.querySelectorAll('ins[data-kind="added"]')).map((node) => node.textContent).join("")).toContain("追記");
    expect(Array.from(document.querySelectorAll('del[data-kind="removed"]')).map((node) => node.textContent).join("")).toContain("古い");
    expect(dbApi.getReaderContentPage).toHaveBeenCalledWith(101, 2, 0);
    expect(dbApi.getReaderContentPage).toHaveBeenCalledWith(101, 1, 0);

    fireEvent.click(screen.getByRole("radio", { name: "v2全文" }));
    expect(screen.getByText("冒頭。新しい文章と追記。結末。")).toBeInTheDocument();
  });

  it("fetches an unsaved provider revision and compares it with the current saved version", async () => {
    window.location.hash = "#/works/101?tab=history&compare=pending";
    const demo = getDemoReader(101);
    const versionOne = { ...demo.versions[0], id: 1, version: 1, textLength: 12, changeSummary: "初回保存" };
    const versionTwo = { ...demo.versions[0], id: 2, version: 2, textLength: 15, changeSummary: "前回の改稿" };
    dbApi.getReaderMetadata.mockResolvedValue({
      download: { ...demo.download, currentVersion: 2 },
      versions: [versionTwo, versionOne],
      assetCount: demo.assets.length,
      isEdited: demo.isEdited,
      activeEditRevision: demo.activeEditRevision,
    });
    dbApi.getReaderContentPage.mockImplementation((_id: number, version: number) => Promise.resolve({
      page: 0,
      pageCount: 1,
      html: "",
      plainText: version === 2 ? "保存済みの本文。" : "最初の本文。",
      totalPlainTextChars: 20,
    }));
    updateJobApi.listPendingRevisionsCommand.mockResolvedValue([{ downloadId: 101, foundAt: "2026-09-01T00:00:00Z" }]);
    updateJobApi.previewPendingRevisionCommand.mockResolvedValue({
      downloadId: 101,
      baseVersion: 2,
      text: "取得元で直された本文と追記。",
      textLength: 14,
    });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><WorkPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    expect(await screen.findByText("保存済み v2 → 取得元の改稿（未保存） の本文差分")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /取得元の改稿/ })).toHaveAttribute("data-active");
    await waitFor(() => expect(updateJobApi.previewPendingRevisionCommand).toHaveBeenCalledWith(101));
    expect(Array.from(document.querySelectorAll('ins[data-kind="added"]')).map((node) => node.textContent).join("")).toContain("取得元");
    expect(screen.getByRole("radio", { name: "取得元の改稿（未保存）全文" })).toBeInTheDocument();
  });
});
