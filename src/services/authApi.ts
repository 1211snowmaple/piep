import { invoke } from "@tauri-apps/api/core";

/**
 * 取得用の接続の様子。
 *
 * **鍵そのものは画面へ来ない。** pixiv の更新用トークン・Cookie、FANBOX の
 * セッションはバックエンドが Windows の資格情報マネージャーに置き、保存や
 * 更新確認のときに自分で読む。画面が知るのは、繋がっているかと名前だけ。
 */
export interface ConnectionStatus {
  pixiv: { name: string; webSession: boolean } | null;
  fanbox: { name: string } | null;
  /** 保存された接続を読めなかった取得元。繋がっていないものとして扱い、繋ぎ直してもらう。 */
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

export function disconnectAccount(source: "pixiv" | "fanbox"): Promise<ConnectionStatus> {
  return invoke<ConnectionStatus>("disconnect_account", { source });
}
