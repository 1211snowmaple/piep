import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { isTauri } from "@tauri-apps/api/core";
import { disconnectPc, getPcStatus, pairPc, pcConnectionIssue, pcErrorMessage, type PcConnectionStatus } from "./pcApi";
import { readingSyncEnabled, sendReading, setReadingSyncEnabled } from "./readingSync";
import { isPairingPayload } from "./pairingQr";

const scannerControls: { stop: (() => boolean) | null } = { stop: null };

declare global {
  interface Window {
    /** 読み取り中の戻るキー。カメラを止めて、アプリは閉じない。 */
    piepAndroidScannerBack?: () => boolean;
  }
}

export function PcConnectionSettings({ status, onStatusChange }: { status: PcConnectionStatus | null; onStatusChange: (status: PcConnectionStatus) => void }) {
  const [pairingText, setPairingText] = useState("");
  const [deviceName, setDeviceName] = useState("piep Android");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [syncReading, setSyncReading] = useState(readingSyncEnabled);
  const [scanning, setScanning] = useState(false);
  const scanStopped = useRef(false);

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

  const register = async (payload = pairingText) => {
    setBusy(true);
    setProblem(null);
    try {
      onStatusChange(await pairPc(payload.trim(), deviceName.trim()));
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

  // カメラの許可は、読むボタンを押したときだけ求める。
  const scanPairing = async () => {
    setProblem(null);
    const scanner = await import("@tauri-apps/plugin-barcode-scanner");
    const stopScan = () => {
      scanStopped.current = true;
      void scanner.cancel();
      return true;
    };
    try {
      let permission = await scanner.checkPermissions();
      if (permission !== "granted") permission = await scanner.requestPermissions();
      if (permission !== "granted") {
        setProblem("カメラを使えません。登録文字列を貼り付けてください");
        return;
      }
      scanStopped.current = false;
      scannerControls.stop = stopScan;
      window.piepAndroidScannerBack = stopScan;
      // カメラは WebView の裏に映る。画面を透かして、案内とボタンだけを上に置く。
      document.documentElement.classList.add("qr-scanning");
      setScanning(true);
      const scanned = await scanner.scan({ windowed: true, formats: [scanner.Format.QRCode] });
      const content = scanned.content?.trim() ?? "";
      if (!content) return;
      // piep の登録用 QR の形でなければ、入力欄にも入れず、接続もしない。
      if (!isPairingPayload(content)) {
        setProblem("piep の接続用 QR ではありません。デスクトップ版の「携帯を追加」に出る QR を読んでください");
        return;
      }
      setPairingText(content);
      // 正しく読めたら、ボタンを押さずにそのまま接続する。中身は Rust 側が通信の前にもう一度確かめる。
      if (deviceName.trim()) void register(content);
      else setProblem("端末の名前を入れてから「サーバーと接続」を押してください");
    } catch {
      if (!scanStopped.current) setProblem("QR を読めませんでした。登録文字列を貼り付けてください");
    } finally {
      delete window.piepAndroidScannerBack;
      scannerControls.stop = null;
      document.documentElement.classList.remove("qr-scanning");
      setScanning(false);
    }
  };

  const registrationForm = <div className="connection-registration">
    <p>デスクトップ版 piep の「設定 → サーバー」の「携帯を追加」で「次へ」を押し、表示された QR を読むと、そのまま接続します。登録文字列を貼り付けたときは「サーバーと接続」を押します。</p>
    <label>端末の名前<input value={deviceName} onChange={(event) => setDeviceName(event.target.value)} maxLength={64} /></label>
    {isTauri() && <button type="button" className="secondary-button" disabled={busy} onClick={() => void scanPairing()}>QR を読む</button>}
    <label>登録文字列<textarea value={pairingText} onChange={(event) => setPairingText(event.target.value)} rows={4} placeholder='{"version":2,"url":"https://..."}' /></label>
    <button className="primary-button" disabled={!pairingText.trim() || !deviceName.trim() || busy || !isTauri()} onClick={() => void register()}>{busy ? "登録中…" : "サーバーと接続"}</button>
  </div>;

  return <section className="settings-card connection-settings">
    {scanning && createPortal(<div className="qr-scanner" role="dialog" aria-label="QR を読む">
      <p>デスクトップ版に出ている QR を写してください</p>
      <div className="qr-scanner-frame" aria-hidden="true" />
      <button type="button" className="primary-button" onClick={() => scannerControls.stop?.()}>やめる</button>
    </div>, document.body)}
    <div className="connection-setting"><div><h2>サーバーとの接続</h2><p>piep サーバーの本棚を読むための接続を管理します。</p></div><span className={status?.reachable ? "connection-badge connected" : "connection-badge"}><span className="status-dot" />{status?.reachable ? "接続中" : "未接続"}</span></div>
    {status?.issue && <p className="connection-issue">{pcConnectionIssue(status)}</p>}
    {problem && <p className="inline-alert" role="alert">{problem}</p>}
    {status?.paired && status.capabilities.includes("reading-sync") && <><label className="connection-sync"><input type="checkbox" checked={syncReading} onChange={(event) => { const enabled = event.target.checked; setReadingSyncEnabled(enabled); setSyncReading(enabled); if (enabled) void sendReading(); }} />読書位置を同期する</label><p className="settings-help">同じ作品を別の端末で開いたときに、続きから読めます。圏外で読んだ分は、次につながったときに届きます。</p></>}
    {status?.paired && <div className="connection-actions"><button className="secondary-button" disabled={busy || !isTauri()} onClick={() => void refresh()}>{busy ? "確認中…" : "接続を確認"}</button></div>}
    {status?.paired ? <details className="connection-repair"><summary>登録を変更・解除</summary>{registrationForm}<button className="secondary-button connection-forget" disabled={busy} onClick={() => void forget()}>この端末の登録を消す</button></details> : registrationForm}
    {!isTauri() && <p className="settings-help">登録は Android アプリで利用できます。</p>}
  </section>;
}
