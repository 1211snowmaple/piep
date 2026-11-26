import { invoke } from "@tauri-apps/api/core";

/**
 * 取得用の接続の様子。
 *
 * **鍵そのものは画面へ来ない。** pixiv の更新用トークン・Cookie、FANBOX の
 * セッションはバックエンドが Windows の資格情報マネージャーに置き、保存や
 * 更新確認のときに自分で読む。画面には接続状態・名前・アカウントの識別子を返す。
 */
/** 登録したアカウントひとつ。`active` は保存・更新確認に使っているもの。 */
export interface ConnectedAccount {
  key: string;
  name: string;
  active: boolean;
}

export interface ConnectionStatus {
  /** 使っているアカウントの様子。 */
  pixiv: { name: string; webSession: boolean } | null;
  fanbox: { name: string } | null;
  /** 登録したアカウント全部。取得元ごとに複数持てて、使うのはそのうち一つ。 */
  accounts?: { pixiv: ConnectedAccount[]; fanbox: ConnectedAccount[] };
  /** 保存された接続を読めなかった取得元。一覧を削除してから登録し直せる。 */
  unreadable?: Array<"pixiv" | "fanbox">;
}

export function getConnectionStatus(): Promise<ConnectionStatus> {
  return invoke<ConnectionStatus>("connection_status");
}

export function loginPixivWebview(): Promise<ConnectionStatus> {
  return invoke<ConnectionStatus>("login_pixiv_webview");
}

/** 手で貼った更新用トークンを確かめ、通れば保存する。 */
export function connectPixivToken(refreshToken: string): Promise<ConnectionStatus> {
  return invoke<ConnectionStatus>("connect_pixiv_token", { refreshToken });
}

export function loginFanboxWebview(): Promise<ConnectionStatus> {
  return invoke<ConnectionStatus>("login_fanbox_webview");
}

/** 手で貼ったセッション ID と UA の組を確かめ、通れば保存する。 */
export function connectFanboxSession(sessionId: string, userAgent: string): Promise<ConnectionStatus> {
  return invoke<ConnectionStatus>("connect_fanbox_session", { sessionId, userAgent });
}

/** 登録したアカウントひとつの接続を解く。使っていたものなら、残りの最初を使う。 */
export function disconnectAccount(source: "pixiv" | "fanbox", key: string): Promise<ConnectionStatus> {
  return invoke<ConnectionStatus>("disconnect_account", { source, key });
}

export function clearUnreadableAccounts(source: "pixiv" | "fanbox"): Promise<ConnectionStatus> {
  return invoke<ConnectionStatus>("clear_unreadable_accounts", { source });
}

/** 保存・更新確認に使うアカウントを切り替える。走っている仕事は始めたときのまま進む。 */
export function activateAccount(source: "pixiv" | "fanbox", key: string): Promise<ConnectionStatus> {
  return invoke<ConnectionStatus>("activate_account", { source, key });
}
