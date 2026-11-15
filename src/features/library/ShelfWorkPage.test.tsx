import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { WorkspaceProvider } from "@/app/WorkspaceContext";
import type { ShelfWork } from "@/services/libraryV2Api";
import WorkPage from "./WorkPage";

vi.mock("@tauri-apps/api/core", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@tauri-apps/api/core")>()),
  convertFileSrc: (path: string, protocol: string) => `http://${protocol}.localhost/${path}`,
}));

const api = vi.hoisted(() => ({
  libraryV2Work: vi.fn(),
  libraryV2Content: vi.fn(),
  libraryV2Media: vi.fn(),
  libraryV2VersionText: vi.fn(),
  libraryV2Original: vi.fn(),
  libraryV2RenderPdfPage: vi.fn(),
}));
vi.mock("@/services/libraryV2Api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/libraryV2Api")>()),
  ...api,
}));
vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
}));

const WORK = "0195f1c2-0000-7000-8000-000000000001";
const FIRST = "0195f1c2-0000-7000-8000-000000000011";
const SECOND = "0195f1c2-0000-7000-8000-000000000012";
const IMAGE = "ab".repeat(32);

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
  favorite: true,
  watching: false,
  revised: true,
  image_count: 1,
  file_count: 0,
  excerpt: null,
  tags: ["雨"],
  cover: null,
  series_title: null,
  series_position: null,
  coverFile: null,
  legacyId: null,
  caption: "説明",
  authorKey: "77",
  seriesKey: null,
  versions: [
    { snapshot: FIRST, version: 1, saved_at: Date.UTC(2026, 0, 1), text_length: 3 },
    { snapshot: SECOND, version: 2, saved_at: Date.UTC(2026, 0, 2), text_length: 7 },
  ],
};

function renderPage(hash: string) {
  window.location.hash = hash;
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><WorkPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);
}

describe("ShelfWorkPage", () => {
  beforeEach(() => {
    for (const fn of Object.values(api)) fn.mockReset();
    api.libraryV2Work.mockResolvedValue(work);
    api.libraryV2Content.mockImplementation(async (_work: string, _snapshot: string | null, page: number) => ({
      page,
      pageCount: 2,
      html: `<p>${page + 1}ページ本文</p><img class="novel-image" data-local-path="${IMAGE}" alt="挿絵" />`,
    }));
    api.libraryV2VersionText.mockImplementation(async (_work: string, snapshot: string) => snapshot === FIRST ? "一行目" : "二行目");
  });

  it("opens a work of the new shelf by its id and reads the body's images by their fingerprint", async () => {
    renderPage(`#/works/${WORK}?tab=content`);
    expect(await screen.findByText("1ページ本文")).toBeInTheDocument();
    expect(api.libraryV2Work).toHaveBeenCalledWith(WORK);
    expect(api.libraryV2Content).toHaveBeenCalledWith(WORK, null, 0);
    expect(screen.getByAltText("挿絵")).toHaveAttribute("src", `http://piep-library.localhost/${IMAGE}`);
    // 読むだけ。変える操作は押せない。
    expect(screen.getByRole("button", { name: "お気に入り（新しい棚では変えられません）" })).toBeDisabled();

    fireEvent.click(screen.getByRole("button", { name: "2" }));
    expect(await screen.findByText("2ページ本文")).toBeInTheDocument();
    expect(api.libraryV2Content).toHaveBeenCalledWith(WORK, null, 1);

    // 読む画面へは、新しい棚の ID のまま進む。
    fireEvent.click(screen.getByRole("button", { name: "読む" }));
    await waitFor(() => expect(window.location.hash).toBe(`#/reader/${WORK}`));
  });

  it("compares a version with the one before it and shows that version's body", async () => {
    renderPage(`#/works/${WORK}?tab=history`);
    await waitFor(() => expect(api.libraryV2VersionText).toHaveBeenCalledWith(WORK, SECOND));
    expect(api.libraryV2VersionText).toHaveBeenCalledWith(WORK, FIRST);
    fireEvent.click(await screen.findByRole("button", { name: /^v1/ }));
    fireEvent.click(await screen.findByRole("button", { name: "この版の本文を見る" }));
    await waitFor(() => expect(api.libraryV2Content).toHaveBeenCalledWith(WORK, FIRST, 0));
    expect(await screen.findByText("保存済み v1")).toBeInTheDocument();
  });

  it("opens a PDF attachment's pages from the body and from the attachment list", async () => {
    const pdf = "cd".repeat(32);
    api.libraryV2Content.mockResolvedValue({
      page: 0,
      pageCount: 1,
      html: `<div class="attachment-notice">取り込んだ本文 <button type="button" class="attachment-original" data-local-path="${pdf}" data-file-name="本文.pdf">原本を見る</button></div><p>本文</p>`,
    });
    api.libraryV2Media.mockResolvedValue([{ kind: "file", role: "attachment", name: "本文.pdf", mime_type: "application/pdf", length: 10, file: pdf }]);
    api.libraryV2RenderPdfPage.mockResolvedValue({ image: "data:image/webp;base64,page", width: 1200, height: 1553, pageCount: 2 });
    renderPage(`#/works/${WORK}?tab=content`);

    fireEvent.click(await screen.findByRole("button", { name: "原本を見る" }));
    expect(await screen.findByRole("img", { name: "本文.pdf の 1 ページ目" })).toBeInTheDocument();
    expect(api.libraryV2RenderPdfPage).toHaveBeenCalledWith(WORK, pdf, 0, expect.any(Number));
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());

    api.libraryV2RenderPdfPage.mockClear();
    fireEvent.click(screen.getByRole("tab", { name: /画像・添付/ }));
    fireEvent.click(await screen.findByText("原本を見る"));
    expect(await screen.findByRole("img", { name: "本文.pdf の 1 ページ目" })).toBeInTheDocument();
  });
});
