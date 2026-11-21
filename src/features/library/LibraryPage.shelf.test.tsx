import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { WorkspaceProvider } from "@/app/WorkspaceContext";
import type { ShelfCard } from "@/services/libraryV2Api";
import LibraryPage from "./LibraryPage";

const v2 = vi.hoisted(() => ({
  libraryV2Status: vi.fn(),
  libraryV2Search: vi.fn(),
  libraryV2SetUserState: vi.fn(),
  libraryV2RemoveWork: vi.fn(),
}));
vi.mock("@/services/libraryV2Api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/libraryV2Api")>()),
  ...v2,
}));
const health = vi.hoisted(() => ({ libraryV2FilterFacets: vi.fn(), libraryV2SearchFilterFacets: vi.fn() }));
vi.mock("@/services/shelfHealthApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/shelfHealthApi")>()),
  ...health,
}));
const legacy = vi.hoisted(() => ({ getFilterFacets: vi.fn(), setFlagsForIds: vi.fn(), deleteDownloads: vi.fn() }));
vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  ...legacy,
}));
vi.mock("@/store", () => ({ store: { get: vi.fn().mockResolvedValue(null) } }));

function card(index: number): ShelfCard {
  return {
    id: `0195f1c2-0000-7000-8000-00000000000${index}`,
    snapshot_id: `0195f1c2-0000-7000-8000-00000000001${index}`,
    edit_id: null,
    provider: "pixiv",
    external_key: String(index),
    content_type: "novel",
    title: `棚の作品${index}`,
    author_name: "作者",
    saved_at: Date.UTC(2026, 10, index),
    published_at: null,
    updated_at: null,
    text_length: 10,
    logical_size: 100,
    media_count: 0,
    version_number: 1,
    favorite: false,
    watching: false,
    revised: false,
    image_count: 0,
    file_count: 0,
    excerpt: null,
    tags: [],
    cover: null,
    series_title: null,
    series_position: null,
    coverFile: null,
    legacyId: null,
  } as unknown as ShelfCard;
}

function view() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <MantineProvider env="test">
      <QueryClientProvider client={client}>
        <ModalsProvider>
          <AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter>
        </ModalsProvider>
      </QueryClientProvider>
    </MantineProvider>,
  );
}

describe("新しい棚の一覧の操作", () => {
  beforeAll(() => {
    Element.prototype.scrollIntoView = () => {};
  });
  beforeEach(() => {
    window.location.hash = "#/library";
    v2.libraryV2Status.mockResolvedValue({ state: "ready", path: "C:/shelf" });
    v2.libraryV2Search.mockResolvedValue({ page: { items: [], next: null, total: 2 }, intent: null, matches: [], cards: [card(1), card(2)] });
    v2.libraryV2SetUserState.mockResolvedValue({ favorite: true, watching: false, generation: 2 });
    v2.libraryV2RemoveWork.mockResolvedValue(undefined);
    health.libraryV2FilterFacets.mockResolvedValue({
      tags: [{ name: "雨", count: 2, sources: [] }],
      authors: [{ name: "作者", count: 2, sources: ["pixiv"] }],
      authorEntities: [],
      series: [],
      contentTypes: [{ name: "novel", count: 2, sources: [] }],
      assetTypes: [],
    });
  });
  afterEach(() => {
    localStorage.clear();
    vi.clearAllMocks();
  });

  it("選んだ棚の作品をまとめてお気に入りにし、旧棚には頼まない", async () => {
    view();
    fireEvent.click(await screen.findByRole("button", { name: "複数選択" }));
    fireEvent.click(await screen.findByRole("button", { name: /棚の作品1を選択/ }));
    fireEvent.click(await screen.findByRole("button", { name: /棚の作品2を選択/ }));
    expect((await screen.findAllByText("2件")).length).toBeGreaterThan(0);
    fireEvent.click(screen.getByRole("button", { name: "その他" }));
    fireEvent.click(await screen.findByRole("menuitem", { name: "お気に入りに追加" }));
    await waitFor(() => expect(v2.libraryV2SetUserState).toHaveBeenCalledTimes(2));
    expect(v2.libraryV2SetUserState).toHaveBeenCalledWith(card(1).id, { favorite: true });
    expect(legacy.setFlagsForIds).not.toHaveBeenCalled();
  });

  it("選んだ作品を確かめてから棚から外す", async () => {
    view();
    fireEvent.click(await screen.findByRole("button", { name: "複数選択" }));
    fireEvent.click(await screen.findByRole("button", { name: /棚の作品2を選択/ }));
    fireEvent.click(screen.getByRole("button", { name: "その他" }));
    fireEvent.click(await screen.findByRole("menuitem", { name: "棚から外す" }));
    const dialog = await screen.findByRole("dialog", { name: "選択した作品を棚から外しますか？" });
    fireEvent.click(within(dialog).getByRole("button", { name: "外す" }));
    await waitFor(() => expect(v2.libraryV2RemoveWork).toHaveBeenCalledWith(card(2).id));
    expect(legacy.deleteDownloads).not.toHaveBeenCalled();
  });


  it("絞り込みの選択肢を新しい棚から数える", async () => {
    view();
    fireEvent.click(await screen.findByRole("button", { name: "絞り込み" }));
    await waitFor(() => expect(health.libraryV2FilterFacets).toHaveBeenCalled());
    expect(legacy.getFilterFacets).not.toHaveBeenCalled();
  });
});
