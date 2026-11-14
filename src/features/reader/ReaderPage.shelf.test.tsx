import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import type { ShelfWork } from "@/services/libraryV2Api";
import ReaderPage from "./ReaderPage";

vi.mock("@tauri-apps/api/core", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@tauri-apps/api/core")>()),
  convertFileSrc: (path: string, protocol: string) => `http://${protocol}.localhost/${path}`,
}));

const api = vi.hoisted(() => ({
  libraryV2Work: vi.fn(),
  libraryV2Content: vi.fn(),
  libraryV2Outline: vi.fn(),
  libraryV2SearchContent: vi.fn(),
}));
vi.mock("@/services/libraryV2Api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/libraryV2Api")>()),
  ...api,
}));
const legacy = vi.hoisted(() => ({
  getReaderMetadata: vi.fn(),
  getReaderContentPage: vi.fn(),
}));
vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  ...legacy,
}));

const WORK = "0195f1c2-0000-7000-8000-000000000001";
const FIRST = "0195f1c2-0000-7000-8000-000000000011";
const SECOND = "0195f1c2-0000-7000-8000-000000000012";
const IMAGE = "cd".repeat(32);

const work: ShelfWork = {
  id: WORK,
  snapshot_id: SECOND,
  edit_id: null,
  provider: "pixiv",
  external_key: "12345",
  content_type: "novel",
  title: "題",
  author_name: "作者",
  saved_at: Date.UTC(2026, 0, 2),
  published_at: Date.UTC(2025, 11, 31),
  updated_at: null,
  text_length: 7,
  logical_size: 4096,
  media_count: 1,
  version_number: 2,
  favorite: false,
  watching: false,
  revised: true,
  image_count: 1,
  file_count: 0,
  excerpt: null,
  tags: [],
  cover: null,
  series_title: null,
  series_position: null,
  coverFile: null,
  caption: null,
  authorKey: "77",
  seriesKey: null,
  versions: [
    { snapshot: FIRST, version: 1, saved_at: Date.UTC(2026, 0, 1), text_length: 3 },
    { snapshot: SECOND, version: 2, saved_at: Date.UTC(2026, 0, 2), text_length: 7 },
  ],
};

function renderReader(hash: string) {
  window.location.hash = hash;
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<MantineProvider><QueryClientProvider client={client}><AppRouter><ReaderPage /></AppRouter></QueryClientProvider></MantineProvider>);
}

describe("ReaderPage on the new shelf", () => {
  beforeEach(() => {
    window.localStorage.clear();
    for (const fn of [...Object.values(api), ...Object.values(legacy)]) fn.mockReset();
    api.libraryV2Work.mockResolvedValue(work);
    api.libraryV2Content.mockImplementation(async (_work: string, snapshot: string | null, page: number) => ({
      page,
      pageCount: 1,
      html: `<p>${snapshot === FIRST ? "古い版の本文" : "今の本文"}</p><img class="novel-image" data-local-path="${IMAGE}" alt="挿絵" />`,
      totalPlainTextChars: 7,
      sourcePageStarts: [0],
    }));
    api.libraryV2Outline.mockResolvedValue([]);
    api.libraryV2SearchContent.mockResolvedValue([]);
  });

  it("reads the body, images and outline from the new shelf and offers no edits", async () => {
    renderReader(`#/reader/${WORK.toUpperCase()}`);
    expect(await screen.findByText("今の本文")).toBeInTheDocument();
    // ID は小文字にそろえて読む（読書位置としおりの鍵も同じ）。
    expect(api.libraryV2Work).toHaveBeenCalledWith(WORK);
    expect(api.libraryV2Content).toHaveBeenCalledWith(WORK, null, 0);
    expect(api.libraryV2Outline).toHaveBeenCalledWith(WORK, null);
    expect(screen.getByAltText("挿絵")).toHaveAttribute("src", `http://piep-library.localhost/${IMAGE}`);
    expect(screen.getByText("新しい棚（試験中）")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "編集" })).toBeDisabled();
    expect(screen.getByRole("button", { name: /保存フォルダー/ })).toBeDisabled();
    expect(legacy.getReaderMetadata).not.toHaveBeenCalled();
    expect(legacy.getReaderContentPage).not.toHaveBeenCalled();
  });

  it("reads a saved version by turning its number into that version's snapshot", async () => {
    renderReader(`#/reader/${WORK}?version=1`);
    expect(await screen.findByText("古い版の本文")).toBeInTheDocument();
    await waitFor(() => expect(api.libraryV2Content).toHaveBeenCalledWith(WORK, FIRST, 0));
    expect(api.libraryV2Content).not.toHaveBeenCalledWith(WORK, null, 0);
  });
});
