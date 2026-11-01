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
import { buildProofreadBatches, replaceOnce } from "./ProofreadPanel";

const dbApi = vi.hoisted(() => ({
  getEditorDocument: vi.fn(),
  saveWorkDraft: vi.fn(),
  activateWorkEdit: vi.fn(),
  discardWorkDraft: vi.fn(),
}));
const assist = vi.hoisted(() => ({ proofreadSegments: vi.fn(), allowBody: true }));

vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  getEditorDocument: dbApi.getEditorDocument,
  saveWorkDraft: dbApi.saveWorkDraft,
  activateWorkEdit: dbApi.activateWorkEdit,
  discardWorkDraft: dbApi.discardWorkDraft,
}));
vi.mock("@/services/assistApi", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/services/assistApi")>();
  const baseUrl = "http://127.0.0.1:8080/v1";
  const model = "local-model";
  return {
    ...actual,
    loadAssistSettings: async () => ({
      ...actual.DEFAULT_ASSIST_SETTINGS,
      enabled: true,
      baseUrl,
      model,
      allowBody: assist.allowBody,
      verifiedTarget: actual.assistTarget({ baseUrl, model }),
    }),
    proofreadSegments: assist.proofreadSegments,
  };
});

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

describe("校正に送る単位", () => {
  it("文字を持つブロックだけを、上限の字数でまとめる", () => {
    const batches = buildProofreadBatches([
      { clientId: "a", blockType: "paragraph", text: "あ".repeat(40) },
      { clientId: "img", blockType: "image", text: "キャプション" },
      { clientId: "b", blockType: "heading", text: "い".repeat(40) },
      { clientId: "c", blockType: "paragraph", text: "   " },
      { clientId: "d", blockType: "quote", text: "う".repeat(40) },
    ], 100);
    expect(batches.map((batch) => batch.map((segment) => segment.id))).toEqual([["a", "b"], ["d"]]);
  });

  it("長いブロックは行の切れ目で分け、どれも同じブロックを名乗る", () => {
    const text = ["一".repeat(60), "二".repeat(60), "三".repeat(60)].join("\n");
    const batches = buildProofreadBatches([{ clientId: "long", blockType: "paragraph", text }], 100);
    const segments = batches.flat();
    expect(segments.every((segment) => segment.id === "long")).toBe(true);
    expect(segments.map((segment) => segment.text).join("\n")).toBe(text);
  });
});

describe("候補を当てる", () => {
  it("ちょうど一度だけ現れる箇所だけを置き換える", () => {
    expect(replaceOnce("部屋は暗らかった。", "暗らかった", "暗かった")).toBe("部屋は暗かった。");
    expect(replaceOnce("あいう。あいう。", "あいう", "あいえ")).toBeNull();
    expect(replaceOnce("部屋は暗かった。", "暗らかった", "暗かった")).toBeNull();
  });
});

describe("編集画面の校正", () => {
  beforeEach(() => {
    window.localStorage.clear();
    assist.allowBody = true;
    assist.proofreadSegments.mockReset();
    dbApi.getEditorDocument.mockResolvedValue(documentWith([
      block(0, "paragraph", "彼は静かに扉を開けた。部屋は暗らかった。"),
      block(1, "paragraph", "二つ目の段落。"),
    ]));
    dbApi.saveWorkDraft.mockResolvedValue(null);
  });

  it("押した候補だけが本文に入り、元に戻せる", async () => {
    assist.proofreadSegments.mockImplementation(async (_engine, segments: { id: string; text: string }[]) => {
      const target = segments.find((segment) => segment.text.includes("暗らかった"));
      return target ? [{ segmentId: target.id, before: "部屋は暗らかった", after: "部屋は暗かった", kind: "誤字", reason: "「暗い」の活用" }] : [];
    });
    renderEditor();
    const first = await screen.findByLabelText("文章 1");
    fireEvent.click(screen.getByRole("button", { name: "編集で使えるAIの手伝い" }));
    fireEvent.click(await screen.findByText("本文全体を校正"));

    const apply = await screen.findByRole("button", { name: "直す" });
    // 押す前は本文は変わらない。
    expect(first).toHaveValue("彼は静かに扉を開けた。部屋は暗らかった。");
    expect(assist.proofreadSegments).toHaveBeenCalledTimes(1);
    fireEvent.click(apply);

    await waitFor(() => expect(screen.getByLabelText("文章 1")).toHaveValue("彼は静かに扉を開けた。部屋は暗かった。"));
    expect(screen.getByText("直しました")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "元に戻す" }));
    await waitFor(() => expect(screen.getByLabelText("文章 1")).toHaveValue("彼は静かに扉を開けた。部屋は暗らかった。"));
  });

  it("候補のあとで書き換えた箇所には当てない", async () => {
    assist.proofreadSegments.mockImplementation(async (_engine, segments: { id: string; text: string }[]) => {
      const target = segments.find((segment) => segment.text.includes("暗らかった"));
      return target ? [{ segmentId: target.id, before: "部屋は暗らかった", after: "部屋は暗かった", kind: "誤字", reason: "" }] : [];
    });
    renderEditor();
    const first = await screen.findByLabelText("文章 1");
    fireEvent.click(screen.getByRole("button", { name: "編集で使えるAIの手伝い" }));
    fireEvent.click(await screen.findByText("本文全体を校正"));
    const apply = await screen.findByRole("button", { name: "直す" });
    fireEvent.change(first, { target: { value: "書き直した段落。" } });
    fireEvent.click(apply);
    expect(await screen.findByText("本文が変わったため当てられません")).toBeInTheDocument();
    expect(screen.getByLabelText("文章 1")).toHaveValue("書き直した段落。");
  });

  it("本文を送る許可が無ければ、理由を出して押せない", async () => {
    assist.allowBody = false;
    renderEditor();
    await screen.findByLabelText("文章 1");
    fireEvent.click(screen.getByRole("button", { name: "編集で使えるAIの手伝い" }));
    expect(await screen.findAllByText("本文を送る許可が要ります（設定の「AIの手伝い」）")).not.toHaveLength(0);
    expect(assist.proofreadSegments).not.toHaveBeenCalled();
  });
});
