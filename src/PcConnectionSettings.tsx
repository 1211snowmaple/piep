import { useEffect, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { disconnectPc, getPcStatus, pairPc, pcConnectionIssue, pcErrorMessage, type PcConnectionStatus } from "./pcApi";
import { readingSyncEnabled, sendReading, setReadingSyncEnabled } from "./readingSync";

export function PcConnectionSettings({ status, onStatusChange }: { status: PcConnectionStatus | null; onStatusChange: (status: PcConnectionStatus) => void }) {
  const [pairingText, setPairingText] = useState("");
  const [deviceName, setDeviceName] = useState("piep Android");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [syncReading, setSyncReading] = useState(readingSyncEnabled);

  const refresh = async () => {
    setBusy(true);
    setProblem(null);
    try { onStatusChange(await getPcStatus()); }
    catch (error) { setProblem(pcErrorMessage(error)); }
    finally { setBusy(false); }
  };

  useEffect(() => {
    if (!isTauri()) return;
    let active = true;
    void getPcStatus().then((next) => { if (active) onStatusChange(next); })
      .catch((error) => { if (active) setProblem(pcErrorMessage(error)); });
    return () => { active = false; };
  }, [onStatusChange]);

  const register = async () => {
    setBusy(true);
    setProblem(null);
    try {
      onStatusChange(await pairPc(pairingText.trim(), deviceName.trim()));
      setPairingText("");
    } catch (error) { setProblem(pcErrorMessage(error)); }
    finally { setBusy(false); }
  };

  const forget = async () => {
    setBusy(true);
    setProblem(null);
    try { onStatusChange(await disconnectPc()); }
    catch (error) { setProblem(pcErrorMessage(error)); }
    finally { setBusy(false); }
  };

  const registrationForm = <div className="connection-registration">
    <p>デスクトップ版 piep の「設定 → サーバー（β）」で「携帯を追加」を押し、表示された登録文字列を入力します。</p>
    <label>端末の名前<input value={deviceName} onChange={(event) => setDeviceName(event.target.value)} maxLength={64} /></label>
    <label>登録文字列<textarea value={pairingText} onChange={(event) => setPairingText(event.target.value)} rows={4} placeholder='{"version":2,"url":"https://..."}' /></label>
    <button className="primary-button" disabled={!pairingText.trim() || !deviceName.trim() || busy || !isTauri()} onClick={() => void register()}>{busy ? "登録中…" : "サーバーと接続"}</button>
  </div>;

  return <section className="settings-card connection-settings">
    <div className="connection-setting"><div><h2>サーバーとの接続</h2><p>piep サーバーの本棚を読むための接続を管理します。</p></div><span className={status?.reachable ? "connection-badge connected" : "connection-badge"}><span className="status-dot" />{status?.reachable ? "接続中" : "未接続"}</span></div>
    {status?.issue && <p className="connection-issue">{pcConnectionIssue(status)}</p>}
    {problem && <p className="inline-alert" role="alert">{problem}</p>}
    {status?.paired && status.capabilities.includes("reading-sync") && <><label className="connection-sync"><input type="checkbox" checked={syncReading} onChange={(event) => { const enabled = event.target.checked; setReadingSyncEnabled(enabled); setSyncReading(enabled); if (enabled) void sendReading(); }} />読書位置を同期する</label><p className="settings-help">同じ作品を別の端末で開いたときに、続きから読めます。圏外で読んだ分は、次につながったときに届きます。</p></>}
    {status?.paired && <div className="connection-actions"><button className="secondary-button" disabled={busy || !isTauri()} onClick={() => void refresh()}>{busy ? "確認中…" : "接続を確認"}</button></div>}
    {status?.paired ? <details className="connection-repair"><summary>登録を変更・解除</summary>{registrationForm}<button className="secondary-button connection-forget" disabled={busy} onClick={() => void forget()}>この端末の登録を消す</button></details> : registrationForm}
    {!isTauri() && <p className="settings-help">登録は Android アプリで利用できます。</p>}
  </section>;
}
