import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { Notifications } from "@mantine/notifications";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { getDemoEditor } from "@/mocks/demoData";
import type { EditorDocument, WorkBlock } from "@/types/library";
import EditorPage from "./EditorPage";

const dbApi = vi.hoisted(() => ({
  getEditorDocument: vi.fn(),
  saveWorkDraft: vi.fn(),
  activateWorkEdit: vi.fn(),
}));

vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  getEditorDocument: dbApi.getEditorDocument,
  saveWorkDraft: dbApi.saveWorkDraft,
  activateWorkEdit: dbApi.activateWorkEdit,
}));

function block(order: number, blockType: WorkBlock["blockType"], text: string | null): WorkBlock {
  return { id: order + 1, editRevisionId: 0, order, blockType, text, assetId: null, attrsJson: null };
}

function documentWithParagraphs(count: number): EditorDocument {
  return {
    ...getDemoEditor(101),
    blocks: Array.from({ length: count }, (_, index) => block(index, "paragraph", `段落${index + 1}`)),
  };
}

function renderEditor() {
  window.location.hash = "#/editor/101";
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <MantineProvider>
      <Notifications />
      <QueryClientProvider client={client}>
        <ModalsProvider>
          <AppRouter><EditorPage /></AppRouter>
        </ModalsProvider>
      </QueryClientProvider>
    </MantineProvider>,
  );
}

describe("消したものが戻せること", () => {
  beforeEach(() => {
    window.localStorage.clear();
    dbApi.getEditorDocument.mockResolvedValue(documentWithParagraphs(4));
    dbApi.saveWorkDraft.mockResolvedValue(null);
    dbApi.activateWorkEdit.mockResolvedValue(null);
  });

  /**
   * 文章を扱う画面に取り消しが無いというのが、この編集画面でいちばん痛い
   * 欠けだった。段落を一つ消すと、書いたものは二度と戻らなかった。
   */
  it("削除した段落を元に戻せる", async () => {
    renderEditor();
    await screen.findByDisplayValue("段落1");

    fireEvent.click(screen.getByRole("button", { name: "ブロック 2を削除" }));
    await waitFor(() => expect(screen.queryByDisplayValue("段落2")).toBeNull());

    fireEvent.click(screen.getByRole("button", { name: "元に戻す" }));
    expect(await screen.findByDisplayValue("段落2")).toBeInTheDocument();
  });

  it("戻したものをやり直せる", async () => {
    renderEditor();
    await screen.findByDisplayValue("段落1");

    fireEvent.click(screen.getByRole("button", { name: "ブロック 3を削除" }));
    await waitFor(() => expect(screen.queryByDisplayValue("段落3")).toBeNull());
    fireEvent.click(screen.getByRole("button", { name: "元に戻す" }));
    await screen.findByDisplayValue("段落3");

    fireEvent.click(screen.getByRole("button", { name: "やり直す" }));
    await waitFor(() => expect(screen.queryByDisplayValue("段落3")).toBeNull());
  });

  it("何もしていないうちは戻すものが無い", async () => {
    renderEditor();
    await screen.findByDisplayValue("段落1");
    expect(screen.getByRole("button", { name: "元に戻す" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "やり直す" })).toBeDisabled();
  });
});

describe("保存できないときに、その理由が出ること", () => {
  beforeEach(() => {
    window.localStorage.clear();
    dbApi.getEditorDocument.mockResolvedValue(documentWithParagraphs(3));
    dbApi.saveWorkDraft.mockResolvedValue(null);
    dbApi.activateWorkEdit.mockResolvedValue(null);
  });

  /**
   * 一覧は仮想化されているので、画面の外にあるブロックの赤字は誰にも見えない。
   * 空のブロックが一つあるだけで、保存ボタンは「押しても何も起きないボタン」
   * になっていた。
   */
  it("空のブロックがあると、どれが止めているかを告げる", async () => {
    renderEditor();
    const first = await screen.findByDisplayValue("段落1");

    fireEvent.change(first, { target: { value: "   " } });
    fireEvent.click(screen.getByRole("button", { name: "下書き保存" }));

    expect(await screen.findByText("まだ保存できません")).toBeInTheDocument();
    expect(await screen.findByText(/1番目のブロック/)).toBeInTheDocument();
    expect(dbApi.saveWorkDraft).not.toHaveBeenCalled();
  });

  it("整っていれば保存に出る", async () => {
    renderEditor();
    const first = await screen.findByDisplayValue("段落1");

    fireEvent.change(first, { target: { value: "書き直した段落" } });
    fireEvent.click(screen.getByRole("button", { name: "下書き保存" }));

    await waitFor(() => expect(dbApi.saveWorkDraft).toHaveBeenCalled());
  });
});

describe("題を直せること", () => {
  beforeEach(() => {
    window.localStorage.clear();
    dbApi.getEditorDocument.mockResolvedValue(documentWithParagraphs(2));
    dbApi.saveWorkDraft.mockResolvedValue(null);
  });

  /**
   * 誤字のある題を直す手立てが、これまでどこにも無かった。取得元の題は
   * そのまま残り、直した題が読み手に見える題になる。
   */
  it("直した題を保存に載せる", async () => {
    renderEditor();
    const field = await screen.findByLabelText("この作品のタイトル");
    expect(field).toHaveValue(getDemoEditor(101).download.title);

    fireEvent.change(field, { target: { value: "直した題名" } });
    fireEvent.click(screen.getByRole("button", { name: "下書き保存" }));

    await waitFor(() => expect(dbApi.saveWorkDraft).toHaveBeenCalled());
    const calls = dbApi.saveWorkDraft.mock.calls;
    const [, , title] = calls[calls.length - 1];
    expect(title).toBe("直した題名");
  });
});

describe("本文の置き換え", () => {
  beforeEach(() => {
    window.localStorage.clear();
    dbApi.getEditorDocument.mockResolvedValue({
      ...getDemoEditor(101),
      blocks: [block(0, "paragraph", "雨と雨と雨"), block(1, "paragraph", "晴れ")],
    });
    dbApi.saveWorkDraft.mockResolvedValue(null);
  });

  it("見つけた数を数え、まとめて置き換えられる", async () => {
    renderEditor();
    await screen.findByDisplayValue("雨と雨と雨");

    fireEvent.click(screen.getByRole("button", { name: "本文を検索・置換" }));
    fireEvent.change(await screen.findByLabelText("本文から探す"), { target: { value: "雨" } });
    expect(await screen.findByText("3件")).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("置き換える文字"), { target: { value: "雪" } });
    fireEvent.click(screen.getByRole("button", { name: "すべて置換" }));

    expect(await screen.findByDisplayValue("雪と雪と雪")).toBeInTheDocument();
    // 置き換えも取り消せる。まとめて直したあとで気が変わることはある。
    fireEvent.click(screen.getByRole("button", { name: "元に戻す" }));
    expect(await screen.findByDisplayValue("雨と雨と雨")).toBeInTheDocument();
  });
});
