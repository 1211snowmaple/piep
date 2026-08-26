import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { getDemoReader } from "@/mocks/demoData";
import type { CollectionKind, WorkCollection } from "@/types/collections";
import type { ReaderContentPage } from "@/types/library";
import ReaderPage from "./ReaderPage";

const dbApi = vi.hoisted(() => ({
  getReaderMetadata: vi.fn(),
  getReaderContentPage: vi.fn(),
  searchReaderContent: vi.fn(),
  searchDownloadsV2: vi.fn(),
}));
const collectionApi = vi.hoisted(() => ({
  listCollectionsForWork: vi.fn(),
  getWorkCollection: vi.fn(),
}));

vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  getReaderMetadata: dbApi.getReaderMetadata,
  getReaderContentPage: dbApi.getReaderContentPage,
  searchReaderContent: dbApi.searchReaderContent,
  searchDownloadsV2: dbApi.searchDownloadsV2,
}));

vi.mock("@/services/collectionApi", () => ({
  listCollectionsForWork: collectionApi.listCollectionsForWork,
  getWorkCollection: collectionApi.getWorkCollection,
}));

/** A single-page work, so the finish panel is on screen straight away. */
function onlyPage(html: string): ReaderContentPage {
  return { page: 0, pageCount: 1, html, plainText: html.replace(/<[^>]+>/g, ""), totalPlainTextChars: 100 };
}

function collectionOf(kind: CollectionKind): WorkCollection {
  const member = (sourceId: string, downloadId: number, title: string, position: number) => ({
    collectionId: "collection-1",
    source: "pixiv" as const,
    sourceId,
    downloadId,
    title,
    authorName: "作者",
    coverPath: null,
    textLength: 100,
    position,
    memberRole: "main" as const,
    addedBy: "manual" as const,
    pinned: false,
    note: null,
    missing: false,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    // リーダーの前後移動は download_id と並びだけで決まる。作品そのものは
    // 読み込み済みのものを使うので、この画面の試験には要らない。
    work: null,
    editions: [],
  });
  return {
    id: "collection-1",
    name: "雨の記憶",
    description: null,
    collectionKind: kind,
    coverDownloadId: null,
    coverPath: null,
    coverMode: "mosaic" as const,
    coverImagePath: null,
    coverTiles: [],
    nameSource: "manual" as const,
    track: "manual" as const,
    revision: 1,
    memberCount: 2,
    availableCount: 2,
    totalTextLength: 200,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    members: [member("101", 101, "いま読んでいる話", 0), member("102", 102, "となりの話", 1)],
  };
}

function renderReader() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<MantineProvider><QueryClientProvider client={client}><AppRouter><ReaderPage /></AppRouter></QueryClientProvider></MantineProvider>);
}

describe("ReaderPage collection continuation", () => {
  beforeEach(() => {
    window.localStorage.clear();
    window.sessionStorage.clear();
    window.location.hash = "#/reader/101";
    const demo = getDemoReader(101);
    dbApi.getReaderMetadata.mockResolvedValue({
      download: { ...demo.download, id: 101, source: "pixiv", sourceId: "101", seriesId: null, personId: null, authorId: "" },
      versions: demo.versions,
      assetCount: demo.assets.length,
      isEdited: demo.isEdited,
      activeEditRevision: demo.activeEditRevision,
    });
    dbApi.searchReaderContent.mockResolvedValue([]);
    dbApi.searchDownloadsV2.mockResolvedValue({ items: [], nextCursor: null, total: 0 });
    dbApi.getReaderContentPage.mockResolvedValue(onlyPage("<p>本文</p>"));
    collectionApi.listCollectionsForWork.mockResolvedValue([{ id: "collection-1" }]);
  });

  it("offers the next work only when the collection actually has a reading order", async () => {
    collectionApi.getWorkCollection.mockResolvedValue(collectionOf("ordered"));
    renderReader();

    expect(await screen.findByText("コレクション「雨の記憶」")).toBeInTheDocument();
    // 順序付きなので現在位置と「読む順あり」を示し、次の作品へ進める。
    expect(screen.getByText(/1 \/ 2/)).toBeInTheDocument();
    expect(screen.getByText(/読む順あり/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "となりの話" })).toBeInTheDocument();
    expect(screen.queryByText("読む順は決まっていません。同じまとまりの作品です。")).toBeNull();
  });

  it("never presents an unordered collection as a serial", async () => {
    collectionApi.getWorkCollection.mockResolvedValue(collectionOf("unordered"));
    renderReader();

    expect(await screen.findByText("コレクション「雨の記憶」")).toBeInTheDocument();
    // 設計提案 10-4 / Phase 2 受け入れ条件: 順序なしを「続編」として見せない。
    await waitFor(() => expect(screen.getByText("読む順は決まっていません。同じまとまりの作品です。")).toBeInTheDocument());
    expect(screen.getByText(/順序なし/)).toBeInTheDocument();
    expect(screen.queryByText(/次の作品/)).toBeNull();
    expect(screen.queryByText(/前の作品/)).toBeNull();
    expect(screen.queryByText(/\d+ \/ \d+/)).toBeNull();
    // 隣の作品自体は開ける。順序を主張しないだけである。
    expect(screen.getByRole("button", { name: "となりの話" })).toBeInTheDocument();
  });

  it("follows the series cursor when the current work is the last item of a page", async () => {
    const demo = getDemoReader(101);
    dbApi.getReaderMetadata.mockResolvedValue({
      download: { ...demo.download, id: 101, source: "pixiv", sourceId: "101", seriesId: "long-series", seriesTitle: "長い連載", personId: null, authorId: "" },
      versions: demo.versions,
      assetCount: demo.assets.length,
      isEdited: demo.isEdited,
      activeEditRevision: demo.activeEditRevision,
    });
    collectionApi.listCollectionsForWork.mockResolvedValue([]);
    dbApi.searchDownloadsV2.mockImplementation(({ cursor }: { cursor?: string | null }) => Promise.resolve(cursor
      ? { items: [{ ...demo.download, id: 102, title: "二百一話" }], nextCursor: null, totalEstimate: 201, searchMeta: {}, facetsVersion: 1 }
      : { items: [{ ...demo.download, id: 101, title: "二百話" }], nextCursor: "page-2", totalEstimate: 201, searchMeta: {}, facetsVersion: 1 }));

    renderReader();

    expect(await screen.findByText("公式シリーズ「長い連載」")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "二百一話" })).toBeInTheDocument();
    expect(dbApi.searchDownloadsV2).toHaveBeenCalledWith(expect.objectContaining({ cursor: "page-2" }));
  });
});
