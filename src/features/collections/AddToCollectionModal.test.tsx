import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { AddToCollectionModal } from "@/features/collections/AddToCollectionModal";

const api = vi.hoisted(() => ({ shelfCollections: vi.fn(), editShelfCollection: vi.fn() }));
vi.mock("@/features/library/shelf", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/features/library/shelf")>()),
  shelfCollections: api.shelfCollections,
  editShelfCollection: api.editShelfCollection,
  invalidateShelf: vi.fn(),
}));
vi.mock("@/services/dbApi", () => ({
  isTauriRuntime: () => true,
  getAssetUrl: (path: string | null) => path,
}));

const WORKS = [
  { source: "pixiv", sourceId: "11" },
  { source: "pixiv", sourceId: "12" },
  { source: "fanbox", sourceId: "13" },
];

function existing(id: string, name: string) {
  return {
    id,
    name,
    description: null,
    collectionKind: "ordered" as const,
    coverPath: null,
    coverMode: "mosaic" as const,
    coverImagePath: null,
    coverTiles: [],
    nameSource: "manual" as const,
    track: "manual" as const,
    revision: 1,
    memberCount: 2,
    availableCount: 2,
    totalTextLength: 100,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
  };
}

function renderModal(works = WORKS) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <MantineProvider>
      <QueryClientProvider client={client}>
        <AppRouter>
          <AddToCollectionModal opened onClose={vi.fn()} works={works} />
        </AppRouter>
      </QueryClientProvider>
    </MantineProvider>,
  );
}

describe("AddToCollectionModal", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    window.location.hash = "#/library";
    api.shelfCollections.mockResolvedValue([existing("collection-1", "雨の記憶")]);
    api.editShelfCollection.mockImplementation(async (id: string, change: { kind: string; title?: string }) =>
      ({ ...existing(id, change.title ?? "雨の記憶"), members: [] }));
  });

  it("sends only the works, leaving the ordering to the shelf", async () => {
    renderModal();
    fireEvent.change(screen.getByLabelText(/^名前/), { target: { value: "新しい束" } });
    fireEvent.click(screen.getByRole("button", { name: "3作品を入れる" }));
    await waitFor(() =>
      expect(api.editShelfCollection).toHaveBeenCalledWith(expect.any(String), { kind: "create", title: "新しい束", description: null, ordered: true, works: WORKS }),
    );
    // 読む順のある束は、棚が投稿日順に並べる。
    const created = api.editShelfCollection.mock.calls[0][0];
    await waitFor(() => expect(api.editShelfCollection).toHaveBeenCalledWith(created, { kind: "sort", by: "published" }));
  });

  it("refuses to create a collection with no name rather than inventing one", () => {
    renderModal();
    expect(screen.getByRole("button", { name: "名前を入れてください" })).toBeDisabled();
  });

  it("adds to an existing collection when one is picked", async () => {
    renderModal();
    await screen.findByText("雨の記憶");
    fireEvent.click(screen.getByText("雨の記憶"));
    fireEvent.click(screen.getByRole("button", { name: "3作品を入れる" }));
    await waitFor(() =>
      expect(api.editShelfCollection).toHaveBeenCalledWith("collection-1", { kind: "add", works: WORKS }),
    );
    expect(api.editShelfCollection).toHaveBeenCalledTimes(1);
  });

  it("starts on 新規 even after the listing arrives, so the default never moves", async () => {
    renderModal();
    await screen.findByText("雨の記憶");
    // 読み込みの前後で選択が動くと、開いた直後に押した人だけ違う束へ入る。
    expect(screen.getByLabelText(/^名前/)).toBeInTheDocument();
  });

  it("shows listing failures and prevents creation until the list recovers", async () => {
    api.shelfCollections.mockRejectedValueOnce(new Error("database unavailable"));
    renderModal();
    expect(await screen.findByText("database unavailable")).toBeInTheDocument();
    expect(screen.queryByText("まだコレクションはありません")).toBeNull();
    fireEvent.change(screen.getByLabelText(/^名前/), { target: { value: "新しい束" } });
    expect(screen.getByRole("button", { name: "コレクションを読み込めません" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "再読み込み" }));
    expect(await screen.findByText("雨の記憶")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "3作品を入れる" })).toBeEnabled();
  });
});
