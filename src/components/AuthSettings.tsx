import { useState, useEffect } from "react";
import { store } from "../store";
import { loginFanboxWebview, loginPixivWebview, verifyFanboxSession, verifyPixivToken } from "@/services/authApi";
import { scanAndReimportDownloads } from "@/services/dbApi";
import { AlertIcon, PaletteIcon, HeartIcon } from "./icons/Icons";
import { askDialog, messageDialog } from "@/services/dialogApi";
import { Database } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";

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
  const [reimportStatus, setReimportStatus] = useState<"idle" | "loading" | "success" | "error">("idle");
  const [reimportCount, setReimportCount] = useState(0);

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
            const user = await verifyPixivToken<UserData>(savedPixiv);
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
            const user = await verifyFanboxSession<UserData>(savedFanbox, savedUA);
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
      const [token, user] = await loginPixivWebview<[string, UserData]>();
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
      const [cookieStr, user, ua] = await loginFanboxWebview<[string, UserData, string]>();

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

    const isConfirmed = await askDialog(
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

    const isConfirmed = await askDialog(
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

  const handleReimportLibrary = async () => {
    if (!isTauriRuntime()) {
      setErrorMsg("この操作はTauriアプリ内でのみ利用できます。");
      return;
    }

    const isConfirmed = await askDialog(
      "ローカルの downloads フォルダ内にある作品データをスキャンし、ライブラリのデータベース情報を再構築します。\nこの操作により、現在のライブラリ情報と検索インデックスは一度クリアされ、ローカルファイル群から完全に再インポートされます。よろしいですか？",
      { title: "ライブラリの再構築・インポート", kind: "warning", okLabel: "インポートを実行", cancelLabel: "キャンセル" }
    );
    if (!isConfirmed) return;

    setReimportStatus("loading");
    setErrorMsg("");
    try {
      const count = await scanAndReimportDownloads();
      setReimportCount(count);
      setReimportStatus("success");
      await messageDialog(`ライブラリの再構築・インポートが完了しました！\n復元された作品: ${count} 件`, { title: "インポート完了", kind: "info" });
    } catch (err: any) {
      setReimportStatus("error");
      setErrorMsg(err.toString());
    }
  };

  return (
    <div className="flex w-full flex-col gap-5">
      {errorMsg && (
        <div className="error-banner">
          <AlertIcon />
          <span>{errorMsg}</span>
        </div>
      )}

      {/* Pixiv Auth */}
      <Card className="p-5">
        <div className="section-title flex items-center gap-2">
          <PaletteIcon />
          Pixiv 連携
          {pixivStatus === "success" && <Badge className="status-badge connected">連携済み</Badge>}
          {pixivStatus === "error" && <Badge variant="destructive" className="status-badge error">セッション切れ</Badge>}
        </div>

        <p className="section-desc">
          アプリ内ブラウザを開いて Pixiv にログインします。<br />
          取得したトークンはローカルに安全に保存されます。
        </p>

        <Button type="button" className="w-full" onClick={loginPixiv} disabled={pixivStatus === "loading"}>
          {pixivStatus === "loading" ? "待機中..." : (pixivStatus === "success" ? "再連携する" : "Pixiv と連携を開始")}
        </Button>

        {pixivStatus === "success" && pixivUser && (
          <div className="result flex items-center justify-between gap-3">
            <div className="flex min-w-0 items-center gap-3">
              {pixivUser.profile_image_urls?.medium && (
                <img src={pixivUser.profile_image_urls.medium} alt="Avatar" className="avatar" />
              )}
              <div className="user-info">
                <span className="user-name">{pixivUser.name}</span>
                <span className="user-id">ID: {pixivUser.id}</span>
              </div>
            </div>
            <Button type="button" variant="outline" size="sm" onClick={logoutPixiv}>
              連携を解除
            </Button>
          </div>
        )}
      </Card>

      {/* FANBOX Auth */}
      <Card className="p-5">
        <div className="section-title flex items-center gap-2">
          <HeartIcon />
          FANBOX 連携
          {fanboxStatus === "success" && <Badge className="status-badge connected">連携済み</Badge>}
          {fanboxStatus === "error" && <Badge variant="destructive" className="status-badge error">セッション切れ</Badge>}
        </div>

        <p className="section-desc">
          FANBOX にログインして、限定コンテンツの取得を有効にします。<br />
          セッション情報を自動的に取得します。
        </p>

        <Button type="button" className="w-full" onClick={loginFanbox} disabled={fanboxStatus === "loading"}>
          {fanboxStatus === "loading" ? "待機中..." : (fanboxStatus === "success" ? "再連携する" : "FANBOX と連携を開始")}
        </Button>

        {fanboxStatus === "success" && fanboxUser && (
          <div className="result flex items-center justify-between gap-3">
            <div className="flex min-w-0 items-center gap-3">
              {fanboxUser.iconUrl && (
                <img src={fanboxUser.iconUrl} alt="Avatar" className="avatar" />
              )}
              <div className="user-info">
                <span className="user-name">{fanboxUser.name}</span>
                <span className="user-id">ID: {fanboxUser.userId}</span>
              </div>
            </div>
            <Button type="button" variant="outline" size="sm" onClick={logoutFanbox}>
              連携を解除
            </Button>
          </div>
        )}
      </Card>

      {/* Library Reconstruction */}
      <Card className="mt-2 p-5">
        <div className="section-title flex items-center gap-2">
          <Database size={18} className="text-primary" />
          ローカルライブラリの再構築
          {reimportStatus === "success" && <Badge variant="secondary" className="status-badge connected">再構築完了</Badge>}
        </div>

        <p className="section-desc">
          ローカルフォルダ（downloads）を再走査し、保存されている作品データ（JSON、表紙、イラスト等）からデータベースおよび検索インデックスを完全に復元します。
        </p>

        <Button
          type="button"
          className="w-full"
          onClick={handleReimportLibrary}
          disabled={reimportStatus === "loading"}
        >
          {reimportStatus === "loading" ? "再構築中 (しばらくお待ちください)..." : "ローカルライブラリを再構築・インポート"}
        </Button>

        {reimportStatus === "success" && (
          <div className="result mt-3 border-l-4 border-primary pl-3 text-sm text-muted-foreground">
            前回の再構築により、合計 <strong>{reimportCount}</strong> 件の小説作品がライブラリに正常に復元されました。
          </div>
        )}
      </Card>
    </div>
  );
}
