import { invoke } from "@tauri-apps/api/core";

export function verifyPixivToken<T>(refreshToken: string): Promise<T> {
  return invoke<T>("verify_pixiv_token", { refreshToken });
}

export function verifyFanboxSession<T>(sessionId: string, userAgent: string): Promise<T> {
  return invoke<T>("verify_fanbox_session", { sessionId, userAgent });
}

export function loginPixivWebview<T>(): Promise<T> {
  return invoke<T>("login_pixiv_webview");
}

export function loginFanboxWebview<T>(): Promise<T> {
  return invoke<T>("login_fanbox_webview");
}
