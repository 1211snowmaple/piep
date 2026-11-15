import type { QueryClient } from "@tanstack/react-query";
import { demoCollections } from "@/mocks/demoData";
import { listWorkCollections } from "@/services/collectionApi";
import { isTauriRuntime } from "@/services/dbApi";
import { shelfCollections } from "@/features/library/shelfV2";
import type { WorkCollectionSummary } from "@/types/collections";

/**
 * 棚のコレクション一覧。
 *
 * 同じ鍵で二箇所から読むので、取り方も一箇所に置く。鍵が同じで中身の関数が
 * 違うと、**先に載ったほうが勝つ** — ライブラリの件数表示とタブの中身が、
 * どちらが先にマウントされたかで食い違う。
 *
 * プレビューではデモを返す。空のままにすると、カードの崩れがデスクトップ版
 * でしか見つからない。
 */
export function workCollectionsQueryOptions(shelf = false) {
  // 新しい棚の一覧は鍵を分ける（旧棚の変更で消す鍵 ["work-collections"] の下に置くと、
  // 旧棚を書き換えるたびに読み直す。逆に旧棚の一覧が新しい棚の答えで埋まることもない）。
  return {
    queryKey: shelf ? ["library-v2-collections"] as const : ["work-collections"] as const,
    queryFn: (): Promise<WorkCollectionSummary[]> =>
      shelf
        ? shelfCollections()
        : isTauriRuntime()
          ? listWorkCollections()
          : Promise.resolve(demoCollections as WorkCollectionSummary[]),
  };
}

/**
 * 束の顔ぶれが変わったときに、古くなる画面。
 *
 * この一覧は三箇所（束の詳細・追加のモーダル・棚の一覧）に丸写しされていて、
 * リーダーの「同じまとまりの作品」だけがどれにも入っていなかった。外した作品が
 * 読み終わりの欄に残り、押すと続きとして開く。**写経をやめて一箇所に置く。**
 */
export function invalidateCollectionViews(queryClient: QueryClient): void {
  queryClient.invalidateQueries({ queryKey: ["work-collections"] });
  queryClient.invalidateQueries({ queryKey: ["work-collection"] });
  queryClient.invalidateQueries({ queryKey: ["collections-for-work"] });
  queryClient.invalidateQueries({ queryKey: ["collections-for-person"] });
  queryClient.invalidateQueries({ queryKey: ["reader-member-collections"] });
}
