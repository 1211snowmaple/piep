import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MantineProvider } from "@mantine/core";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { CollectionSuggestion } from "@/types/collections";

/**
 * 束の提案を採るか外すかは、**DB を書き換える操作**である。この画面は
 * 0.10.0 で作り直された領域のうち最も大きく、いちばん動くのに、テストが
 * 一つも無かった。ここで守るのは「押した通りの顔ぶれが送られるか」だけに
 * 絞る - 見た目ではなく、DB へ渡す中身が要点だから。
 */
const collectionApi = vi.hoisted(() => ({
  listCollectionSuggestions: vi.fn(),
  acceptCollectionSuggestion: vi.fn(),
  rejectCollectionSuggestion: vi.fn(),
  dismissCollectionSuggestion: vi.fn(),
  dismissSweptSuggestions: vi.fn(),
  // 名前の上書き判定は純粋な関数なので、本物と同じ振る舞いを置く。
  suggestionNameOverride: (proposed: string, selected: string) => (selected === proposed ? undefined : selected),
}));
vi.mock("@/services/collectionApi", () => collectionApi);
vi.mock("@/services/dbApi", () => ({ isTauriRuntime: () => true, getAssetUrl: (path: string) => path }));
vi.mock("@/services/assistApi", () => ({ nameCollectionSuggestion: vi.fn() }));
vi.mock("@/features/assist/useAssist", () => ({ useAssist: () => ({ engine: null }) }));
vi.mock("@/app/router", () => ({ useAppNavigate: () => vi.fn() }));
vi.mock("@/components/WorkCover", () => ({ WorkCover: () => <span /> }));

import { SuggestionInbox } from "@/features/collections/SuggestionInbox";

function member(index: number) {
  return {
    source: "pixiv",
    sourceId: String(index),
    downloadId: index,
    title: `作品${index}`,
    authorName: "作者",
    coverPath: null,
    position: index,
  };
}

function suggestion(): CollectionSuggestion {
  return {
    id: "sug-1",
    track: "sequence",
    proposedName: "雨の連作",
    reason: "題名の話数が続いている",
    confidence: 0.9,
    nameOptions: [{ name: "雨の連作", source: "rule" }],
    members: [member(1), member(2), member(3)],
  } as unknown as CollectionSuggestion;
}

function renderInbox() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <MantineProvider>
      <QueryClientProvider client={client}>
        <SuggestionInbox sweeping={false} savedSearchIdeas={[]} onSweep={vi.fn()} />
      </QueryClientProvider>
    </MantineProvider>,
  );
}

describe("SuggestionInbox", () => {
  beforeEach(() => {
    collectionApi.listCollectionSuggestions.mockReset().mockResolvedValue([suggestion()]);
    collectionApi.acceptCollectionSuggestion.mockReset().mockResolvedValue({ id: "col-1" });
    collectionApi.rejectCollectionSuggestion.mockReset().mockResolvedValue(undefined);
    collectionApi.dismissCollectionSuggestion.mockReset().mockResolvedValue(undefined);
  });

  it("採用すると、提案の顔ぶれをそのまま渡す", async () => {
    renderInbox();
    const accept = await screen.findByRole("button", { name: "3作品で作る" });
    await userEvent.click(accept);

    await waitFor(() => expect(collectionApi.acceptCollectionSuggestion).toHaveBeenCalled());
    const payload = collectionApi.acceptCollectionSuggestion.mock.calls[0]?.[0];
    expect(payload.suggestionId).toBe("sug-1");
    expect(payload.memberKeys).toEqual([
      { source: "pixiv", sourceId: "1" },
      { source: "pixiv", sourceId: "2" },
      { source: "pixiv", sourceId: "3" },
    ]);
  });

  /**
   * **外した作品を送らない。** ここを取りこぼすと、束から抜いたはずの作品が
   * そのまま入る（採用）か、抜いたつもりの作品まで「今後出さない」に
   * なる（除外）。押した内容と DB へ渡すものが食い違う、いちばん静かな壊れ方。
   */
  it("外した作品は、採用の顔ぶれから抜ける", async () => {
    renderInbox();
    const drop = await screen.findByRole("button", { name: "作品2を外す" });
    await userEvent.click(drop);
    // 外したことが画面に出る（戻す側の名前に変わる）。
    expect(await screen.findByRole("button", { name: "作品2を戻す" })).toBeInTheDocument();

    // 押せるボタンの文言そのものが、送る顔ぶれの数を名乗る。
    await userEvent.click(await screen.findByRole("button", { name: "2作品で作る" }));
    await waitFor(() => expect(collectionApi.acceptCollectionSuggestion).toHaveBeenCalled());
    const payload = collectionApi.acceptCollectionSuggestion.mock.calls[0]?.[0];
    expect(payload.memberKeys).toEqual([
      { source: "pixiv", sourceId: "1" },
      { source: "pixiv", sourceId: "3" },
    ]);
  });

  it("閉じるは、その提案だけを閉じる", async () => {
    renderInbox();
    await userEvent.click(await screen.findByRole("button", { name: "あとで" }));
    await waitFor(() => expect(collectionApi.dismissCollectionSuggestion).toHaveBeenCalledWith("sug-1"));
    expect(collectionApi.acceptCollectionSuggestion).not.toHaveBeenCalled();
    expect(collectionApi.rejectCollectionSuggestion).not.toHaveBeenCalled();
  });
});
