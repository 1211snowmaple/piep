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
  discardWorkDraft: vi.fn(),
}));

vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  getEditorDocument: dbApi.getEditorDocument,
  saveWorkDraft: dbApi.saveWorkDraft,
  activateWorkEdit: dbApi.activateWorkEdit,
  discardWorkDraft: dbApi.discardWorkDraft,
}));

function block(order: number, blockType: WorkBlock["blockType"], text: string | null): WorkBlock {
  return { id: order + 1, editRevisionId: 0, order, blockType, text, assetId: null, attrsJson: null };
}

function documentWith(blocks: WorkBlock[], extra: Partial<EditorDocument> = {}): EditorDocument {
  return { ...getDemoEditor(101), blocks, ...extra };
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

describe("書きかけを失わないこと", () => {
  beforeEach(() => {
    window.localStorage.clear();
    dbApi.getEditorDocument.mockResolvedValue(documentWith([
      block(0, "paragraph", "段落1"),
      block(1, "paragraph", "段落2"),
      block(2, "paragraph", "段落3"),
      block(3, "paragraph", "段落4"),
    ]));
    dbApi.saveWorkDraft.mockResolvedValue(null);
    dbApi.activateWorkEdit.mockResolvedValue(null);
    dbApi.discardWorkDraft.mockResolvedValue(undefined);
  });

  /**
   * 探すのは `blocks` ではなく form の値。打鍵では `blocks` を更新しない作りな
   * ので、さっき打った語を探すと 0 件になり、「すべて置換」も押せなかった。
   */
  it("いま打った語を、そのまま探して置き換えられる", async () => {
    renderEditor();
    const first = await screen.findByDisplayValue("段落1");
    fireEvent.change(first, { target: { value: "打鍵した雨の話" } });

    fireEvent.click(screen.getByRole("button", { name: "本文を検索・置換" }));
    fireEvent.change(await screen.findByLabelText("本文から探す"), { target: { value: "雨" } });

    expect(screen.getByText("1件")).toBeInTheDocument();
    const replace = screen.getByRole("button", { name: "すべて置換" });
    expect(replace).toBeEnabled();

    fireEvent.change(screen.getByLabelText("置き換える文字"), { target: { value: "雪" } });
    fireEvent.click(replace);
    expect(await screen.findByDisplayValue("打鍵した雪の話")).toBeInTheDocument();
  });

  /**
   * 控えは「ブロックを消した時点」のもの。そのまま戻すと、そのあとに書いた
   * 文字までまとめて消えていた ―― 段落を1つ戻したいだけで、書いたばかりの
   * 一段落を失う。
   */
  it("ブロックを戻しても、そのあとに書いた文章は残る", async () => {
    renderEditor();
    await screen.findByDisplayValue("段落1");

    fireEvent.click(screen.getByRole("button", { name: "ブロック 4を削除" }));
    fireEvent.change(await screen.findByDisplayValue("段落1"), { target: { value: "段落1＋あとから書き足した一文" } });

    fireEvent.click(screen.getByRole("button", { name: "元に戻す" }));

    expect(await screen.findByDisplayValue("段落4")).toBeInTheDocument();
    expect(screen.getByDisplayValue("段落1＋あとから書き足した一文")).toBeInTheDocument();
  });

  it("やり直しても、同じように文章は残る", async () => {
    renderEditor();
    await screen.findByDisplayValue("段落1");

    fireEvent.click(screen.getByRole("button", { name: "ブロック 4を削除" }));
    fireEvent.click(screen.getByRole("button", { name: "元に戻す" }));
    fireEvent.change(await screen.findByDisplayValue("段落2"), { target: { value: "段落2を書き換えた" } });
    fireEvent.click(screen.getByRole("button", { name: "やり直す" }));

    await waitFor(() => expect(screen.queryByDisplayValue("段落4")).toBeNull());
    expect(screen.getByDisplayValue("段落2を書き換えた")).toBeInTheDocument();
  });
});

describe("書きかけの捨て方と、土台の版", () => {
  beforeEach(() => {
    window.localStorage.clear();
    dbApi.saveWorkDraft.mockResolvedValue(null);
    dbApi.activateWorkEdit.mockResolvedValue(null);
    dbApi.discardWorkDraft.mockResolvedValue(undefined);
  });

  it("下書きがあるときだけ、破棄の口が出る", async () => {
    const base = documentWith([block(0, "paragraph", "書きかけ")]);
    dbApi.getEditorDocument.mockResolvedValueOnce(documentWith([block(0, "paragraph", "取り込んだままの本文")]));
    renderEditor();
    await screen.findByDisplayValue("取り込んだままの本文");
    expect(screen.queryByRole("button", { name: "下書きを破棄" })).toBeNull();

    dbApi.getEditorDocument.mockResolvedValue({
      ...base,
      draftRevision: { id: 9, downloadId: 101, baseVersion: 1, status: "draft", title: null, contentHash: null, createdAt: "", updatedAt: "" },
    });
    // 別の画面として開き直す。
    render(<MantineProvider><Notifications /><QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><ModalsProvider><AppRouter><EditorPage /></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);
    expect(await screen.findByRole("button", { name: "下書きを破棄" })).toBeInTheDocument();
  });

  /**
   * 取り込み直した本文があるのに、編集は古い版の上に載ったまま。反映中の
   * 編集版は読書画面と EPUB を覆うので、新しい本文が黙って隠れていた。
   */
  it("土台の版が古いときは、そう告げる", async () => {
    const demo = getDemoEditor(101);
    dbApi.getEditorDocument.mockResolvedValue({
      ...demo,
      download: { ...demo.download, currentVersion: 3 },
      activeRevision: { id: 5, downloadId: 101, baseVersion: 1, status: "active", title: null, contentHash: null, createdAt: "", updatedAt: "" },
    });
    renderEditor();

    expect(await screen.findByText("取り込み直した本文があります")).toBeInTheDocument();
    expect(screen.getByText(/v1 の本文をもとに/)).toBeInTheDocument();
  });
});
