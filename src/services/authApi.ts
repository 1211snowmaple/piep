import { invoke } from "@tauri-apps/api/core";

/**
 * pixiv の利用者。
 *
 * `profile_image_urls` だけ snake_case なのは、Rust 側の `PixivUser` に
 * `rename_all` が付いていないからである（`src-tauri/src/pixiv_api/models.rs`）。
 * 中身は pixiv の応答をそのまま通した JSON なので、形は約束されていない。
 */
export interface PixivUser {
  id: string;
  name: string;
  profile_image_urls?: { medium?: string };
}

/** FANBOX の利用者。 */
export interface FanboxUser {
  userId: string;
  name: string;
  iconUrl?: string | null;
}

/**
 * pixiv と接続したときに受け取るもの。
 *
 * `cookie` と `userAgent` は対でしか意味を持たない。片方だけでは web の一覧を
 * 読めないので、揃っていないときは両方とも無いものとして扱う。
 */
export interface PixivConnection {
  refreshToken: string;
  user: PixivUser;
  cookie: string | null;
  userAgent: string | null;
}

/** FANBOX のログイン窓が持ち帰るもの。順に セッション / 利用者 / そのときの UA。 */
export type FanboxConnection = [session: string, user: FanboxUser, userAgent: string];

export function verifyPixivToken(refreshToken: string): Promise<PixivUser> {
  return invoke<PixivUser>("verify_pixiv_token", { refreshToken });
}

export function verifyFanboxSession(sessionId: string, userAgent: string): Promise<FanboxUser> {
  return invoke<FanboxUser>("verify_fanbox_session", { sessionId, userAgent });
}

export function loginPixivWebview(): Promise<PixivConnection> {
  return invoke<PixivConnection>("login_pixiv_webview");
}

export function loginFanboxWebview(): Promise<FanboxConnection> {
  return invoke<FanboxConnection>("login_fanbox_webview");
}
