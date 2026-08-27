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
  queryClient.invalidateQueries({ queryKey: ["entity-works"] });
  queryClient.invalidateQueries({ queryKey: ["library-shelf-counts"] });
  queryClient.invalidateQueries({ queryKey: ["dashboard"] });
}
