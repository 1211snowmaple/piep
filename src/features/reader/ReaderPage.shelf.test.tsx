import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import type { ShelfCard, ShelfNeighbors, ShelfWork } from "@/services/libraryV2Api";
import { writeReadingPosition } from "@/features/library/readingShelf";
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
  libraryV2Neighbors: vi.fn(),
  libraryV2RenderPdfPage: vi.fn(),
}));
vi.mock("@/services/libraryV2Api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/libraryV2Api")>()),
  ...api,
}));
const legacy = vi.hoisted(() => ({
  getReaderMetadata: vi.fn(),
  getReaderContentPage: vi.fn(),
  getDownloads: vi.fn(),
  searchDownloadsV2: vi.fn(),
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
  legacyId: null,
  caption: null,
  authorKey: "77",
  seriesKey: null,
  versions: [
    { snapshot: FIRST, version: 1, saved_at: Date.UTC(2026, 0, 1), text_length: 3 },
    { snapshot: SECOND, version: 2, saved_at: Date.UTC(2026, 0, 2), text_length: 7 },
  ],
};

const PREVIOUS = "0195f1c2-0000-7000-8000-000000000002";
const NEXT = "0195f1c2-0000-7000-8000-000000000003";
const COVER = "ef".repeat(32);

function card(id: string, title: string, extra: Partial<ShelfCard> = {}): ShelfCard {
  return { ...work, id, snapshot_id: id, title, text_length: 1100, excerpt: null, coverFile: null, ...extra };
}

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
    api.libraryV2Neighbors.mockResolvedValue([]);
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

  it("opens the original pages of a PDF attachment from the new shelf", async () => {
    const pdf = "ab".repeat(32);
    api.libraryV2Content.mockResolvedValue({
      page: 0,
      pageCount: 1,
      html: `<div class="attachment-notice">取り込んだ本文 <button type="button" class="attachment-original" data-local-path="${pdf}" data-file-name="本文.pdf">原本を見る</button></div><p>今の本文</p>`,
      totalPlainTextChars: 4,
      sourcePageStarts: [0],
    });
    api.libraryV2RenderPdfPage.mockResolvedValue({ image: "data:image/webp;base64,page", width: 1200, height: 1553, pageCount: 2 });
    renderReader(`#/reader/${WORK}`);
    fireEvent.click(await screen.findByRole("button", { name: "原本を見る" }));
    expect(await screen.findByRole("img", { name: "本文.pdf の 1 ページ目" })).toBeInTheDocument();
    expect(api.libraryV2RenderPdfPage).toHaveBeenCalledWith(WORK, pdf, 0, expect.any(Number));
  });

  it("offers the previous and next works of the series and the author from the new shelf", async () => {
    const rows: ShelfNeighbors[] = [
      { kind: "series", id: "s", title: "続き物", ordered: true, position: 2, total: 3, missing: 0, previous: card(PREVIOUS, "前の回"), next: card(NEXT, "次の回", { coverFile: COVER, author_name: "別名", excerpt: "一行目<br />二行目" }) },
      { kind: "person", id: "p", title: "", ordered: false, position: 5, total: 9, missing: 0, previous: card(PREVIOUS, "前の回"), next: null },
    ];
    api.libraryV2Neighbors.mockResolvedValue(rows);
    writeReadingPosition(NEXT, null, { page: 2, top: 10 });
    renderReader(`#/reader/${WORK}`);
    expect(await screen.findByText("公式シリーズ「続き物」")).toBeInTheDocument();
    expect(api.libraryV2Neighbors).toHaveBeenCalledWith(WORK);
    expect(screen.getByText("2 / 3 · 公式順")).toBeInTheDocument();
    // 名前のない作者の並びは、作品の作者名で名乗る。読む順ではないので「前の作品」と呼ばない。
    expect(screen.getByText("作者の作品")).toBeInTheDocument();
    expect(screen.getByText("9作品の公開順")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "並びの前：前の回" })).toBeInTheDocument();
    const next = screen.getByRole("button", { name: "次の作品：次の回" });
    // 隣の作品の表紙・作者・字数は並びと一緒に届く。読みかけなら続きからと名乗る。
    expect(next.querySelector("img")).toHaveAttribute("src", `http://piep-library.localhost/${COVER}`);
    expect(next).toHaveTextContent("別名 · 1,100字 · 約2分");
    expect(next).toHaveTextContent("読みかけ・続きから");
    // 説明の HTML は地の文にして見せる。
    expect(next).toHaveTextContent("一行目 二行目");
    expect(next).not.toHaveTextContent("<br />");
    expect(legacy.getDownloads).not.toHaveBeenCalled();
    expect(legacy.searchDownloadsV2).not.toHaveBeenCalled();
    fireEvent.click(next);
    await waitFor(() => expect(window.location.hash).toBe(`#/reader/${NEXT}`));
  });

  it("continues a work moved from the old shelf at the place and bookmarks kept under its old number", async () => {
    const OLD = 4321;
    api.libraryV2Work.mockResolvedValue({ ...work, legacyId: OLD });
    api.libraryV2Content.mockImplementation(async (_work: string, _snapshot: string | null, page: number) => ({
      page, pageCount: 3, html: `<p>${page + 1}ページ目の本文</p>`, totalPlainTextChars: 30, sourcePageStarts: [0],
    }));
    // 旧棚の読む画面が憶えた位置としおり（旧棚の番号で引く）。
    writeReadingPosition(OLD, null, { page: 2, top: 0 });
    window.localStorage.setItem("piep.bookmarks.v1", JSON.stringify({
      [String(OLD)]: [{ id: "b1", page: 3, top: 0, label: "3ページ", createdAt: "2026-11-15T00:00:00Z" }],
    }));
    renderReader(`#/reader/${WORK}`);
    expect(await screen.findByText("2ページ目の本文")).toBeInTheDocument();
    expect(api.libraryV2Content).toHaveBeenCalledWith(WORK, null, 1);
    expect(screen.getByRole("button", { name: "しおり一覧（1件）" })).toBeInTheDocument();
    // 新しい棚の ID では書かない（旧棚と別の記録にならない）。
    expect(window.localStorage.getItem(`piep.reader-position.${WORK}.current`)).toBeNull();
  });

  it("marks a neighbour as being read when its old number has a reading position", async () => {
    api.libraryV2Neighbors.mockResolvedValue([
      { kind: "series", id: "s", title: "続き物", ordered: true, position: 1, total: 2, missing: 0, previous: null, next: card(NEXT, "次の回", { legacyId: 77 }) },
    ] satisfies ShelfNeighbors[]);
    writeReadingPosition(77, null, { page: 1, top: 40 });
    renderReader(`#/reader/${WORK}`);
    expect(await screen.findByRole("button", { name: "次の作品：次の回" })).toHaveTextContent("読みかけ・続きから");
  });

  it("reads a saved version by turning its number into that version's snapshot", async () => {
    renderReader(`#/reader/${WORK}?version=1`);
    expect(await screen.findByText("古い版の本文")).toBeInTheDocument();
    await waitFor(() => expect(api.libraryV2Content).toHaveBeenCalledWith(WORK, FIRST, 0));
    expect(api.libraryV2Content).not.toHaveBeenCalledWith(WORK, null, 0);
  });
});
