import { invoke, isTauri } from "@tauri-apps/api/core";

export interface ReadingPosition {
  source: string;
  sourceId: string;
  version: number | null;
  editHash: string | null;
  page: number;
  position: number;
  finished: boolean;
  deviceId: string;
  deviceTime: number;
  cursor: number;
}

export interface ReadingWrite {
  source: string;
  sourceId: string;
  version: number | null;
  editHash: string | null;
  page: number;
  position: number;
  finished: boolean;
  deviceTime: number;
}

export interface RemoteStart {
  page: number;
  ratio: number;
  // 版が違う位置はページ数だけの目安。超えていたら最初から読む。
  guide: boolean;
  deviceTime: number;
}

const settingKey = "piep.android.readingSync";

export function readingSyncEnabled(): boolean {
  try { return window.localStorage.getItem(settingKey) !== "off"; } catch { return true; }
}

export function setReadingSyncEnabled(enabled: boolean): void {
  try { window.localStorage.setItem(settingKey, enabled ? "on" : "off"); } catch { /* storage unavailable */ }
}

const active = () => isTauri() && readingSyncEnabled();

let running: Promise<void> | null = null;
let lastFinished = 0;

function run(): Promise<void> {
  // 圏外でも読書は続く。届かなかった位置は端末に残り、次の同期で送られる。
  const current = invoke<void>("reading_sync").catch(() => {}).then(() => {
    lastFinished = Date.now();
    running = null;
  });
  running = current;
  return current;
}

/** 位置を受け取る。少し前に同期したばかりなら、やり直さない。 */
export function refreshReading(maxAgeMs = 5000): Promise<void> {
  if (!active()) return Promise.resolve();
  if (running) return running;
  if (Date.now() - lastFinished < maxAgeMs) return Promise.resolve();
  return run();
}

/** 溜まった位置を今すぐ送る。走っている同期には間に合わないので、その後にもう一度走らせる。 */
export function sendReading(): Promise<void> {
  if (!active()) return Promise.resolve();
  return running ? running.then(run) : run();
}

export function refreshReadingWithin(limitMs: number): Promise<void> {
  return Promise.race([refreshReading(), new Promise<void>((resolve) => { window.setTimeout(resolve, limitMs); })]);
}

export async function recordReading(write: ReadingWrite): Promise<void> {
  if (!active()) return;
  try { await invoke<void>("reading_record", { write }); } catch { /* 読書を止めない */ }
}

export async function readRemotePosition(source: string, sourceId: string): Promise<ReadingPosition | null> {
  if (!active()) return null;
  try { return await invoke<ReadingPosition | null>("reading_get", { source, sourceId }); } catch { return null; }
}

/** サーバーの位置を、手元より新しいときだけ開始位置にする。 */
export function remoteStart(remote: ReadingPosition | null, localTime: number, version: number, editHash: string | null): RemoteStart | null {
  if (!remote || remote.deviceTime <= localTime) return null;
  const sameText = remote.version === version && (remote.editHash ?? null) === editHash;
  return { page: remote.page, ratio: sameText ? remote.position : 0, guide: !sameText, deviceTime: remote.deviceTime };
}
