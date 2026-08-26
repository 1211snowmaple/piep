import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { AddToCollectionModal } from "@/features/collections/AddToCollectionModal";

vi.mock("@/services/collectionApi", () => ({
  listWorkCollections: vi.fn(),
  addDownloadsToCollection: vi.fn(),
  createCollectionFromDownloads: vi.fn(),
}));
vi.mock("@/services/dbApi", () => ({
  isTauriRuntime: () => true,
  getAssetUrl: (path: string | null) => path,
}));

import * as collectionApi from "@/services/collectionApi";

const api = collectionApi as unknown as {
  listWorkCollections: ReturnType<typeof vi.fn>;
  addDownloadsToCollection: ReturnType<typeof vi.fn>;
  createCollectionFromDownloads: ReturnType<typeof vi.fn>;
};

function existing(id: string, name: string) {
  return {
    id,
    name,
    description: null,
    collectionKind: "ordered" as const,
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
    totalTextLength: 100,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
  };
}

function renderModal(downloadIds = [11, 12, 13]) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <MantineProvider>
      <QueryClientProvider client={client}>
        <AppRouter>
          <AddToCollectionModal opened onClose={vi.fn()} downloadIds={downloadIds} />
        </AppRouter>
      </QueryClientProvider>
    </MantineProvider>,
  );
}

describe("AddToCollectionModal", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    window.location.hash = "#/library";
    api.listWorkCollections.mockResolvedValue([existing("collection-1", "雨の記憶")]);
    api.createCollectionFromDownloads.mockResolvedValue({ ...existing("collection-9", "新しい束"), members: [] });
    api.addDownloadsToCollection.mockResolvedValue({ ...existing("collection-1", "雨の記憶"), members: [] });
  });

  it("sends only the row ids, leaving the ordering to the saving side", async () => {
    renderModal();
    fireEvent.change(screen.getByLabelText(/^名前/), { target: { value: "新しい束" } });
    fireEvent.click(screen.getByRole("button", { name: "3作品を入れる" }));
    await waitFor(() =>
      expect(api.createCollectionFromDownloads).toHaveBeenCalledWith("新しい束", "ordered", [11, 12, 13]),
    );
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
      expect(api.addDownloadsToCollection).toHaveBeenCalledWith("collection-1", [11, 12, 13]),
    );
    expect(api.createCollectionFromDownloads).not.toHaveBeenCalled();
  });

  it("starts on 新規 even after the listing arrives, so the default never moves", async () => {
    renderModal();
    await screen.findByText("雨の記憶");
    // 読み込みの前後で選択が動くと、開いた直後に押した人だけ違う束へ入る。
    expect(screen.getByLabelText(/^名前/)).toBeInTheDocument();
  });
});
