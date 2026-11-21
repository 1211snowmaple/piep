import { invoke, isTauri } from "@tauri-apps/api/core";
import type { MobileShelfReading, MobileShelfReadingLocation } from "@shared/mobileApi";
import { nativeShelf, type NativeOpened } from "./nativeReading";

/** 読む画面が溜めておく、次に書く位置。 */
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
let sendAgain = false;
let lastFinished = 0;

function run(): Promise<void> {
  // 圏外でも読書は続く。届かなかった位置は端末に残り、次の同期で送られる。
  const current = (async () => {
    do {
      sendAgain = false;
      // 棚の作品の位置を送り、変わった分を受け取る。
      const shelf = nativeShelf();
      if (shelf) await invoke<void>("pc_native_reading_sync", { scope: shelf }).catch(() => {});
      // 同時に届いた保存・画面終了の知らせは一回にまとめる。別々の
      // .then(run) を積むと、同じ同期の直後に何本も起動してしまう。
    } while (sendAgain && active());
    lastFinished = Date.now();
    running = null;
  })();
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
  if (running) {
    sendAgain = true;
    return running;
  }
  return run();
}

export function refreshReadingWithin(limitMs: number): Promise<void> {
  return Promise.race([refreshReading(), new Promise<void>((resolve) => { window.setTimeout(resolve, limitMs); })]);
}

/** 読んだ位置を、棚の作品の位置として端末に置く。通信はしない（次の同期で送る）。 */
export async function recordShelfReading(work: string, location: MobileShelfReadingLocation, finished: boolean): Promise<void> {
  const scope = nativeShelf();
  if (!active() || !scope) return;
  try { await invoke<void>("pc_native_reading_record", { scope, work, location, finished }); } catch { /* 読書を止めない */ }
}

/** 最後に受け取った、棚のこの作品の位置。 */
export async function readShelfPosition(work: string): Promise<MobileShelfReading | null> {
  const scope = nativeShelf();
  if (!active() || !scope) return null;
  try { return await invoke<MobileShelfReading | null>("pc_native_reading_get", { scope, work }); } catch { return null; }
}

/** 棚と位置を受け渡す本か（読む画面で棚から開いた今の版の本）。 */
export function shelfReadingFor(document: NativeOpened | null): NativeOpened | null {
  return document && active() && nativeShelf() ? document : null;
}

/** 読む画面のページ（0 から）とその中の割合を、開いた本の版に結んだ棚の位置にする。 */
export function shelfLocation(document: NativeOpened, page: number, ratio: number): MobileShelfReadingLocation {
  return { viewKind: document.view.kind, viewId: document.view.id, page, ratioPpm: Math.round(Math.min(1, Math.max(0, ratio)) * 1_000_000) };
}

/**
 * 棚の位置を、手元の記録より新しいときだけ開始位置にする。この端末が書いた位置は、手元に
 * 細かい位置が残っているのでそちらを使う（入れ直して手元に何も無いときだけ棚の位置）。
 * 別の版や編集で読んでいた位置はページだけの目安にする。
 */
export function shelfStart(remote: MobileShelfReading | null, localTime: number, document: NativeOpened): RemoteStart | null {
  if (!remote?.location || remote.updatedAt === null) return null;
  if (remote.mine ? localTime > 0 : remote.updatedAt <= localTime) return null;
  const sameText = remote.location.viewKind === document.view.kind && remote.location.viewId === document.view.id;
  return { page: remote.location.page, ratio: sameText ? remote.location.ratioPpm / 1_000_000 : 0, guide: !sameText, deviceTime: remote.updatedAt };
}
