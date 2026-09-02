import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MantineProvider } from "@mantine/core";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { CollectionAdditionResult, WorkCollection } from "@/types/collections";

/**
 * 「この束に合う作品を探す」。
 *
 * 守るのは、押した通りの顔ぶれが束へ送られるかどうかだけにする。見た目では
 * なく、DB へ渡す中身が要点である。外した作品がそのまま入ってしまう壊れ方は、
 * 画面を見ていても気づけない。
 */
const collectionApi = vi.hoisted(() => ({
  suggestCollectionAdditions: vi.fn(),
}));
vi.mock("@/services/collectionApi", () => collectionApi);
vi.mock("@/services/dbApi", () => ({ isTauriRuntime: () => true, getAssetUrl: (path: string) => path }));
vi.mock("@/components/WorkCover", () => ({ WorkCover: () => <span /> }));

import { CollectionAdditionsModal } from "./CollectionAdditionsModal";

function candidate(index: number, reason = "同じ公式シリーズの作品です") {
  return {
    source: "pixiv",
    sourceId: String(index),
    downloadId: index,
    title: `作品${index}`,
    authorName: "作者",
    coverPath: null,
    textLength: 4000,
    publishedAt: "2026-01-01",
    confidence: 0.9,
    reason,
    evidence: [{ kind: "series_run", label: "同じ公式シリーズ", contribution: 0.52 }],
  };
}

function result(overrides: Partial<CollectionAdditionResult> = {}): CollectionAdditionResult {
  return {
    collectionId: "col-1",
    collectionName: "雨の連作",
    candidates: [candidate(1), candidate(2), candidate(3)],
    semanticUsed: true,
    note: null,
    eligibleCount: 3,
    ...overrides,
  };
}

const collection = { id: "col-1", name: "雨の連作" } as unknown as WorkCollection;

function renderModal(onAdd = vi.fn()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <MantineProvider>
      <QueryClientProvider client={client}>
        <CollectionAdditionsModal
          opened
          onClose={vi.fn()}
          collection={collection}
          busy={false}
          onAdd={onAdd}
        />
      </QueryClientProvider>
    </MantineProvider>,
  );
  return onAdd;
}

describe("CollectionAdditionsModal", () => {
  beforeEach(() => {
    collectionApi.suggestCollectionAdditions.mockReset().mockResolvedValue(result());
  });

  /** 「探す」と書かれたボタンで開いた窓で、もう一度「探す」を押させない。 */
  it("searches as soon as it opens", async () => {
    renderModal();
    await waitFor(() =>
      expect(collectionApi.suggestCollectionAdditions).toHaveBeenCalledWith("col-1"));
  });

  it("sends every candidate that was left in", async () => {
    const onAdd = renderModal();
    await userEvent.click(await screen.findByRole("button", { name: "3作品を追加" }));
    expect(onAdd).toHaveBeenCalledWith([1, 2, 3]);
  });

  /**
   * **外した作品を送らない。** ここを取りこぼすと、束に入れないと決めた作品が
   * そのまま入る。押した内容と DB へ渡すものが食い違う、静かな壊れ方である。
   */
  it("drops the candidate the reader took out", async () => {
    const onAdd = renderModal();
    await userEvent.click(await screen.findByRole("button", { name: "作品2を候補から外す" }));
    // 外したことが画面に出る。戻せることが見えなければ、取り消せない操作に見える。
    expect(await screen.findByRole("button", { name: "作品2を候補に戻す" })).toBeInTheDocument();

    await userEvent.click(await screen.findByRole("button", { name: "2作品を追加" }));
    expect(onAdd).toHaveBeenCalledWith([1, 3]);
  });

  /**
   * **題名を最後まで読めること。** 束へ足す候補は同じ連載の続きであることが
   * 多く、書き出しが同じ作品が並ぶ。1行で切ると、どれも同じ文字列になって
   * 見分けられない。
   */
  it("shows the whole title, not a clipped one", async () => {
    const long = "鉄壁の聖騎士さまが催眠ねっちょりポリネシアンセックスで防御スキルを剥がされる話・後編";
    collectionApi.suggestCollectionAdditions.mockResolvedValue(result({
      candidates: [{ ...candidate(1), title: long }],
    }));
    renderModal();
    // 題名がそのまま出ている（部分一致ではなく完全一致で確かめる）。
    expect(await screen.findByText(long)).toBeInTheDocument();
  });

  it("does not offer to add when nothing is left", async () => {
    renderModal();
    for (const id of [1, 2, 3]) {
      await userEvent.click(await screen.findByRole("button", { name: `作品${id}を候補から外す` }));
    }
    expect(await screen.findByRole("button", { name: "0作品を追加" })).toBeDisabled();
  });

  /**
   * 探せなかったことと、無かったことを混ぜない。意味索引が読めないまま
   * 「合う作品はありません」と出すと、壊れていることが見えなくなる。
   */
  it("says so when it could only search part of the way", async () => {
    collectionApi.suggestCollectionAdditions.mockResolvedValue(result({
      candidates: [],
      semanticUsed: false,
      note: "意味索引が読めないので、題名・作者・タグ・リンクだけで探しました。",
      eligibleCount: 0,
    }));
    renderModal();
    expect(await screen.findByText("一部しか探せていません")).toBeInTheDocument();
    expect(screen.queryByText("いま足せそうな作品はありません")).not.toBeInTheDocument();
  });

  it("says plainly when there is nothing to add", async () => {
    collectionApi.suggestCollectionAdditions.mockResolvedValue(result({
      candidates: [],
      eligibleCount: 0,
    }));
    renderModal();
    expect(await screen.findByText("いま足せそうな作品はありません")).toBeInTheDocument();
  });

  /** 出したのが一部であることを隠さない。少数しか出さない設計の裏返し。 */
  it("admits when more candidates cleared the bar than were shown", async () => {
    collectionApi.suggestCollectionAdditions.mockResolvedValue(result({ eligibleCount: 23 }));
    renderModal();
    expect(await screen.findByText(/条件を満たしたのは23作品/)).toBeInTheDocument();
  });
});
