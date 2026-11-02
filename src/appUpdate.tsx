import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";

export interface AppUpdateInfo {
  current: string;
  latest: string;
  available: boolean;
  size: number;
}

const checkKey = "piep.android.appUpdateCheck";
const checkIntervalMs = 24 * 60 * 60 * 1000;

function remembered(): { at: number; info: AppUpdateInfo } | null {
  try {
    const parsed = JSON.parse(window.localStorage.getItem(checkKey) ?? "null") as { at?: number; info?: AppUpdateInfo } | null;
    return parsed?.at && parsed.info ? { at: parsed.at, info: parsed.info } : null;
  } catch { return null; }
}

function remember(info: AppUpdateInfo): void {
  try { window.localStorage.setItem(checkKey, JSON.stringify({ at: Date.now(), info })); } catch { /* storage unavailable */ }
}

/** 起動時に 1 日 1 回確かめる。圏外でも何も言わない。 */
export function useAppUpdate(): [AppUpdateInfo | null, (info: AppUpdateInfo) => void] {
  const [info, setInfo] = useState<AppUpdateInfo | null>(null);
  useEffect(() => {
    if (!isTauri()) return;
    let alive = true;
    const saved = remembered();
    void (async () => {
      const installed = await invoke<string>("app_update_installed_version").catch(() => null);
      // 当てたあとは、前の版で覚えた結果を使わない。
      if (saved && saved.info.current === installed && Date.now() - saved.at < checkIntervalMs) {
        if (alive) setInfo(saved.info);
        return;
      }
      const checked = await invoke<AppUpdateInfo>("app_update_check").catch(() => null);
      if (!checked || !alive) return;
      remember(checked);
      setInfo(checked);
    })();
    return () => { alive = false; };
  }, []);
  return [info, setInfo];
}

function formatSize(bytes: number): string {
  return `${Math.max(1, Math.round(bytes / (1024 * 1024)))} MB`;
}

type Stage =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "downloading"; percent: number }
  | { kind: "needsPermission" }
  | { kind: "installing" }
  | { kind: "problem"; message: string };

export function AppUpdateSettings({ info, onInfo }: { info: AppUpdateInfo | null; onInfo: (info: AppUpdateInfo) => void }) {
  const [stage, setStage] = useState<Stage>({ kind: "idle" });
  const [checked, setChecked] = useState(false);
  const busy = stage.kind === "checking" || stage.kind === "downloading";

  const check = useCallback(async () => {
    setStage({ kind: "checking" });
    try {
      const latest = await invoke<AppUpdateInfo>("app_update_check");
      remember(latest);
      onInfo(latest);
      setChecked(true);
      setStage({ kind: "idle" });
    } catch (error) {
      setStage({ kind: "problem", message: String(error) });
    }
  }, [onInfo]);

  const install = useCallback(async () => {
    setStage({ kind: "downloading", percent: 0 });
    const unlisten = await listen<{ done: number; total: number }>("app-update-progress", ({ payload }) => {
      setStage({ kind: "downloading", percent: Math.floor((payload.done / Math.max(1, payload.total)) * 100) });
    });
    try {
      await invoke<void>("app_update_download");
      const result = window.piepAndroidUpdate?.installDownloadedUpdate() ?? "missing";
      if (result === "started") setStage({ kind: "installing" });
      else if (result === "needs_permission") setStage({ kind: "needsPermission" });
      else setStage({ kind: "problem", message: "新しい版を開けません。もう一度試してください" });
    } catch (error) {
      setStage({ kind: "problem", message: String(error) });
    } finally {
      unlisten();
    }
  }, []);

  if (!isTauri()) return null;
  const available = info?.available === true;
  const summary = available
    ? `新しい版 ${info.latest} があります（${formatSize(info.size)}）。今の版は ${info.current} です。`
    : info
      ? `今の版は ${info.current} です。${checked ? "最新の版です。" : ""}`
      : "新しい版があるか確かめます。";
  return <section className="settings-card app-update-settings">
    <div className="setting-title"><div><h2>アプリの更新</h2><p>{summary}</p></div></div>
    {stage.kind === "downloading" && <p className="settings-help" role="status">取ってきています… {stage.percent}%</p>}
    {stage.kind === "needsPermission" && <p className="settings-help" role="status">開いた画面で piep からのインストールを許可してから、もう一度押してください。</p>}
    {stage.kind === "installing" && <p className="settings-help" role="status">画面の案内に従ってください。保存した作品と登録はそのまま残ります。</p>}
    {stage.kind === "problem" && <p className="inline-alert" role="alert">{stage.message}</p>}
    <div className="connection-actions">
      {available
        ? <button className="primary-button" disabled={busy} onClick={() => void install()}>{stage.kind === "downloading" ? "取ってきています…" : "更新する"}</button>
        : <button className="secondary-button" disabled={busy} onClick={() => void check()}>{stage.kind === "checking" ? "確かめています…" : "更新を確かめる"}</button>}
    </div>
  </section>;
}

/** 一覧の上の控えめな知らせ。読書中は出さない。 */
export function AppUpdateNotice({ info, onOpenSettings }: { info: AppUpdateInfo | null; onOpenSettings: () => void }) {
  if (!info?.available) return null;
  return <div className="app-update-notice" role="status">
    <span>新しい版 {info.latest} があります</span>
    <button type="button" className="secondary-button" onClick={onOpenSettings}>更新する</button>
  </div>;
}

declare global {
  interface Window {
    piepAndroidUpdate?: { installDownloadedUpdate: () => "started" | "needs_permission" | "missing" };
  }
}
