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

  it("sweeps when the reader asks, and offers to look again", async () => {
    renderModal();
    await userEvent.click(await screen.findByRole("button", { name: "棚から探す" }));
    await waitFor(() => expect(collectionApi.sweepCollectionCandidates).toHaveBeenCalledTimes(1));
    // 二度目には別の束が上がってくる。選び方に乱れが入っているので、
    // 「もう一度探す」に意味がある。
    expect(await screen.findByRole("button", { name: "もう一度探す" })).toBeInTheDocument();
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
