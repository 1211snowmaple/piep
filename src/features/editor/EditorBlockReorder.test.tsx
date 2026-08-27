import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen } from "@testing-library/react";
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

function block(order: number, blockType: WorkBlock["blockType"], text: string): WorkBlock {
  return { id: order + 1, editRevisionId: 0, order, blockType, text, assetId: null, attrsJson: null };
}

/** リンクは8番目に置く。エディタは窓に入る分しか描かないので、9番目へ送ると
 *  その行は本当にアンマウントされ、戻すと組み立て直される。 */
function documentWithLinkAtIndexSeven(): EditorDocument {
  const base = getDemoEditor(101);
  const blocks: WorkBlock[] = [
    ...Array.from({ length: 7 }, (_, index) => block(index, "paragraph", `段落${index + 1}`)),
    block(7, "link", "https://example.com/before"),
    block(8, "paragraph", "最後の段落"),
  ];
  return { ...base, blocks };
}

function renderEditor() {
  window.location.hash = "#/editor/101";
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><EditorPage /></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);
}

/**
 * 並べ替えは、いま form が持っている値の上で行わなければならない。
 *
 * 打鍵は `blocks` state を更新しない - 毎打鍵で一覧全体を描き直さないための
 * 判断である。そのため `blocks` は「最後に構造を変えたときの姿」でしかない。
 * 並べ替えがそちらを読んで新しい配列を組むと、**書きかけの本文を巻き戻した
 * うえで並べ替える**ことになり、入力が黙って消える。
 *
 * いまは `insertBlock` / `moveBlock` / `removeBlock` がどれも
 * `form.getValues().blocks` から組んでいるので消えない。ここを「整理」して
 * `blocks` に替えると壊れるので、その一線をこの試験で押さえておく。
 *
 * なお、ブロック操作を挟まない純粋なスクロールでの組み立て直しは、jsdom では
 * 仮想化が働かず再現できない。そちらは手で確かめること。
 */
describe("エディタの並べ替え", () => {
  beforeEach(() => {
    dbApi.getEditorDocument.mockResolvedValue(documentWithLinkAtIndexSeven());
    dbApi.saveWorkDraft.mockResolvedValue(null);
    dbApi.activateWorkEdit.mockResolvedValue(null);
  });

  it("書きかけのURLを持ったまま、行を動かして戻せる", async () => {
    renderEditor();

    const url = await screen.findByLabelText("URL");
    fireEvent.change(url, { target: { value: "https://example.com/after" } });
    expect(await screen.findByDisplayValue("https://example.com/after")).toBeInTheDocument();

    // 9番目へ送る。ここで本当に窓から消えることを確かめておく - 消えていな
    // ければ、この試験は組み立て直しを試せていない。
    fireEvent.click(screen.getByRole("button", { name: "ブロック 8を下へ" }));
    expect(screen.queryByLabelText("URL")).toBeNull();

    // 8番目へ戻す。行はここで新しく組み立てられ、値を取り直す。
    fireEvent.click(screen.getByRole("button", { name: "ブロック 8を下へ" }));

    expect(await screen.findByLabelText("URL")).toHaveValue("https://example.com/after");
  });
});
