import type { QueryClient } from "@tanstack/react-query";

/**
 * 手元の作品が増えたか減ったときに、古くなる画面。
 *
 * 増やす側（Webからの保存・更新ジョブの自動保存）と減らす側（削除）は別々の
 * 場所にあるが、古くなる先は同じである。それぞれが一覧を手で書いていたため、
 * 片方にだけ追記された知らせ先ができ、保存したのにサイドバーの件数が動かない、
 * 更新確認が終わったのに棚が前の一覧のまま、という食い違いが残っていた。
 *
 * `library` はここに含めない。減らしたときは消えた作品を指すカードが残るので
 * `removeQueries` でなければならず、増えたときは `invalidateQueries` でよい。
 * 扱いが違うものを同じ関数に入れると、呼ぶ側がどちらか分からなくなる。
 */
export function invalidateWorkSetViews(queryClient: QueryClient): void {
  queryClient.invalidateQueries({ queryKey: ["library-facets"] });
  queryClient.invalidateQueries({ queryKey: ["library-entities"] });
  // 一覧と件数は対で動く。件数だけ古いと、`ListPager` の最終ページが実在しない
  // 番号を指す。鍵の文字列が違うので前方一致では巻き込めない。
  queryClient.invalidateQueries({ queryKey: ["library-entity-count"] });
  queryClient.invalidateQueries({ queryKey: ["entity-works"] });
  // 作者・シリーズの見出しに出る作品数は `entity` から引いている。作品が
  // 増減したのに触らないと、行は消えたのにバッジは前の数のまま残る。
  // その画面に留まっている限り取り直す引き金が無いので、自分で知らせる。
  queryClient.invalidateQueries({ queryKey: ["entity"] });
  queryClient.invalidateQueries({ queryKey: ["entity-tags"] });
  queryClient.invalidateQueries({ queryKey: ["library-shelf-counts"] });
  queryClient.invalidateQueries({ queryKey: ["dashboard"] });
}

/**
 * ひとつの作品の印（お気に入り・更新監視）が変わったときに、古くなる画面。
 *
 * 同じハートが、押した場所によって直る画面と直らない画面に分かれていた。
 * カードは作者ページ・束・EPUBキューまで知らせていたのに、作品ページは
 * 棚と件数しか知らせていない。**同じ操作なら同じ範囲が古くなる**ので、
 * 一覧はひとつだけ置く。
 */
export function invalidateWorkFlagViews(queryClient: QueryClient, workId: number): Promise<unknown> {
  return Promise.all([
    queryClient.invalidateQueries({ queryKey: ["library"] }),
    queryClient.invalidateQueries({ queryKey: ["library-shelf-counts"] }),
    queryClient.invalidateQueries({ queryKey: ["dashboard"] }),
    queryClient.invalidateQueries({ queryKey: ["entity-works"] }),
    queryClient.invalidateQueries({ queryKey: ["work-collection"] }),
    queryClient.invalidateQueries({ queryKey: ["epub-queue-works"] }),
    queryClient.invalidateQueries({ queryKey: ["reader-metadata", workId] }),
  ]);
}
