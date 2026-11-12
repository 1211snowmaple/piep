// 読み取り開始と書き込み完了の順を覚える。古い応答で楽観更新を巻き戻さない。
let revision = 0;

export const nextFavoriteRevision = (): number => ++revision;
export const favoriteReadRevision = (work: { favoriteReadSequence?: number }): number => work.favoriteReadSequence ?? 0;
export function markFavoriteRead<T extends object>(work: T, value: number): T {
  // QueryClientの構造共有がオブジェクトを組み直しても、取得順を失わない。
  Object.assign(work, { favoriteReadSequence: value });
  return work;
}
