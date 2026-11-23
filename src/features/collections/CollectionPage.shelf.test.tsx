import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { WorkspaceProvider } from "@/app/WorkspaceContext";
import type { ShelfCard, ShelfCollection, ShelfCollectionSummary } from "@/services/shelfApi";
import CollectionPage from "./CollectionPage";
import { CollectionsPanel } from "./CollectionsPanel";

vi.mock("@tauri-apps/api/core", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@tauri-apps/api/core")>()),
  convertFileSrc: (path: string, protocol: string) => `http://${protocol}.localhost/${path}`,
}));

const api = vi.hoisted(() => ({
  shelfStatus: vi.fn(),
  shelfCollection: vi.fn(),
  shelfCollections: vi.fn(),
  shelfEditCollection: vi.fn(),
  shelfImportEditImage: vi.fn(),
}));
vi.mock("@/services/shelfApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/shelfApi")>()),
  ...api,
}));
const openSingleDialog = vi.hoisted(() => vi.fn());
vi.mock("@/services/dialogApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dialogApi")>()),
  openSingleDialog,
}));
const suggest = vi.hoisted(() => ({
  listCollectionSuggestions: vi.fn(),
  suggestCollectionAdditions: vi.fn(),
  generateCollectionSuggestion: vi.fn(),
}));
vi.mock("@/services/collectionApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/collectionApi")>()),
  ...suggest,
}));
vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
}));

const COVER = "ab".repeat(32);

const card: ShelfCard = {
  id: "0195f1c2-0000-7000-8000-000000000001",
  snapshot_id: "0195f1c2-0000-7000-8000-0000000000c1",
  edit_id: null,
  provider: "pixiv",
  external_key: "1",
  content_type: "novel",
  title: "前の話",
  author_name: "作者一",
  saved_at: Date.UTC(2026, 0, 2),
  published_at: null,
  updated_at: null,
  text_length: 1200,
  logical_size: 10,
  media_count: 0,
  version_number: 1,
  favorite: false,
  watching: false,
  revised: false,
  image_count: 0,
  file_count: 0,
  excerpt: null,
  tags: [],
  cover: COVER,
  series_title: null,
  series_position: null,
  coverFile: COVER,
};

const summary: ShelfCollectionSummary = {
  id: "0195f1c2-0000-7000-8000-0000000000d1",
  name: "雨の束",
  description: "説明の文",
  ordered: true,
  cover: null,
  cover_image: null,
  cover_mode: "mosaic",
  name_source: "manual",
  track: "manual",
  tiles: [{ provider: "pixiv", external_key: "1", title: "前の話", author_name: "作者一", cover: COVER }],
  revision: 2,
  member_count: 2,
  available_count: 1,
  total_text_length: 1200,
  created_at: Date.UTC(2026, 0, 1),
  updated_at: Date.UTC(2026, 0, 4),
};

const detail: ShelfCollection = {
  ...summary,
  members: [
    {
      provider: "pixiv", external_key: "1", work: card, title: "前の話", author_name: "作者一", cover: COVER,
      text_length: 1200, position: 0, role: "main", added_by: "manual", pinned: false, note: null, missing: false,
      created_at: Date.UTC(2026, 0, 1), updated_at: Date.UTC(2026, 0, 1),
      editions: [{ ...card, id: "0195f1c2-0000-7000-8000-000000000003", external_key: "3", title: "前の話（全文）" }],
    },
    {
      provider: "pixiv", external_key: "2", work: null, title: "後の話", author_name: "足した時の作者", cover: null,
      text_length: 0, position: 1, role: "main", added_by: "manual", pinned: false, note: null, missing: true,
      created_at: Date.UTC(2026, 0, 1), updated_at: Date.UTC(2026, 0, 1), editions: [],
    },
  ],
};

function renderWith(node: React.ReactNode, hash: string) {
  window.location.hash = hash;
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <MantineProvider>
      <ModalsProvider>
        <QueryClientProvider client={client}>
          <AppRouter><WorkspaceProvider>{node}</WorkspaceProvider></AppRouter>
        </QueryClientProvider>
      </ModalsProvider>
    </MantineProvider>,
  );
}

describe("collections on the shelf", () => {
  beforeEach(() => {
    window.localStorage.clear();
    vi.clearAllMocks();
    api.shelfStatus.mockResolvedValue({ state: "ready", path: "x" });
    api.shelfCollection.mockResolvedValue(detail);
    api.shelfCollections.mockResolvedValue([summary]);
  });

  it("opens a collection by its old id and shows its works", async () => {
    renderWith(<CollectionPage />, "#/collections/0195f1c2-0000-7000-8000-0000000000d1");
    expect(await screen.findByRole("heading", { name: "雨の束" })).toBeInTheDocument();
    expect(api.shelfCollection).toHaveBeenCalledWith("0195f1c2-0000-7000-8000-0000000000d1");
    // 棚にある回は一覧と同じカード（棚の作品の画面へ）、無い回は未保存の札。
    expect(screen.getByText("前の話")).toBeInTheDocument();
    expect(screen.getByText("後の話")).toBeInTheDocument();
    expect(screen.getByText("未保存")).toBeInTheDocument();
    expect(screen.getByText("別版 1件")).toBeInTheDocument();
  });

  it("says a collection the new shelf does not have is not found", async () => {
    api.shelfCollection.mockRejectedValue("コレクションが見つかりません");
    renderWith(<CollectionPage />, "#/collections/gone");
    expect(await screen.findByText("コレクションが見つかりません")).toBeInTheDocument();
  });

  it("lists the collections from the shelf", async () => {
    renderWith(<CollectionsPanel />, "#/library?tab=collections");
    expect(await screen.findByText("雨の束")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /雨の束/ })).toHaveAttribute("href", "#/collections/0195f1c2-0000-7000-8000-0000000000d1");
    expect(screen.getByText("1件")).toBeInTheDocument();
    await waitFor(() => expect(api.shelfCollections).toHaveBeenCalled());
  });

  describe("editing", () => {
    beforeEach(() => {
      api.shelfEditCollection.mockResolvedValue(detail);
      suggest.listCollectionSuggestions.mockResolvedValue([]);
    });

    it("edits the collection on the shelf", async () => {
      renderWith(<CollectionPage />, "#/collections/0195f1c2-0000-7000-8000-0000000000d1");
      expect(await screen.findByRole("heading", { name: "雨の束" })).toBeInTheDocument();
      await waitFor(() => expect(screen.getByRole("button", { name: "表紙を変える" })).not.toHaveAttribute("data-disabled"));
      expect(screen.getByRole("button", { name: "一冊のEPUB" })).not.toHaveAttribute("data-disabled");
      expect(screen.getByRole("button", { name: "合う作品を探す" })).not.toHaveAttribute("data-disabled");

      // 並べ替えは作品の鍵で、画面の ID のまま送る。
      fireEvent.click(screen.getByRole("button", { name: "前の話を一つ後へ" }));
      await waitFor(() => expect(api.shelfEditCollection).toHaveBeenCalledWith("0195f1c2-0000-7000-8000-0000000000d1", {
        kind: "reorder",
        works: [{ source: "pixiv", sourceId: "2" }, { source: "pixiv", sourceId: "1" }],
      }));

      // 削除は確認を経て、棚の一回の編集で消す。
      fireEvent.click(screen.getByRole("button", { name: "このコレクションのその他の操作" }));
      fireEvent.click(await screen.findByRole("menuitem", { name: "コレクションを削除" }));
      api.shelfEditCollection.mockResolvedValueOnce(null);
      fireEvent.click(await screen.findByRole("button", { name: "削除する" }));
      await waitFor(() => expect(api.shelfEditCollection).toHaveBeenLastCalledWith("0195f1c2-0000-7000-8000-0000000000d1", { kind: "delete" }));
    });

    it("saves the selection of a newly imported cover", async () => {
      const file = "cd".repeat(32);
      openSingleDialog.mockResolvedValueOnce("C:/画像/表紙.png");
      api.shelfImportEditImage.mockResolvedValueOnce({ file, selection: "cover-selection" });
      renderWith(<CollectionPage />, "#/collections/0195f1c2-0000-7000-8000-0000000000d1");
      fireEvent.click(await screen.findByRole("button", { name: "表紙を変える" }));
      fireEvent.click(await screen.findByRole("button", { name: /画像を選ぶ/ }));
      expect(await screen.findByText(file)).toBeInTheDocument();
      fireEvent.click(screen.getByRole("button", { name: "この表紙にする" }));
      await waitFor(() => expect(api.shelfEditCollection).toHaveBeenCalledWith("0195f1c2-0000-7000-8000-0000000000d1", {
        kind: "look", cover_work: null, cover_mode: "file", cover_image: file, cover_selection: "cover-selection",
      }));
    });

    it("adds the works found for the collection by their keys", async () => {
      suggest.suggestCollectionAdditions.mockResolvedValue({
        collectionId: "0195f1c2-0000-7000-8000-0000000000d1", collectionName: "雨の束", semanticUsed: false, eligibleCount: 1, note: null,
        candidates: [{
          source: "pixiv", sourceId: "9", work: "0195f1c2-0000-7000-8000-000000000009",
          title: "次の話", authorName: "作者一", coverPath: COVER, textLength: 900, publishedAt: "2026-01-05T00:00:00Z",
          confidence: 0.9, reason: "同じシリーズの作品です", evidence: [],
        }],
      });
      renderWith(<CollectionPage />, "#/collections/0195f1c2-0000-7000-8000-0000000000d1");
      expect(await screen.findByRole("heading", { name: "雨の束" })).toBeInTheDocument();
      fireEvent.click(screen.getByRole("button", { name: "合う作品を探す" }));
      await waitFor(() => expect(suggest.suggestCollectionAdditions).toHaveBeenCalledWith("0195f1c2-0000-7000-8000-0000000000d1"));
      fireEvent.click(await screen.findByRole("button", { name: "1作品を追加" }));
      await waitFor(() => expect(api.shelfEditCollection).toHaveBeenCalledWith("0195f1c2-0000-7000-8000-0000000000d1", {
        kind: "add", works: [{ source: "pixiv", sourceId: "9" }],
      }));
    });

    it("seeds a suggestion from the work the work page sent by its shelf UUID", async () => {
      const seed = "0195f1c2-0000-7000-8000-000000000001";
      suggest.generateCollectionSuggestion.mockResolvedValue({});
      renderWith(<CollectionsPanel />, `#/library?tab=collections&suggest=${seed}`);
      await waitFor(() => expect(suggest.generateCollectionSuggestion).toHaveBeenCalledWith([seed]));
      await waitFor(() => expect(window.location.hash).not.toContain("suggest="));
    });

    it("makes a new collection on the shelf", async () => {
      renderWith(<CollectionsPanel />, "#/library?tab=collections");
      expect(await screen.findByText("雨の束")).toBeInTheDocument();
      await waitFor(() => expect(screen.getByRole("button", { name: "新しいコレクション" })).not.toHaveAttribute("aria-disabled"));
      expect(screen.getByRole("button", { name: "まとまりを探す" })).not.toHaveAttribute("data-disabled");
      fireEvent.click(screen.getByRole("button", { name: "新しいコレクション" }));
      fireEvent.change(await screen.findByRole("textbox", { name: "名前" }), { target: { value: "新しい束" } });
      fireEvent.click(screen.getByRole("button", { name: "保存" }));
      await waitFor(() => expect(api.shelfEditCollection).toHaveBeenCalledWith(expect.any(String), {
        kind: "create", title: "新しい束", description: null, ordered: true, works: [],
      }));
    });
  });
});
