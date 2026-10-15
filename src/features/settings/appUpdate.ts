/**
 * アプリ更新まわりの、画面から切り離せる判断。
 *
 * 押したら更新する方式なので、勝手に進む部分は「確認」だけ。その確認を
 * 起動時に走らせるかどうかは設定として残す。
 */

export const APP_UPDATE_CHECK_KEY = "piep.app-update-check";

/**
 * 起動時に確認するか。既定は確認する。
 *
 * 覚えていない・壊れている値は既定に倒す。設定を読めなかったことを理由に
 * 更新の存在を黙って隠すより、確認して知らせるほうが害が小さい。
 */
export function launchCheckEnabled(stored: string | null): boolean {
  return stored !== "0";
}

export function storeLaunchCheck(enabled: boolean): string {
  return enabled ? "1" : "0";
}

/** ダウンロードの進み具合。長さが分からないうちは割合を出さない。 */
export function downloadPercent(downloaded: number, total: number | null): number | null {
  if (total === null || total <= 0) return null;
  return Math.min(100, Math.round((downloaded / total) * 100));
}

/**
 * 配信物に書かれた変更点。
 *
 * リリース本文がそのまま入るので、長さも中身も当てにできない。画面に出すのは
 * 先頭の一部だけにして、続きはリリースページで読んでもらう。
 */
export function summarizeNotes(notes: string | null | undefined, limit = 600): string | null {
  const trimmed = notes?.trim();
  if (!trimmed) return null;
  return trimmed.length > limit ? `${trimmed.slice(0, limit)}…` : trimmed;
}

/**
 * 署名鍵をまだ入れていない状態を、失敗としてではなく手順として伝える。
 *
 * updater は署名が前提で、公開鍵が空のままだと確認そのものが通らない。生の
 * エラー文を出しても何をすればいいのか分からないので、ここで言い換える。
 */
export function describeCheckFailure(message: string): string {
  if (/pubkey|public key|signature|UpdaterDisabled|Updater is disabled/i.test(message)) {
    return `更新の確認に失敗しました。配布物の署名鍵がまだ設定されていない可能性があります（${message}）`;
  }
  if (noReleasePublishedYet(message)) {
    return "配信された更新がまだありません。リリースが下書きのままか、まだ1本も公開されていない状態です。";
  }
  return `更新の確認に失敗しました（${message}）`;
}

/**
 * 更新目録がまだ無い状態。
 *
 * リリースは下書きで作られるので、publish するまで `latest.json` は誰にも
 * 見えず、確認は 404 で終わる。これは壊れているのではなく、まだ配っていない
 * だけなので、赤い失敗として出さない。
 */
export function noReleasePublishedYet(message: string): boolean {
  return /404|not found|Could not fetch a valid release JSON/i.test(message);
}
