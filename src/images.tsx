import { useEffect, useRef, useState, type ReactEventHandler } from "react";
import { convertFileSrc, isTauri } from "@tauri-apps/api/core";
import type { PcWork } from "./pcApi";

function imageUrl(path: string): string | null {
  return isTauri() ? convertFileSrc(path, "pcimg") : null;
}

export const workCoverUrl = (work: PcWork, large = false) => imageUrl(`work/${work.id}/${work.currentVersion}/${work.contentHash ?? ""}${large ? "/large" : ""}`);
/** 本文の挿絵。認証は Rust 側で済ませ、JavaScript には鍵も画像の中身も渡らない。 */
export const illustrationUrl = (workId: number, assetId: number, width: number, version: number, contentHash: string | null) => imageUrl(`asset/${workId}/${assetId}/${width}/${version}/${contentHash ?? ""}`);
export const workCoverUrlById = (workId: number, revision: number) => imageUrl(`work/${workId}/c${revision}/`);
export const entityCoverUrl = (kind: "people" | "series", source: string, key: string, banner = false) => imageUrl(`entity/${kind}/${source}/${encodeURIComponent(key)}${banner ? "/banner" : ""}`);
export const collectionCoverUrl = (collectionId: string, slot: string, revision: number) => imageUrl(`collection/${collectionId}/${slot}/${revision}`);
export const savedCoverUrl = (workId: number, generation: number) => imageUrl(`saved/${workId}/${generation}`);
export const savedEntityCoverUrl = (workId: number, kind: "people" | "series", generation: number) => imageUrl(`saved-entity/${workId}/${kind}/${generation}`);
export const savedCollectionCoverUrl = (workId: number, collectionId: string, generation: number) => imageUrl(`saved-collection/${workId}/${collectionId}/${generation}`);

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
