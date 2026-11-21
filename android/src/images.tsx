import { useEffect, useRef, useState, type ReactEventHandler } from "react";
import { convertFileSrc, isTauri } from "@tauri-apps/api/core";
import type { PcCollection, PcWork } from "./pcApi";
import { shelfCoverPath, shelfEntityCoverPath } from "./nativeReading";

// WebView の HTTP キャッシュも接続ごとに分ける。Rust 側のディスクキャッシュ
// だけを分けても、同じ URL が先に WebView から返されてしまう。
const newRemoteScope = () => [...crypto.getRandomValues(new Uint32Array(4))].map((part) => part.toString(16).padStart(8, "0")).join("");
let remoteScope = newRemoteScope();
function imageUrl(path: string, remote = true): string | null {
  if (!isTauri()) return null;
  const url = convertFileSrc(path, "pcimg");
  return remote ? `${url}?connection=${remoteScope}` : url;
}

/** 起動し直したときも、接続を変えたときも、前の接続の URL を使わない。 */
export function forgetServerImages(): void {
  remoteScope = newRemoteScope();
  loaded.clear();
  failed.clear();
}

/** 作品の表紙。認証は Rust 側で済ませ、JavaScript には鍵も画像の中身も渡らない。 */
export function workCoverUrl(work: PcWork, large = false): string | null {
  const path = shelfCoverPath(work, large);
  return path ? imageUrl(path) : null;
}
/** 作者・シリーズの画像。 */
export function entityCoverUrl(kind: "people" | "series", source: string, key: string, banner = false): string | null {
  const path = shelfEntityCoverPath(kind, source, key, banner);
  return path ? imageUrl(path) : null;
}
/** コレクションの表紙（`single` は表紙に選んだ作品の表紙）。 */
export const collectionCoverUrl = (collection: Pick<PcCollection, "id" | "shelfLibrary">, slot: string, revision: number) => collection.shelfLibrary
  ? imageUrl(`shelf-collection/${collection.shelfLibrary}/${collection.id}/${slot}/${revision}`)
  : null;
export const savedCoverUrl = (workId: string, generation: number) => imageUrl(`saved/${workId}/${generation}`, false);
export const savedEntityCoverUrl = (workId: string, kind: "people" | "series", generation: number) => imageUrl(`saved-entity/${workId}/${kind}/${generation}`, false);
export const savedCollectionCoverUrl = (workId: string, collectionId: string, generation: number) => imageUrl(`saved-collection/${workId}/${collectionId}/${generation}`, false);

/**
 * プロフィール画像を持つ取得元か。pixiv はアイコンの無い人にも「画像なし」の絵を返すが、
 * FANBOX は URL を返さないので、保存するものが無い。持たない取得元には「画像なし」の絵を出さない。
 */
export const hasProfileImages = (source: string) => source === "pixiv" || source === "fanbox";

/** アイコンを置いていない作者の絵。デスクトップ版（`NoImageMark`）と同じ絵で、保存元での見え方に揃える。 */
export function NoProfileImage() {
  return <img className="no-profile-image" src="/no-profile.png" alt="" decoding="async" />;
}

// 一度出た画像は、スクロールで戻ったときに溶け込ませず、その場で出す。
const loaded = new Set<string>();
// 画像のない作者は多い。行き来のたびに問い合わせ直さない。通信の失敗もあるので、しばらくしたら試し直す。
const failed = new Map<string, number>();
const retryFailedAfterMs = 60_000;
// 通信が一瞬切れた、サーバーが混んでいた、という失敗で表紙を諦めない。
// 間を空けて取り直し、それでも出なければ代わりの絵にする。
const retryDelaysMs = [1_200, 4_000];

/** 取り直しの印。経路は Rust 側で見るので、問い合わせの部分は読み捨てられる。 */
function attemptSrc(src: string, attempt: number): string {
  return attempt === 0 ? src : `${src}${src.includes("?") ? "&" : "?"}attempt=${attempt}`;
}

export function imageRecentlyFailed(src: string | null): boolean {
  if (!src) return false;
  const at = failed.get(src);
  if (at === undefined) return false;
  if (Date.now() - at < retryFailedAfterMs) return true;
  failed.delete(src);
  return false;
}

/**
 * 読み込めたら静かに現れる画像。下に置いた代わりの絵から切り替わるときの
 * ちらつきを消すのが目的で、読み込み済みのものには動きを付けない。
 */
export function SmoothImage({ src, alt, className, onFailed, onLoaded }: {
  src: string;
  alt: string;
  className?: string;
  onFailed?: () => void;
  onLoaded?: ReactEventHandler<HTMLImageElement>;
}) {
  const known = loaded.has(src);
  const [shown, setShown] = useState(known);
  const [attempt, setAttempt] = useState(0);
  const node = useRef<HTMLImageElement>(null);
  useEffect(() => {
    setShown(loaded.has(src));
    setAttempt(0);
    if (node.current?.complete && node.current.naturalWidth > 0) { loaded.add(src); setShown(true); }
  }, [src]);
  const retry = useRef<number | null>(null);
  useEffect(() => () => { if (retry.current !== null) window.clearTimeout(retry.current); }, []);
  return <img
    ref={node}
    className={className ? `smooth-image ${className}` : "smooth-image"}
    src={attemptSrc(src, attempt)}
    alt={alt}
    draggable={false}
    decoding={known ? "sync" : "async"}
    data-shown={shown || undefined}
    data-instant={known || undefined}
    onLoad={(event) => { loaded.add(src); failed.delete(src); setShown(true); onLoaded?.(event); }}
    onError={() => {
      loaded.delete(src);
      if (attempt < retryDelaysMs.length) {
        const next = attempt + 1;
        if (retry.current !== null) window.clearTimeout(retry.current);
        retry.current = window.setTimeout(() => { retry.current = null; setAttempt(next); }, retryDelaysMs[attempt]);
        return;
      }
      failed.set(src, Date.now());
      if (failed.size > 2000) failed.clear();
      onFailed?.();
    }}
  />;
}
