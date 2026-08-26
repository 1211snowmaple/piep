import { demoCollections } from "@/mocks/demoData";
import { listWorkCollections } from "@/services/collectionApi";
import { isTauriRuntime } from "@/services/dbApi";
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
export function workCollectionsQueryOptions() {
  return {
    queryKey: ["work-collections"] as const,
    queryFn: (): Promise<WorkCollectionSummary[]> =>
      isTauriRuntime()
        ? listWorkCollections()
        : Promise.resolve(demoCollections as WorkCollectionSummary[]),
  };
}
