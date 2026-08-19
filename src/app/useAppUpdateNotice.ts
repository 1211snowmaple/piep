import { useEffect, useRef } from "react";
import { notifications } from "@mantine/notifications";
import { APP_UPDATE_CHECK_KEY, launchCheckEnabled } from "@/features/settings/appUpdate";
import { checkForAppUpdate } from "@/services/appUpdateApi";

/** 一度知らせた版を、画面を移るたびに知らせ直さない。 */
const NOTIFICATION_ID = "app-update-available";

/**
 * 起動時に一度だけ、piep 自身に新しい版が出ていないか確かめる。
 *
 * 見つけても何も入れ替えない。知らせるところまでが自動で、そこから先は
 * 設定 > piepについて で本人が押す。確認に失敗したときは黙る - 使っている
 * 最中に、頼んでもいない通信の失敗を見せられても困る。
 */
export function useAppUpdateNotice(onOpen: () => void) {
  // 開発中の再マウントで二重に走らせない。
  const started = useRef(false);
  const openRef = useRef(onOpen);
  openRef.current = onOpen;

  useEffect(() => {
    if (started.current) return;
    started.current = true;
    // 読めない localStorage で起動時のフックごと落とさない。設定を思い出せ
    // なかったときは既定（確認する）に倒す。
    let stored: string | null = null;
    try { stored = window.localStorage.getItem(APP_UPDATE_CHECK_KEY); } catch { stored = null; }
    if (!launchCheckEnabled(stored)) return;

    let cancelled = false;
    void checkForAppUpdate()
      .then((update) => {
        if (cancelled || !update) return;
        notifications.show({
          id: NOTIFICATION_ID,
          title: `piep v${update.version} が公開されています`,
          message: "設定の「piepについて」から更新できます。",
          color: "piep",
          autoClose: false,
          onClick: () => {
            notifications.hide(NOTIFICATION_ID);
            openRef.current();
          },
        });
      })
      .catch(() => {
        // 確認できないことは、それ自体は使い手の問題ではない。
      });
    return () => { cancelled = true; };
  }, []);
}
