import { convertFileSrc, invoke, isTauri } from "@tauri-apps/api/core";
import type { PcConnectionStatus, PcOutlineEntry, PcPage, PcWork } from "./pcApi";

type NativeScope = NonNullable<PcConnectionStatus["nativeLibrary"]>["scope"];

/** 新しい棚から開いた一冊。ページと画像は端末の中で組んである。 */
export interface NativeOpened {
  key: string;
  entityId: string;
  view: { kind: "snapshot" | "edit"; id: string };
  edited: boolean;
  pageCount: number;
  sourcePageStarts: number[];
  totalPlainTextChars: number;
  outline: PcOutlineEntry[];
}

let scope: NativeScope | null = null;
const opened = new Map<string, Promise<NativeOpened | null>>();

/**
 * 接続先が新しい棚の読書用の一式を出せるときだけ、本文をそこから読む。
 * 接続や棚が変わったら、前の棚で開いたものは使わない。
 */
export function adoptNativeLibrary(status: PcConnectionStatus | null): void {
  const next = status?.reachable && status.nativeLibrary?.readerBundle ? status.nativeLibrary.scope : null;
  const same = next && scope
    && next.libraryId === scope.libraryId
    && next.subjectId === scope.subjectId
    && next.syncEpoch === scope.syncEpoch
    && next.connectionGeneration === scope.connectionGeneration;
  if (same) return;
  scope = next;
  opened.clear();
}

/**
 * 作品を新しい棚で開く。棚に無い作品や、新しい棚を読めない接続では null（今までどおり読む）。
 * `fresh` は開き直すとき（端末が覚えておける冊数を超えて、組んだものが消えたとき）。
 */
export function openNative(work: Pick<PcWork, "source" | "sourceId">, fresh = false): Promise<NativeOpened | null> {
  const current = scope;
  if (!current || !isTauri()) return Promise.resolve(null);
  const id = `${work.source}:${work.sourceId}`;
  const found = opened.get(id);
  if (found && !fresh) return found;
  const next = invoke<NativeOpened>("pc_native_open", { scope: current, provider: work.source, key: work.sourceId })
    .catch(() => null);
  opened.set(id, next);
  // 開けなかったことは覚えない。次に開いたときに確かめ直す。
  void next.then((result) => { if (!result && opened.get(id) === next) opened.delete(id); });
  return next;
}

export const nativePage = (document: NativeOpened, page: number) => invoke<PcPage>("pc_native_page", { key: document.key, page });

/** 本文の挿絵。部分実体の受け取りとハッシュの確認は Rust 側で済ませる。 */
export function nativeImageUrl(document: NativeOpened, assetId: number): string | null {
  if (!isTauri()) return null;
  return convertFileSrc(`native/${document.key}/${assetId}`, "pcimg");
}
