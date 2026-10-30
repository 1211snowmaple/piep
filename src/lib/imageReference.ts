/** Identify a Pixiv illustration without guessing an invalid page number. */
export function imageReferenceInfo(reference: string): { url?: string; issue?: string } {
  const match = /^\[pixivimage:(id=)?(\d+)(?:-(\d+))?\]$/.exec(reference);
  if (!match) return {};
  return {
    url: `https://www.pixiv.net/artworks/${match[2]}`,
    issue: match[1] ? "取得元の本文で画像の指定方法が誤っています。"
      : match[3] && Number(match[3]) === 0 ? "取得元の本文が0ページ目を指定しています。画像のページは1から始まります。" : undefined,
  };
}
