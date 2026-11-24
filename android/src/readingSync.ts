import { invoke, isTauri } from "@tauri-apps/api/core";
import type { MobileReading, MobileReadingLocation } from "@shared/mobileApi";
import { connectedShelf, type OpenedShelfWork } from "./shelf";

/** 読む画面が溜めておく、次に書く位置。 */
export interface ReadingWrite {
  page: number;
  position: number;
  finished: boolean;
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
type ShelfScope = NonNullable<ReturnType<typeof connectedShelf>>;

let running: Promise<void> | null = null;
let runningScope: ShelfScope | null = null;
let sendAgain = false;
let lastFinished: { scope: ShelfScope; time: number } | null = null;

function run(shelf: ShelfScope): Promise<void> {
  // 圏外でも読書は続く。届かなかった位置は端末に残り、次の同期で送られる。
  const current = (async () => {
    let scope: ShelfScope | null = shelf;
    do {
      sendAgain = false;
      runningScope = scope;
      await invoke<void>("pc_reading_sync", { scope }).catch(() => {});
      lastFinished = { scope, time: Date.now() };
      // 同時に届いた保存・画面終了の知らせは一回にまとめる。別々の
      // .then(run) を積むと、同じ同期の直後に何本も起動してしまう。
      scope = sendAgain && active() ? connectedShelf() : null;
    } while (scope);
    runningScope = null;
    running = null;
  })();
  running = current;
  return current;
}

/** 位置を受け取る。少し前に同期したばかりなら、やり直さない。 */
export function refreshReading(maxAgeMs = 5000): Promise<void> {
  const scope = connectedShelf();
  if (!active() || !scope) return Promise.resolve();
  if (running) {
    if (runningScope !== scope) sendAgain = true;
    return running;
  }
  if (lastFinished?.scope === scope && Date.now() - lastFinished.time < maxAgeMs) return Promise.resolve();
  return run(scope);
}

/** 溜まった位置を今すぐ送る。走っている同期には間に合わないので、その後にもう一度走らせる。 */
export function sendReading(): Promise<void> {
  const scope = connectedShelf();
  if (!active() || !scope) return Promise.resolve();
  if (running) {
    sendAgain = true;
    return running;
  }
  return run(scope);
}

export function refreshReadingWithin(limitMs: number): Promise<void> {
  return Promise.race([refreshReading(), new Promise<void>((resolve) => { window.setTimeout(resolve, limitMs); })]);
}

/** 読んだ位置を、棚の作品の位置として端末に置く。通信はしない（次の同期で送る）。 */
export async function recordShelfReading(work: string, location: MobileReadingLocation, finished: boolean): Promise<void> {
  const scope = connectedShelf();
  if (!active() || !scope) return;
  try { await invoke<void>("pc_reading_record", { scope, work, location, finished }); } catch { /* 読書を止めない */ }
}

/** 最後に受け取った、棚のこの作品の位置。 */
export async function readShelfPosition(work: string): Promise<MobileReading | null> {
  const scope = connectedShelf();
  if (!active() || !scope) return null;
  try {
    const position = await invoke<MobileReading | null>("pc_reading_get", { scope, work });
    return connectedShelf() === scope ? position : null;
  } catch { return null; }
}

/** 棚と位置を受け渡す本か（読む画面で棚から開いた今の版の本）。 */
export function shelfReadingFor(document: OpenedShelfWork | null): OpenedShelfWork | null {
  return document && active() && connectedShelf() ? document : null;
}

/** 読む画面のページ（0 から）とその中の割合を、開いた本の版に結んだ棚の位置にする。 */
export function shelfLocation(document: OpenedShelfWork, page: number, ratio: number): MobileReadingLocation {
  return { viewKind: document.view.kind, viewId: document.view.id, page, ratioPpm: Math.round(Math.min(1, Math.max(0, ratio)) * 1_000_000) };
}

/**
 * 棚の位置を、手元の記録より新しいときだけ開始位置にする。この端末が書いた位置は、手元に
 * 細かい位置が残っているのでそちらを使う（入れ直して手元に何も無いときだけ棚の位置）。
 * 別の版や編集で読んでいた位置はページだけの目安にする。
 */
export function shelfStart(remote: MobileReading | null, localTime: number, document: OpenedShelfWork): RemoteStart | null {
  if (!remote?.location || remote.updatedAt === null) return null;
  if (remote.mine ? localTime > 0 : remote.updatedAt <= localTime) return null;
  const sameText = remote.location.viewKind === document.view.kind && remote.location.viewId === document.view.id;
  return { page: remote.location.page, ratio: sameText ? remote.location.ratioPpm / 1_000_000 : 0, guide: !sameText, deviceTime: remote.updatedAt };
}
