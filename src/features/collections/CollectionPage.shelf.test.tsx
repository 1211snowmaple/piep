import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { WorkspaceProvider } from "@/app/WorkspaceContext";
import { SHELF_V2_PREFERENCE_KEY } from "@/features/library/shelfV2";
import type { ShelfCard, ShelfCollection, ShelfCollectionSummary } from "@/services/libraryV2Api";
import CollectionPage from "./CollectionPage";
import { CollectionsPanel } from "./CollectionsPanel";

vi.mock("@tauri-apps/api/core", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@tauri-apps/api/core")>()),
  convertFileSrc: (path: string, protocol: string) => `http://${protocol}.localhost/${path}`,
}));

const api = vi.hoisted(() => ({
  libraryV2Status: vi.fn(),
  libraryV2Collection: vi.fn(),
  libraryV2Collections: vi.fn(),
  libraryV2EditCollection: vi.fn(),
}));
vi.mock("@/services/libraryV2Api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/libraryV2Api")>()),
  ...api,
}));
// 旧棚の命令は一つも呼ばれないこと。
const legacy = vi.hoisted(() => ({
  getWorkCollection: vi.fn(),
  listWorkCollections: vi.fn(),
  listCollectionSuggestions: vi.fn(),
}));
vi.mock("@/services/collectionApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/collectionApi")>()),
  ...legacy,
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
  legacyId: 41,
};

const summary: ShelfCollectionSummary = {
  id: "0195f1c2-0000-7000-8000-0000000000d1",
  legacy_id: "c-old",
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
      editions: [{ ...card, id: "0195f1c2-0000-7000-8000-000000000003", external_key: "3", title: "前の話（全文）", legacyId: null }],
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

describe("collections on the new shelf", () => {
  beforeEach(() => {
    window.localStorage.clear();
    window.localStorage.setItem(SHELF_V2_PREFERENCE_KEY, "true");
    vi.clearAllMocks();
    api.libraryV2Status.mockResolvedValue({ state: "ready", path: "x", unresolved: [] });
    api.libraryV2Collection.mockResolvedValue(detail);
    api.libraryV2Collections.mockResolvedValue([summary]);
  });

  it("opens a collection by its old id and shows its works read-only", async () => {
    renderWith(<CollectionPage />, "#/collections/c-old");
    expect(await screen.findByRole("heading", { name: "雨の束" })).toBeInTheDocument();
    expect(api.libraryV2Collection).toHaveBeenCalledWith("c-old");
    // 棚にある回は一覧と同じカード（新しい棚の作品の画面へ）、無い回は未保存の札。
    expect(screen.getByText("前の話")).toBeInTheDocument();
    expect(screen.getByText("後の話")).toBeInTheDocument();
    expect(screen.getByText("未保存")).toBeInTheDocument();
    expect(screen.getByText("別版 1件")).toBeInTheDocument();
    expect(screen.getByText("新しい棚（試験中）")).toBeInTheDocument();
    // 書き換える操作は押せない形で、理由を添える。押しても窓は開かない。
    const add = screen.getByRole("button", { name: "作品を追加（新しい棚では変えられません）" });
    expect(add).toHaveAttribute("aria-disabled", "true");
    fireEvent.click(add);
    expect(screen.queryByText("コレクションに作品を追加")).toBeNull();
    expect(screen.getByRole("button", { name: "一冊のEPUB（新しい棚では変えられません）" })).toHaveAttribute("data-disabled", "true");
    expect(screen.getByRole("button", { name: "このコレクションのその他の操作（新しい棚では変えられません）" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "前の話を一つ後へ（新しい棚では変えられません）" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "前の話をコレクションから外す（新しい棚では変えられません）" })).toBeInTheDocument();
    for (const call of Object.values(legacy)) expect(call).not.toHaveBeenCalled();
  });

  it("says a collection the new shelf does not have is not found", async () => {
    api.libraryV2Collection.mockRejectedValue("コレクションが見つかりません");
    renderWith(<CollectionPage />, "#/collections/gone");
    expect(await screen.findByText("コレクションが見つかりません")).toBeInTheDocument();
  });

  it("lists the collections from the new shelf and keeps making and finding them locked", async () => {
    renderWith(<CollectionsPanel />, "#/library?tab=collections");
    expect(await screen.findByText("雨の束")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /雨の束/ })).toHaveAttribute("href", "#/collections/c-old");
    expect(screen.getByText("1件")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "新しいコレクション（新しい棚では変えられません）" })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByRole("button", { name: "まとまりを探す（新しい棚では変えられません）" })).toBeInTheDocument();
    await waitFor(() => expect(api.libraryV2Collections).toHaveBeenCalled());
    for (const [name, call] of Object.entries(legacy)) expect(call, name).not.toHaveBeenCalled();
  });

  describe("once the shelf is switched over", () => {
    beforeEach(() => {
      api.libraryV2Status.mockResolvedValue({ state: "ready", path: "x", unresolved: [], switched: true });
      api.libraryV2EditCollection.mockResolvedValue(detail);
    });

    it("edits the collection on the new shelf and keeps what is not connected yet locked", async () => {
      renderWith(<CollectionPage />, "#/collections/c-old");
      expect(await screen.findByRole("heading", { name: "雨の束" })).toBeInTheDocument();
      // 表紙・EPUB・探す は、切り替えた棚でもまだ押せない（理由が変わる）。
      await waitFor(() => expect(screen.getByRole("button", { name: "一冊のEPUB（新しい棚ではまだ使えません）" })).toHaveAttribute("data-disabled", "true"));
      expect(screen.getByRole("button", { name: "表紙を変える（新しい棚ではまだ使えません）" })).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "合う作品を探す（新しい棚ではまだ使えません）" })).toBeInTheDocument();

      // 並べ替えは作品の鍵で、画面の ID（旧棚の ID）のまま送る。
      fireEvent.click(screen.getByRole("button", { name: "前の話を一つ後へ" }));
      await waitFor(() => expect(api.libraryV2EditCollection).toHaveBeenCalledWith("c-old", {
        kind: "reorder",
        works: [{ source: "pixiv", sourceId: "2" }, { source: "pixiv", sourceId: "1" }],
      }));

      // 削除は確認を経て、新しい棚の一回の編集で消す。
      fireEvent.click(screen.getByRole("button", { name: "このコレクションのその他の操作" }));
      fireEvent.click(await screen.findByRole("menuitem", { name: "コレクションを削除" }));
      api.libraryV2EditCollection.mockResolvedValueOnce(null);
      fireEvent.click(await screen.findByRole("button", { name: "削除する" }));
      await waitFor(() => expect(api.libraryV2EditCollection).toHaveBeenLastCalledWith("c-old", { kind: "delete" }));
      for (const [name, call] of Object.entries(legacy)) expect(call, name).not.toHaveBeenCalled();
    });

    it("makes a new collection on the new shelf", async () => {
      renderWith(<CollectionsPanel />, "#/library?tab=collections");
      expect(await screen.findByText("雨の束")).toBeInTheDocument();
      await waitFor(() => expect(screen.getByRole("button", { name: "新しいコレクション" })).not.toHaveAttribute("aria-disabled"));
      expect(screen.getByRole("button", { name: "まとまりを探す（新しい棚ではまだ使えません）" })).toBeInTheDocument();
      fireEvent.click(screen.getByRole("button", { name: "新しいコレクション" }));
      fireEvent.change(await screen.findByRole("textbox", { name: "名前" }), { target: { value: "新しい束" } });
      fireEvent.click(screen.getByRole("button", { name: "保存" }));
      await waitFor(() => expect(api.libraryV2EditCollection).toHaveBeenCalledWith(expect.any(String), {
        kind: "create", title: "新しい束", description: null, ordered: true, works: [],
      }));
      for (const [name, call] of Object.entries(legacy)) expect(call, name).not.toHaveBeenCalled();
    });
  });
});
