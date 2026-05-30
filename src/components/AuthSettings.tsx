import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { store } from "../store";
import { AlertIcon, PaletteIcon, HeartIcon } from "./icons/Icons";
import { ask } from "@tauri-apps/plugin-dialog";

interface UserData {
  name: string;
  id?: string;
  userId?: string;
  iconUrl?: string;
  profile_image_urls?: {
    medium?: string;
  };
}

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export function AuthSettings() {
  const [pixivStatus, setPixivStatus] = useState<"idle" | "loading" | "success" | "error">("idle");
  const [fanboxStatus, setFanboxStatus] = useState<"idle" | "loading" | "success" | "error">("idle");

  const [pixivUser, setPixivUser] = useState<UserData | null>(null);
  const [fanboxUser, setFanboxUser] = useState<UserData | null>(null);
  const [errorMsg, setErrorMsg] = useState("");

  useEffect(() => {
    const loadSettings = async () => {
      if (!isTauriRuntime()) return;

      try {
        const savedPixiv = await store.get<string>("pixiv_refresh_token");
        if (savedPixiv) {
          try {
            const user: UserData = await invoke("verify_pixiv_token", { refreshToken: savedPixiv });
            setPixivUser(user);
            setPixivStatus("success");
          } catch (e) {
            console.error("Pixiv token verification failed:", e);
            setPixivStatus("error");
            setErrorMsg("Pixivの認証セッションが切れました。再度連携を行ってください。");
          }
        }

        const savedFanbox = await store.get<string>("fanbox_session_id");
        const savedUA = await store.get<string>("fanbox_user_agent");
        if (savedFanbox && savedUA) {
          try {
            const user: UserData = await invoke("verify_fanbox_session", {
              sessionId: savedFanbox,
              userAgent: savedUA
            });
            setFanboxUser(user);
            setFanboxStatus("success");
          } catch (e) {
            console.error("FANBOX session verification failed:", e);
            setFanboxStatus("error");
            setErrorMsg("FANBOXの認証セッションが切れました。再度連携を行ってください。");
          }
        }
      } catch (e) {
        console.error("Failed to load settings:", e);
      }
    };
    loadSettings();
  }, []);

  const loginPixiv = async () => {
    if (!isTauriRuntime()) {
      setErrorMsg("この操作はTauriアプリ内でのみ利用できます。");
      return;
    }

    setPixivStatus("loading");
    setErrorMsg("");
    try {
      const [token, user] = await invoke<[string, UserData]>("login_pixiv_webview");
      setPixivUser(user);
      setPixivStatus("success");
      await store.set("pixiv_refresh_token", token);
      await store.save();
    } catch (err: any) {
      setPixivStatus("error");
      setErrorMsg(err.toString());
    }
  };

  const loginFanbox = async () => {
    if (!isTauriRuntime()) {
      setErrorMsg("この操作はTauriアプリ内でのみ利用できます。");
      return;
    }

    setFanboxStatus("loading");
    setErrorMsg("");
    try {
      const [cookieStr, user, ua] = await invoke<[string, UserData, string]>("login_fanbox_webview");

      setFanboxUser(user);
      setFanboxStatus("success");
      await store.set("fanbox_session_id", cookieStr);
      await store.set("fanbox_user_agent", ua);
      await store.save();
    } catch (err: any) {
      setFanboxStatus("error");
      setErrorMsg(err.toString());
    }
  };

  const logoutPixiv = async () => {
    if (!isTauriRuntime()) return;

    const isConfirmed = await ask(
      "Pixivとの連携を解除しますか？\n保存されている認証トークンが削除されます。",
      { title: "連携解除の確認", kind: "warning", okLabel: "連携を解除", cancelLabel: "キャンセル" }
    );
    if (!isConfirmed) return;
    try {
      await store.delete("pixiv_refresh_token");
      await store.save();
      setPixivUser(null);
      setPixivStatus("idle");
    } catch (e) {
      console.error("Failed to disconnect Pixiv:", e);
    }
  };

  const logoutFanbox = async () => {
    if (!isTauriRuntime()) return;

    const isConfirmed = await ask(
      "FANBOXとの連携を解除しますか？\n保存されているセッション資格情報が削除されます。",
      { title: "連携解除の確認", kind: "warning", okLabel: "連携を解除", cancelLabel: "キャンセル" }
    );
    if (!isConfirmed) return;
    try {
      await store.delete("fanbox_session_id");
      await store.delete("fanbox_user_agent");
      await store.save();
      setFanboxUser(null);
      setFanboxStatus("idle");
    } catch (e) {
      console.error("Failed to disconnect FANBOX:", e);
    }
  };

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: '1.25rem', width: '100%' }}>
      {errorMsg && (
        <div className="error-banner">
          <AlertIcon />
          <span>{errorMsg}</span>
        </div>
      )}

      {/* Pixiv Auth */}
      <div className="card">
        <div className="section-title" style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
          <PaletteIcon />
          Pixiv 連携
          {pixivStatus === "success" && <span className="status-badge connected">連携済み</span>}
          {pixivStatus === "error" && <span className="status-badge error" style={{ backgroundColor: 'rgba(239, 68, 68, 0.15)', color: '#ef4444', border: '1px solid rgba(239, 68, 68, 0.25)' }}>セッション切れ</span>}
        </div>

        <p className="section-desc">
          アプリ内ブラウザを開いて Pixiv にログインします。<br />
          取得したトークンはローカルに安全に保存されます。
        </p>

        <button className="primary" onClick={loginPixiv} disabled={pixivStatus === "loading"} style={{ width: '100%' }}>
          {pixivStatus === "loading" ? "待機中..." : (pixivStatus === "success" ? "再連携する" : "Pixiv と連携を開始")}
        </button>

        {pixivStatus === "success" && pixivUser && (
          <div className="result" style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: '12px' }}>
              {pixivUser.profile_image_urls?.medium && (
                <img src={pixivUser.profile_image_urls.medium} alt="Avatar" className="avatar" />
              )}
              <div className="user-info">
                <span className="user-name">{pixivUser.name}</span>
                <span className="user-id">ID: {pixivUser.id}</span>
              </div>
            </div>
            <button className="btn-disconnect" onClick={logoutPixiv}>
              連携を解除
            </button>
          </div>
        )}
      </div>

      {/* FANBOX Auth */}
      <div className="card">
        <div className="section-title" style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
          <HeartIcon />
          FANBOX 連携
          {fanboxStatus === "success" && <span className="status-badge connected">連携済み</span>}
          {fanboxStatus === "error" && <span className="status-badge error" style={{ backgroundColor: 'rgba(239, 68, 68, 0.15)', color: '#ef4444', border: '1px solid rgba(239, 68, 68, 0.25)' }}>セッション切れ</span>}
        </div>

        <p className="section-desc">
          FANBOX にログインして、限定コンテンツの取得を有効にします。<br />
          セッション情報を自動的に取得します。
        </p>

        <button className="primary" onClick={loginFanbox} disabled={fanboxStatus === "loading"} style={{ width: '100%' }}>
          {fanboxStatus === "loading" ? "待機中..." : (fanboxStatus === "success" ? "再連携する" : "FANBOX と連携を開始")}
        </button>

        {fanboxStatus === "success" && fanboxUser && (
          <div className="result" style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: '12px' }}>
              {fanboxUser.iconUrl && (
                <img src={fanboxUser.iconUrl} alt="Avatar" className="avatar" />
              )}
              <div className="user-info">
                <span className="user-name">{fanboxUser.name}</span>
                <span className="user-id">ID: {fanboxUser.userId}</span>
              </div>
            </div>
            <button className="btn-disconnect" onClick={logoutFanbox}>
              連携を解除
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
