import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { WorkspaceProvider } from "@/app/WorkspaceContext";
import type { ShelfWork } from "@/services/shelfApi";
import WorkPage from "./WorkPage";

vi.mock("@tauri-apps/api/core", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@tauri-apps/api/core")>()),
  convertFileSrc: (path: string, protocol: string) => `http://${protocol}.localhost/${path}`,
}));

const api = vi.hoisted(() => ({
  shelfWork: vi.fn(),
  shelfContent: vi.fn(),
  shelfMedia: vi.fn(),
  shelfRevisionSnapshot: vi.fn(),
  shelfOriginal: vi.fn(),
  shelfRenderPdfPage: vi.fn(),
  shelfStatus: vi.fn(),
  shelfEditHistory: vi.fn(),
  shelfEditPointer: vi.fn(),
  shelfSetUserState: vi.fn(),
  shelfCollections: vi.fn(),
  shelfWorkCollections: vi.fn(),
  shelfEditCollection: vi.fn(),
}));
vi.mock("@/services/shelfApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/shelfApi")>()),
  ...api,
}));
vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
}));
const updates = vi.hoisted(() => ({ listPendingRevisionsCommand: vi.fn(), previewPendingRevisionCommand: vi.fn() }));
vi.mock("@/services/updateJobApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/updateJobApi")>()),
  ...updates,
}));
const runSingleCheck = vi.hoisted(() => vi.fn());
vi.mock("@/features/updates/startSingleCheck", () => ({ runSingleCheck }));

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
  caption: "説明",
  authorKey: "77",
  seriesKey: null,
  editBaseVersion: null,
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
  afterEach(() => { Reflect.deleteProperty(window, "__TAURI_INTERNALS__"); });
  beforeEach(() => {
    for (const fn of Object.values(api)) fn.mockReset();
    window.localStorage.clear();
    runSingleCheck.mockReset().mockResolvedValue(undefined);
    updates.listPendingRevisionsCommand.mockReset().mockResolvedValue([]);
    updates.previewPendingRevisionCommand.mockReset();
    api.shelfWork.mockResolvedValue(work);
    api.shelfEditHistory.mockResolvedValue({ generation: 0, active: null, draft: null, archived: [] });
    api.shelfContent.mockImplementation(async (_work: string, _snapshot: string | null, page: number) => ({
      page,
      pageCount: 2,
      html: `<p>${page + 1}ページ本文</p><img class="novel-image" data-local-path="${IMAGE}" alt="挿絵" />`,
    }));
    api.shelfRevisionSnapshot.mockImplementation(async (_work: string, snapshot: string) => ({
      text: snapshot === FIRST ? "一行目" : "二行目",
      metadata: { title: "題", caption: "説明", tags: [], publishedAt: null, updatedAt: null, assets: [], assetsKnown: true },
    }));
  });

  it("opens a work of the shelf by its id and reads the body's images by their fingerprint", async () => {
    renderPage(`#/works/${WORK}?tab=content`);
    expect(await screen.findByText("1ページ本文")).toBeInTheDocument();
    expect(api.shelfWork).toHaveBeenCalledWith(WORK);
    expect(api.shelfContent).toHaveBeenCalledWith(WORK, null, 0);
    expect(screen.getByAltText("挿絵")).toHaveAttribute("src", `http://piep-library.localhost/${IMAGE}`);

    fireEvent.click(screen.getByRole("button", { name: "2" }));
    expect(await screen.findByText("2ページ本文")).toBeInTheDocument();
    expect(api.shelfContent).toHaveBeenCalledWith(WORK, null, 1);

    // 読む画面へは、棚の ID のまま進む。
    fireEvent.click(screen.getByRole("button", { name: "読む" }));
    await waitFor(() => expect(window.location.hash).toBe(`#/reader/${WORK}`));
  });

  it("compares a version with the one before it and shows that version's body", async () => {
    renderPage(`#/works/${WORK}?tab=history`);
    await waitFor(() => expect(api.shelfRevisionSnapshot).toHaveBeenCalledWith(WORK, SECOND));
    expect(api.shelfRevisionSnapshot).toHaveBeenCalledWith(WORK, FIRST);
    fireEvent.click(await screen.findByRole("button", { name: /^v1/ }));
    fireEvent.click(await screen.findByRole("button", { name: "この版の本文を見る" }));
    await waitFor(() => expect(api.shelfContent).toHaveBeenCalledWith(WORK, FIRST, 0));
    expect(await screen.findByText("保存済み v1")).toBeInTheDocument();
  });

  it("compares an unsaved rewrite with the current saved version without saving it", async () => {
    updates.listPendingRevisionsCommand.mockResolvedValue([{ work: WORK, foundAt: "2026-09-01T00:00:00Z" }]);
    updates.previewPendingRevisionCommand.mockResolvedValue({ work: WORK, baseVersion: 2, text: "二行目と追記", textLength: 6 });
    renderPage(`#/works/${WORK}?tab=history&compare=pending`);

    expect(await screen.findByText("保存済み v2 → 取得元の改稿（未保存） の変更")).toBeInTheDocument();
    expect(updates.previewPendingRevisionCommand).toHaveBeenCalledWith(WORK);
    expect(api.shelfRevisionSnapshot).toHaveBeenCalledWith(WORK, SECOND);
    expect(api.shelfRevisionSnapshot).not.toHaveBeenCalledWith(WORK, FIRST);
    expect(screen.getByRole("button", { name: /取得元の改稿/ })).toHaveAttribute("data-active");
    expect(Array.from(document.querySelectorAll('ins[data-kind="added"]')).map((node) => node.textContent).join("")).toContain("追記");

    // 保存した版を選べば、いつもの版どうしの比較へ戻る。
    fireEvent.click(screen.getByRole("button", { name: /^v2/ }));
    await waitFor(() => expect(api.shelfRevisionSnapshot).toHaveBeenCalledWith(WORK, FIRST));
    expect(screen.getByRole("button", { name: /取得元の改稿/ })).not.toHaveAttribute("data-active");
    expect(updates.previewPendingRevisionCommand).toHaveBeenCalledTimes(1);
  });

  it("shows metadata and saved image changes even when source text is identical", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
    const oldImage = "cd".repeat(32);
    api.shelfRevisionSnapshot.mockImplementation(async (_work: string, snapshot: string) => ({
      text: "同じ本文",
      metadata: {
        title: "題", caption: snapshot === FIRST ? "古い説明" : "新しい説明", tags: [snapshot === FIRST ? "古いタグ" : "新しいタグ"],
        publishedAt: null, updatedAt: null, assetsKnown: true,
        assets: [{ key: "pixiv-uploaded:1", kind: "illustrations", label: "1.jpg", source: "", available: true,
          contentHash: snapshot === FIRST ? oldImage : IMAGE, localPath: `shelf:${snapshot === FIRST ? oldImage : IMAGE}` }],
      },
    }));
    renderPage(`#/works/${WORK}?tab=history`);
    expect(await screen.findByText("本文の変更はありません。")).toBeInTheDocument();
    const metadata = screen.getByLabelText("作品情報の差分");
    expect(metadata).toHaveTextContent("古い説明");
    expect(metadata).toHaveTextContent("新しい説明");
    expect(metadata).toHaveTextContent("古いタグ");
    expect(metadata).toHaveTextContent("新しいタグ");
    expect(screen.getByAltText("変更前の挿絵")).toHaveAttribute("src", `http://piep-library.localhost/${oldImage}`);
    expect(screen.getByAltText("変更後の挿絵")).toHaveAttribute("src", `http://piep-library.localhost/${IMAGE}`);
  });

  it("does not compare an unacquired body as a deletion and keeps that version's source JSON selectable", async () => {
    api.shelfRevisionSnapshot.mockImplementation(async (_work: string, snapshot: string) => ({
      text: snapshot === FIRST ? null : "取得した本文",
      metadata: { title: snapshot === FIRST ? "旧題" : "新題", caption: null, tags: null, publishedAt: null, updatedAt: null, assets: [], assetsKnown: false },
    }));
    api.shelfOriginal.mockImplementation(async (_work: string, snapshot: string | null) => JSON.stringify({ title: snapshot === FIRST ? "選んだ原本" : "現在の原本" }));
    renderPage(`#/works/${WORK}?tab=history`);
    expect(await screen.findByText("一方の版に本文が保存されていないため、本文の差分は比較できません。")).toBeInTheDocument();
    expect(screen.getByLabelText("作品情報の差分")).toHaveTextContent("旧題");
    expect(screen.getByLabelText("作品情報の差分")).toHaveTextContent("新題");
    expect(screen.queryByText("本文の変更はありません。")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /^v1/ }));
    await waitFor(() => expect(screen.getByRole("button", { name: "この版の本文を見る" })).toBeDisabled());
    fireEvent.click(screen.getByRole("button", { name: "この版の保存データを見る" }));
    expect(await screen.findByText("保存済み v1 の保存データ")).toBeInTheDocument();
    await waitFor(() => expect(api.shelfOriginal).toHaveBeenCalledWith(WORK, FIRST));
    expect(await screen.findByText(/選んだ原本/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "現在の保存データへ" }));
    await waitFor(() => expect(api.shelfOriginal).toHaveBeenCalledWith(WORK, null));
    expect(await screen.findByText(/現在の原本/)).toBeInTheDocument();
  });

  it("checks just this work by its shelf UUID", async () => {
    api.shelfStatus.mockResolvedValue({ state: "ready", path: "x" });
    renderPage(`#/works/${WORK}`);
    const button = await screen.findByRole("button", { name: "更新を確認" });
    await waitFor(() => expect(button).toBeEnabled());
    fireEvent.click(button);
    await waitFor(() => expect(runSingleCheck).toHaveBeenCalledWith({ kind: "work", workId: WORK, label: work.title }, expect.any(Function)));
  });

  it("adds this work to a collection and seeds a suggestion from it", async () => {
    api.shelfStatus.mockResolvedValue({ state: "ready", path: "x" });
    const summary = (id: string, name: string) => ({
      id, name, description: null, ordered: true, cover: null, cover_image: null, cover_mode: "mosaic",
      name_source: "manual", track: "manual", tiles: [], revision: 1, member_count: 1, available_count: 1,
      total_text_length: 10, created_at: 0, updated_at: 0,
    });
    const IN = "0195f1c2-0000-7000-8000-0000000000d1";
    const OUT = "0195f1c2-0000-7000-8000-0000000000d2";
    api.shelfCollections.mockResolvedValue([summary(IN, "入っている束"), summary(OUT, "まだの束")]);
    api.shelfWorkCollections.mockResolvedValue([summary(IN, "入っている束")]);
    api.shelfEditCollection.mockResolvedValue({ ...summary(OUT, "まだの束"), members: [] });
    renderPage(`#/works/${WORK}`);
    const menu = await screen.findByRole("button", { name: "コレクション（1件に追加済み）" });
    await waitFor(() => expect(api.shelfCollections).toHaveBeenCalled());
    fireEvent.click(menu);
    fireEvent.click(await screen.findByRole("menuitem", { name: "まだの束" }));
    expect(screen.getByRole("menuitem", { name: "入っている束（追加済み）", hidden: true })).toBeDisabled();
    await waitFor(() => expect(api.shelfEditCollection).toHaveBeenCalledWith(OUT, {
      kind: "add", works: [{ source: "pixiv", sourceId: "12345" }],
    }));
  });

  it("seeds a collection suggestion from this work by its shelf UUID", async () => {
    api.shelfStatus.mockResolvedValue({ state: "ready", path: "x" });
    api.shelfCollections.mockResolvedValue([]);
    api.shelfWorkCollections.mockResolvedValue([]);
    renderPage(`#/works/${WORK}`);
    const menu = await screen.findByRole("button", { name: "コレクションに追加" });
    await waitFor(() => expect(api.shelfCollections).toHaveBeenCalled());
    fireEvent.click(menu);
    fireEvent.click(await screen.findByRole("menuitem", { name: "関連作品からひな型を作る" }));
    await waitFor(() => expect(window.location.hash).toBe(`#/library?tab=collections&suggest=${WORK}`));
  });


  it("changes this work's watching flag and reloads the saved state", async () => {
    api.shelfStatus.mockResolvedValue({ state: "ready", path: "x" });
    api.shelfSetUserState.mockImplementation(async (_id: string, { watching }: { watching: boolean }) => {
      api.shelfWork.mockResolvedValue({ ...work, watching });
      return { watching, favorite: false, generation: 1 };
    });
    renderPage(`#/works/${WORK}`);
    const toggle = await screen.findByRole("switch", { name: "更新監視" });
    expect(toggle).not.toBeChecked();
    fireEvent.click(toggle);
    await waitFor(() => expect(api.shelfSetUserState).toHaveBeenCalledWith(WORK, { watching: true }));
    await waitFor(() => { expect(toggle).toBeEnabled(); expect(toggle).toBeChecked(); });
    expect(api.shelfWork.mock.calls.length).toBeGreaterThan(1);
    fireEvent.click(toggle);
    await waitFor(() => expect(api.shelfSetUserState).toHaveBeenCalledWith(WORK, { watching: false }));
    await waitFor(() => { expect(toggle).toBeEnabled(); expect(toggle).not.toBeChecked(); });
  });


  it("restores the current watching flag if a write fails", async () => {
    api.shelfStatus.mockResolvedValue({ state: "ready", path: "x" });
    api.shelfSetUserState.mockRejectedValue(new Error("競合"));
    renderPage(`#/works/${WORK}`);
    const toggle = await screen.findByRole("switch", { name: "更新監視" });
    fireEvent.click(toggle);
    await waitFor(() => expect(api.shelfSetUserState).toHaveBeenCalledWith(WORK, { watching: true }));
    await waitFor(() => { expect(toggle).toBeEnabled(); expect(toggle).not.toBeChecked(); });
  });

  it("says which version an edit was made on and that a later one is not shown", async () => {
    api.shelfWork.mockResolvedValue({ ...work, edit_id: FIRST, editBaseVersion: 1 });
    renderPage(`#/works/${WORK}?tab=content`);
    expect(await screen.findByText("v1 をもとに編集した本文です。取り込み済みの v2 は表示されていません。")).toBeInTheDocument();
  });

  it("re-publishes an earlier edit and takes one off the list", async () => {
    api.shelfStatus.mockResolvedValue({ state: "ready", path: "x" });
    const OLD = "0195f1c2-0000-7000-8000-0000000000e1";
    const OLDER = "0195f1c2-0000-7000-8000-0000000000e2";
    api.shelfEditHistory.mockResolvedValue({
      generation: 6,
      active: null,
      draft: { id: "0195f1c2-0000-7000-8000-0000000000d1", baseVersion: 2, title: null, createdAt: Date.UTC(2026, 0, 4) },
      archived: [
        { id: OLD, baseVersion: 2, title: null, createdAt: Date.UTC(2026, 0, 3) },
        { id: OLDER, baseVersion: 1, title: null, createdAt: Date.UTC(2026, 0, 2) },
      ],
    });
    api.shelfEditPointer.mockResolvedValue({ active: OLD, draft: null, generation: 7 });
    renderPage(`#/works/${WORK}?tab=history`);

    // 下書きは作品の画面でも知らせる。
    expect(await screen.findByText("未反映")).toBeInTheDocument();
    const restore = await screen.findAllByRole("button", { name: "もう一度反映" });
    fireEvent.click(restore[0]);
    fireEvent.click(await screen.findByRole("button", { name: "反映する" }));
    await waitFor(() => expect(api.shelfEditPointer).toHaveBeenCalledWith(WORK, 6, { kind: "restore", edit: OLD }));

    fireEvent.click(screen.getAllByRole("button", { name: "一覧から外す" })[1]);
    fireEvent.click(await screen.findByRole("button", { name: "外す" }));
    await waitFor(() => expect(api.shelfEditPointer).toHaveBeenCalledWith(WORK, 6, { kind: "forget", edit: OLDER }));

    fireEvent.click(screen.getByRole("button", { name: "すべて外す" }));
    expect(await screen.findByText("以前の編集版を2件すべて一覧から外しますか？")).toBeInTheDocument();
  });


  it("opens a PDF attachment's pages from the body and from the attachment list", async () => {
    const pdf = "cd".repeat(32);
    api.shelfContent.mockResolvedValue({
      page: 0,
      pageCount: 1,
      html: `<div class="attachment-notice">取り込んだ本文 <button type="button" class="attachment-original" data-local-path="${pdf}" data-file-name="本文.pdf">原本を見る</button></div><p>本文</p>`,
    });
    api.shelfMedia.mockResolvedValue([{ kind: "file", role: "attachment", name: "本文.pdf", mime_type: "application/pdf", length: 10, file: pdf }]);
    api.shelfRenderPdfPage.mockResolvedValue({ image: "data:image/webp;base64,page", width: 1200, height: 1553, pageCount: 2 });
    renderPage(`#/works/${WORK}?tab=content`);

    fireEvent.click(await screen.findByRole("button", { name: "原本を見る" }));
    expect(await screen.findByRole("img", { name: "本文.pdf の 1 ページ目" })).toBeInTheDocument();
    expect(api.shelfRenderPdfPage).toHaveBeenCalledWith(WORK, pdf, 0, expect.any(Number));
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());

    api.shelfRenderPdfPage.mockClear();
    fireEvent.click(screen.getByRole("tab", { name: /画像・添付/ }));
    fireEvent.click(await screen.findByText("原本を見る"));
    expect(await screen.findByRole("img", { name: "本文.pdf の 1 ページ目" })).toBeInTheDocument();
  });
});
