/**
 * 手元の版が、取得元に追いついているか。
 *
 * piep が扱う時間は二つの軸に分かれる。**取得元の時間**（作者が公開した日、
 * 直した日）と、**手元の時間**（この版を取った日、最後に照合した日）である。
 * 利用者が本当に知りたいのは日付そのものではなく、その二つがずれているか
 * どうかで、それは日付ではなく状態として扱ったほうがよい。
 *
 * 判断はこの一箇所に置く。カード・作品ページ・棚が別々に条件を書くと、
 * 同じ作品が画面によって違う顔をすることになる。
 */

/** 取得元と手元のずれ。 */
export type WorkFreshness =
  /** 取得元の最終更新と、手元にある版が一致している。既定の状態。 */
  | "current"
  /** 取得元のほうが新しい。取り直す入口になる。 */
  | "revised"
  /** 取得元の最終更新を、まだ知らない。**「最新」と偽らない。** */
  | "unchecked";

/** 状態を決めるのに要る、作品の断片。 */
export interface FreshnessInput {
  /** 取得元での最終更新。照合できた範囲のもの。 */
  sourceUpdatedAt: string | null;
  /** 改稿の候補が出ているか。取得元のほうが新しいと分かっている印。 */
  hasPendingRevision?: boolean;
}

/**
 * 状態を決める。
 *
 * 改稿が出ているかどうかを最初に見る。更新確認が「取得元のほうが新しい」と
 * 判断したなら、手元の列がどう見えていてもそれが答えである
 * （改稿を見つけたとき、基準値はわざと書き換えない - 取り直して初めて
 * 追いついたと言えるため）。
 */
export function workFreshness(work: FreshnessInput): WorkFreshness {
  if (work.hasPendingRevision) return "revised";
  return work.sourceUpdatedAt ? "current" : "unchecked";
}

/**
 * 「最終更新」を出す意味があるか。
 *
 * 一度も直されていない作品では、最終更新は公開日と同じ瞬間を指す。同じものを
 * 二度書いても情報は増えないので、**その行はそもそも作らない**。常に空欄か
 * 重複になる行を画面に置かない、という決めごとに従う。
 *
 * 書式が違っても（取得元は詳細で `+00:00`、一覧で `+09:00` を返す）
 * 指している瞬間で比べる。読めない値は「語ることがない」に倒す。
 */
export function hasSourceRevision(work: {
  sourceCreatedAt: string | null;
  sourceUpdatedAt: string | null;
}): boolean {
  if (!work.sourceUpdatedAt) return false;
  const updated = new Date(work.sourceUpdatedAt).getTime();
  if (Number.isNaN(updated)) return false;
  if (!work.sourceCreatedAt) return true;
  const created = new Date(work.sourceCreatedAt).getTime();
  if (Number.isNaN(created)) return true;
  return updated > created;
}
