import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MantineProvider } from "@mantine/core";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

/**
 * 「まとまりを探す」をオーバーレイにした。
 *
 * 棚の一覧の途中に結果を差し込んでいたころは、候補が出ているあいだずっと
 * コレクションそのものが下へ押し出されていた。候補は**片付けるもの**なので、
 * 棚の上に居座る種類の情報ではない。
 *
 * ここで守るのは、走査を始めるまでは走らせないことと、走っている最中に
 * 閉じられないこと。押していないのに棚を舐めはじめる窓は、開くのが怖い。
 */
const collectionApi = vi.hoisted(() => ({
  sweepCollectionCandidates: vi.fn(),
  listCollectionSuggestions: vi.fn(),
  dismissSweptSuggestions: vi.fn(),
  acceptCollectionSuggestion: vi.fn(),
  rejectCollectionSuggestion: vi.fn(),
  dismissCollectionSuggestion: vi.fn(),
  suggestionNameOverride: (proposed: string, selected: string) => (selected === proposed ? undefined : selected),
}));
vi.mock("@/services/collectionApi", () => collectionApi);
vi.mock("@/services/dbApi", () => ({ isTauriRuntime: () => true, getAssetUrl: (path: string) => path }));
vi.mock("@/services/assistApi", () => ({ nameCollectionSuggestion: vi.fn() }));
vi.mock("@/features/assist/useAssist", () => ({ useAssist: () => ({ engine: null }) }));
vi.mock("@/app/router", () => ({ useAppNavigate: () => vi.fn() }));
vi.mock("@/components/WorkCover", () => ({ WorkCover: () => <span /> }));

import { CollectionSweepModal } from "./CollectionSweepModal";

/** 候補が1件ある状態。カードが描けるだけの最小限を埋める。 */
function oneSuggestion() {
  return [{
    id: "sug-1",
    proposedName: "雨の連作",
    nameOptions: [{ source: "title", name: "雨の連作", label: "題名の共通部分" }],
    collectionKind: "ordered",
    track: "sequence",
    origin: "sweep",
    evidenceSummary: "題名が連番になっている2作です",
    score: 0.9,
    ruleVersion: "test",
    state: "pending",
    members: [1, 2].map((index) => ({
      source: "pixiv",
      sourceId: String(index),
      downloadId: index,
      title: `作品${index}`,
      authorName: "作者",
      coverPath: null,
      textLength: 1000,
      proposedPosition: index,
      score: 1,
      selected: true,
      evidence: [],
    })),
    createdAt: "2026-09-01",
    updatedAt: "2026-09-01",
  }];
}

function renderModal(onClose = vi.fn()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <MantineProvider>
      <QueryClientProvider client={client}>
        <CollectionSweepModal opened onClose={onClose} />
      </QueryClientProvider>
    </MantineProvider>,
  );
  return onClose;
}

describe("CollectionSweepModal", () => {
  beforeEach(() => {
    collectionApi.listCollectionSuggestions.mockReset().mockResolvedValue([]);
    collectionApi.dismissSweptSuggestions.mockReset().mockResolvedValue(1);
    collectionApi.sweepCollectionCandidates.mockReset().mockResolvedValue({
      bundles: [],
      savedSearchSuggestions: [],
      semanticUsed: true,
      note: null,
    });
  });

  /** 開いただけで棚を舐めない。走査は意味索引を全部読む、重い操作である。 */
  it("does not sweep until it is asked to", async () => {
    renderModal();
    expect(await screen.findByRole("button", { name: "棚から探す" })).toBeInTheDocument();
    expect(collectionApi.sweepCollectionCandidates).not.toHaveBeenCalled();
  });

  it("sweeps when the reader asks", async () => {
    renderModal();
    await userEvent.click(await screen.findByRole("button", { name: "棚から探す" }));
    await waitFor(() => expect(collectionApi.sweepCollectionCandidates).toHaveBeenCalledTimes(1));
  });

  /**
   * 走査は毎回、確度を重みにした籤で選び直す。だから「更新」は同じものを
   * 取り直す操作ではなく、**別の顔ぶれを引き直す**操作である。候補があるうちは
   * その名前で出す。
   */
  it("calls the button 更新 once there is something to re-roll", async () => {
    collectionApi.listCollectionSuggestions.mockResolvedValue(oneSuggestion());
    renderModal();
    expect(await screen.findByRole("button", { name: "更新" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "棚から探す" })).not.toBeInTheDocument();
  });

  /**
   * **閉じることが、片付けることである。**
   *
   * 候補は下書きで、閉じるとは見終わったということ。それを二つの操作へ分けると、
   * 窓を閉じたのに数字だけが棚の入口に残る。✕ を押したらそのまま片付く。
   */
  it("clears the candidates when the window is closed", async () => {
    collectionApi.listCollectionSuggestions.mockResolvedValue(oneSuggestion());
    const onClose = renderModal();
    await screen.findByRole("button", { name: "更新" });

    // ✕ の名前が、何が起きるかを言っている。
    await userEvent.click(screen.getByRole("button", { name: "閉じて候補を片付ける" }));
    expect(onClose).toHaveBeenCalled();
    await waitFor(() => expect(collectionApi.dismissSweptSuggestions).toHaveBeenCalled());
  });

  /** 押すものは一つだけ。片付けるための別のボタンは置かない。 */
  it("keeps a single action, and no separate close-all button", async () => {
    collectionApi.listCollectionSuggestions.mockResolvedValue(oneSuggestion());
    renderModal();
    expect(await screen.findByRole("button", { name: "更新" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "すべて閉じる" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "まとめて" })).not.toBeInTheDocument();
  });

  /** 片付けるものが無いときに、空の書き込みを送らない。 */
  it("does not write anything when there was nothing to clear", async () => {
    const onClose = renderModal();
    await screen.findByRole("button", { name: "棚から探す" });
    // 片付けるものが無いときは、名前も「閉じる」だけになる。
    await userEvent.click(screen.getByRole("button", { name: "閉じる" }));
    expect(onClose).toHaveBeenCalled();
    expect(collectionApi.dismissSweptSuggestions).not.toHaveBeenCalled();
  });

  /**
   * 探せなかったことを、見つからなかったことにしない。意味索引が読めない棚で
   * 「まとまりはありません」とだけ出すと、壊れていることが表に出ない。
   */
  it("shows why the sweep could only go part of the way", async () => {
    collectionApi.sweepCollectionCandidates.mockResolvedValue({
      bundles: [],
      savedSearchSuggestions: [],
      semanticUsed: false,
      note: "意味索引が読めないので、題材の束は探せませんでした。続き物だけを出しています。",
    });
    renderModal();
    await userEvent.click(await screen.findByRole("button", { name: "棚から探す" }));
    expect(await screen.findByText("一部しか探せていません")).toBeInTheDocument();
  });

  it("says plainly when the shelf has nothing worth offering", async () => {
    renderModal();
    expect(await screen.findByText("いま出せるまとまりはありません")).toBeInTheDocument();
  });
});
