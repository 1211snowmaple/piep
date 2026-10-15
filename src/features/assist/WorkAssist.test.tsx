import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  useAssist: vi.fn(),
  suggestTags: vi.fn(),
  workTags: vi.fn(),
  acceptTags: vi.fn(),
  relatedTagCandidates: vi.fn(),
  acceptRelatedTags: vi.fn(),
  removeManualTag: vi.fn(),
  removeAssistedTag: vi.fn(),
  loadNote: vi.fn(),
}));

vi.mock("@/features/assist/useAssist", () => ({ useAssist: mocks.useAssist }));
vi.mock("@/features/assist/AssistLauncher", () => ({
  AssistLauncher: ({ items }: { items: Array<{ id: string; label: string; enabled: boolean; onSelect: () => void }> }) => (
    <div>{items.map((item) => <button key={item.id} disabled={!item.enabled} onClick={item.onSelect}>{item.label}</button>)}</div>
  ),
}));
vi.mock("@/app/PageAssistContext", () => ({ usePageAssist: () => false }));
vi.mock("@/app/router", () => ({ useAppNavigate: () => vi.fn() }));
vi.mock("@/services/dbApi", () => ({ isTauriRuntime: () => true }));
vi.mock("@/services/assistApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/assistApi")>()),
  suggestTags: mocks.suggestTags,
  workTags: mocks.workTags,
  acceptTags: mocks.acceptTags,
  relatedTagCandidates: mocks.relatedTagCandidates,
  acceptRelatedTags: mocks.acceptRelatedTags,
  removeManualTag: mocks.removeManualTag,
  removeAssistedTag: mocks.removeAssistedTag,
  loadNote: mocks.loadNote,
}));

import { WorkAssist } from "@/features/assist/WorkAssist";

const proposal = { tag: "園芸", reason: "花壇の手入れが描かれています" };
const relatedCandidate = { tag: "料理", reason: "他作品のタグ併用から見つかった候補", matchingWorkCount: 12, cooccurringWorkCount: 4 };

function view(downloadId: number, client = new QueryClient({ defaultOptions: { queries: { retry: false } } })) {
  return {
    client,
    element: <MantineProvider><QueryClientProvider client={client}><WorkAssist downloadId={downloadId} /></QueryClientProvider></MantineProvider>,
  };
}

async function openTagModal(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole("button", { name: "タグを補完する" }));
  expect(await screen.findByRole("dialog", { name: "タグの補完" })).toBeInTheDocument();
}

describe("WorkAssist downloadId switch", () => {
  beforeEach(() => {
    mocks.useAssist.mockImplementation((feature: string) => ({
      engine: feature === "work_tagging" ? { model: "test-model" } : null,
      settings: undefined,
      body: false,
      loading: false,
    }));
    mocks.workTags.mockResolvedValue([]);
    mocks.loadNote.mockResolvedValue(null);
    mocks.acceptTags.mockResolvedValue([]);
    mocks.relatedTagCandidates.mockResolvedValue([relatedCandidate]);
    mocks.acceptRelatedTags.mockResolvedValue([{ name: relatedCandidate.tag, source: "manual" }]);
    mocks.removeManualTag.mockResolvedValue([]);
    mocks.removeAssistedTag.mockResolvedValue([]);
  });

  afterEach(() => vi.clearAllMocks());

  it("shows separately stored source context without parsing labels in the note", async () => {
    const text = "保存済みの主欄。\n\n［本文の末尾・確認用の原文］\n\n本文内の欄名。";
    mocks.loadNote.mockResolvedValue({ text, sourceTail: "保存された末尾原文。", modelId: "test-model", promptVersion: "work_synopsis/v11" });
    render(view(1).element);
    expect(await screen.findByText((_, element) => element?.textContent === text)).toBeVisible();
    expect(screen.getByText("本文末尾の原文を確認").closest("details")).not.toHaveAttribute("open");
    expect(screen.getByText("保存された末尾原文。")).not.toBeVisible();
  });

  it("ignores an old pending tag proposal after switching works and offers nothing to adopt", async () => {
    const user = userEvent.setup();
    let resolveOld!: (value: typeof proposal[]) => void;
    mocks.suggestTags.mockReturnValueOnce(new Promise((resolve) => { resolveOld = resolve; }));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const first = view(1, client);
    const rendered = render(first.element);

    await openTagModal(user);
    await user.click(screen.getByRole("button", { name: "タグを提案してもらう" }));
    expect(mocks.suggestTags).toHaveBeenCalledWith({ model: "test-model" }, 1);

    rendered.rerender(view(2, client).element);
    await act(async () => { resolveOld([proposal]); });

    await openTagModal(user);
    expect(screen.queryByText(proposal.tag, { exact: true })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /選んだ\d+件を付ける/ })).not.toBeInTheDocument();
    expect(mocks.acceptTags).not.toHaveBeenCalled();
  });

  it("clears an already selected proposal when switching works", async () => {
    const user = userEvent.setup();
    mocks.suggestTags.mockResolvedValueOnce([proposal]);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const rendered = render(view(1, client).element);

    await openTagModal(user);
    await user.click(screen.getByRole("button", { name: "タグを提案してもらう" }));
    const oldProposal = await screen.findByRole("checkbox", { name: /園芸/ });
    await user.click(oldProposal);
    expect(screen.getByRole("button", { name: "選んだ1件を付ける" })).toBeEnabled();

    rendered.rerender(view(2, client).element);
    await openTagModal(user);

    expect(screen.queryByRole("checkbox", { name: /園芸/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /選んだ\d+件を付ける/ })).not.toBeInTheDocument();
    expect(mocks.acceptTags).not.toHaveBeenCalled();
  });

  it("offers shelf co-occurrence when AI is disabled, and only saves after explicit selection", async () => {
    const user = userEvent.setup();
    mocks.useAssist.mockImplementation(() => ({ engine: null, settings: undefined, body: false, loading: false }));
    render(view(11).element);

    const action = screen.getByRole("button", { name: "棚の併用候補" });
    expect(action).toBeEnabled();
    await user.click(action);
    expect(await screen.findByRole("dialog", { name: "棚の併用候補" })).toBeInTheDocument();
    expect(screen.getByText(/候補作成時はこの作品の本文や概要を読みません/)).toBeInTheDocument();
    expect(mocks.relatedTagCandidates).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "併用候補を読み込む" }));
    expect(await screen.findByRole("checkbox", { name: /料理/ })).toBeInTheDocument();
    expect(mocks.relatedTagCandidates).toHaveBeenCalledWith(11);
    expect(mocks.acceptRelatedTags).not.toHaveBeenCalled();

    await user.click(screen.getByRole("checkbox", { name: /料理/ }));
    await user.click(screen.getByRole("button", { name: "選んだ1件を手動で付ける" }));
    expect(mocks.acceptRelatedTags).toHaveBeenCalledWith(11, ["料理"]);
  });

  it("refreshes the saved tag list when manual adoption errors after a possible commit", async () => {
    const user = userEvent.setup();
    mocks.workTags.mockResolvedValueOnce([]).mockResolvedValueOnce([
      { name: relatedCandidate.tag, source: "manual" },
    ]);
    mocks.acceptRelatedTags.mockRejectedValueOnce(new Error("タグは保存済みですが索引更新に失敗しました"));
    render(view(12).element);

    await user.click(screen.getByRole("button", { name: "棚の併用候補" }));
    await user.click(await screen.findByRole("button", { name: "併用候補を読み込む" }));
    await user.click(await screen.findByRole("checkbox", { name: /料理/ }));
    await user.click(screen.getByRole("button", { name: "選んだ1件を手動で付ける" }));

    expect(await screen.findByText("手動で追加したタグ")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "手動タグ「料理」を外す" })).toBeInTheDocument();
    expect(mocks.workTags).toHaveBeenCalledTimes(2);
  });

  it("does not show an old co-occurrence response after switching works", async () => {
    const user = userEvent.setup();
    let resolveOld!: (value: typeof relatedCandidate[]) => void;
    mocks.relatedTagCandidates.mockReturnValueOnce(new Promise((resolve) => { resolveOld = resolve; }));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const rendered = render(view(21, client).element);

    await user.click(screen.getByRole("button", { name: "棚の併用候補" }));
    await user.click(await screen.findByRole("button", { name: "併用候補を読み込む" }));
    expect(mocks.relatedTagCandidates).toHaveBeenCalledWith(21);

    rendered.rerender(view(22, client).element);
    await act(async () => { resolveOld([relatedCandidate]); });
    await user.click(screen.getByRole("button", { name: "棚の併用候補" }));

    expect(screen.queryByRole("checkbox", { name: /料理/ })).not.toBeInTheDocument();
    expect(mocks.acceptRelatedTags).not.toHaveBeenCalled();
  });

  it("clears selected co-occurrence candidates when switching works", async () => {
    const user = userEvent.setup();
    mocks.relatedTagCandidates.mockResolvedValueOnce([relatedCandidate]);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const rendered = render(view(25, client).element);

    await user.click(screen.getByRole("button", { name: "棚の併用候補" }));
    await user.click(await screen.findByRole("button", { name: "併用候補を読み込む" }));
    await user.click(await screen.findByRole("checkbox", { name: /料理/ }));
    expect(screen.getByRole("button", { name: "選んだ1件を手動で付ける" })).toBeEnabled();

    rendered.rerender(view(26, client).element);
    await user.click(screen.getByRole("button", { name: "棚の併用候補" }));
    expect(screen.queryByRole("checkbox", { name: /料理/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /選んだ\d+件を手動で付ける/ })).not.toBeInTheDocument();
    expect(mocks.acceptRelatedTags).not.toHaveBeenCalled();
  });

  it("clears selection on reload and blocks adoption during and after a failed reload", async () => {
    const user = userEvent.setup();
    let failReload!: () => void;
    mocks.relatedTagCandidates.mockResolvedValueOnce([relatedCandidate]);
    mocks.relatedTagCandidates.mockImplementationOnce(() => new Promise((_, reject) => {
      failReload = () => reject(new Error("試験エラー"));
    }));
    render(view(27).element);

    await user.click(screen.getByRole("button", { name: "棚の併用候補" }));
    await user.click(await screen.findByRole("button", { name: "併用候補を読み込む" }));
    await user.click(await screen.findByRole("checkbox", { name: /料理/ }));
    expect(screen.getByRole("button", { name: "選んだ1件を手動で付ける" })).toBeEnabled();

    await user.click(screen.getByRole("button", { name: "候補を再読み込み" }));
    expect(screen.queryByRole("checkbox", { name: /料理/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /選んだ\d+件を手動で付ける/ })).not.toBeInTheDocument();
    await act(async () => { failReload(); });

    expect(await screen.findByRole("alert")).toHaveTextContent("試験エラー");
    expect(screen.queryByRole("checkbox", { name: /料理/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /選んだ\d+件を手動で付ける/ })).not.toBeInTheDocument();
    expect(mocks.acceptRelatedTags).not.toHaveBeenCalled();
  });

  it("does not adopt a picked tag after it becomes an existing tag", async () => {
    const user = userEvent.setup();
    mocks.relatedTagCandidates.mockResolvedValueOnce([relatedCandidate]);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(view(28, client).element);

    await user.click(screen.getByRole("button", { name: "棚の併用候補" }));
    await user.click(await screen.findByRole("button", { name: "併用候補を読み込む" }));
    await user.click(await screen.findByRole("checkbox", { name: /料理/ }));
    expect(screen.getByRole("button", { name: "選んだ1件を手動で付ける" })).toBeEnabled();

    act(() => client.setQueryData(["assist-tags", 28], [{ name: "料理", source: "manual" }]));
    expect(await screen.findByText("新しく追加できる候補はありません。再読み込みできます。")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /手動で付ける/ })).not.toBeInTheDocument();
    expect(mocks.acceptRelatedTags).not.toHaveBeenCalled();
  });

  it("shows manual tags separately and removes each tag through its matching command", async () => {
    const user = userEvent.setup();
    mocks.workTags.mockResolvedValueOnce([
      { name: "手動候補", source: "manual" },
      { name: "モデル候補", source: "llm" },
      { name: "取得タグ", source: "origin" },
    ]);
    mocks.removeManualTag.mockResolvedValueOnce([{ name: "モデル候補", source: "llm" }]);
    render(view(31).element);

    expect(await screen.findByText("手動で追加したタグ")).toBeInTheDocument();
    expect(screen.getByText("モデルの案から採ったタグ")).toBeInTheDocument();
    expect(screen.getByText("モデル候補")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "手動タグ「手動候補」を外す" }));
    expect(mocks.removeManualTag).toHaveBeenCalledWith(31, "手動候補");
    await user.click(screen.getByRole("button", { name: "モデル採用タグ「モデル候補」を外す" }));
    expect(mocks.removeAssistedTag).toHaveBeenCalledWith(31, "モデル候補");
    expect(mocks.removeManualTag).not.toHaveBeenCalledWith(31, "取得タグ");
  });
});
