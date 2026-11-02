import { useEffect, useRef, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { CalendarDays, FileText, Images, MoreHorizontal, UserRound } from "lucide-react";
import { getOfflineEntityCover, getPcEntityCover, type PcWork } from "./pcApi";
import { enqueueCoverRequest, WorkThumbnail } from "./WorkThumbnail";
import { ProviderMark } from "./ProviderMark";
import { MobileTagRow } from "./MobileTagRow";

const avatars = new Map<string, string>();
const pendingAvatars = new Map<string, Promise<string | null>>();
const oversizedAvatars = new Set<string>();
const maxAvatarCacheChars = 4 * 1024 * 1024;
let avatarCacheChars = 0;

function cachedAvatar(key: string): string | null {
  const value = avatars.get(key);
  if (!value) return null;
  avatars.delete(key);
  avatars.set(key, value);
  return value;
}

function cacheAvatar(key: string, value: string): boolean {
  const previous = avatars.get(key);
  if (previous) {
    avatars.delete(key);
    avatarCacheChars -= previous.length;
  }
  // data URL は文字列として WebView に残るため、画像の件数だけでなく総量も制限する。
  if (value.length > maxAvatarCacheChars) {
    oversizedAvatars.add(key);
    if (oversizedAvatars.size > 200) oversizedAvatars.delete(oversizedAvatars.values().next().value!);
    return false;
  }
  while (avatarCacheChars + value.length > maxAvatarCacheChars) {
    const oldest = avatars.keys().next().value;
    if (oldest === undefined) break;
    avatarCacheChars -= avatars.get(oldest)!.length;
    avatars.delete(oldest);
  }
  avatars.set(key, value);
  avatarCacheChars += value.length;
  return true;
}

function discardAvatar(key: string) {
  const cached = avatars.get(key);
  if (cached) {
    avatarCacheChars -= cached.length;
    avatars.delete(key);
  }
  oversizedAvatars.add(key);
  if (oversizedAvatars.size > 200) oversizedAvatars.delete(oversizedAvatars.values().next().value!);
}

export function AuthorAvatar({ work, offline, offlineAuthorCover, snapshotGeneration }: { work: PcWork; offline: boolean; offlineAuthorCover: boolean; snapshotGeneration?: number }) {
  const key = offline ? `saved:${work.id}:${snapshotGeneration ?? 0}` : `${work.source}:${work.authorId}`;
  const node = useRef<HTMLSpanElement>(null);
  const [image, setImage] = useState<string | null>(null);
  useEffect(() => {
    setImage(null);
    if ((offline ? !offlineAuthorCover : !work.authorId) || !isTauri()) return;
    const target = node.current;
    if (!target) return;
    let alive = true;
    let visible = false;
    const load = () => {
      const cached = cachedAvatar(key);
      if (cached) { setImage(cached); return; }
      if (oversizedAvatars.has(key)) return;
      let request = pendingAvatars.get(key);
      if (!request) {
        request = enqueueCoverRequest(() => offline ? getOfflineEntityCover(work.id, "people") : getPcEntityCover("people", work.source, work.authorId))
          .then((asset) => `data:${asset.mimeType};base64,${asset.base64}`)
          .catch(() => null)
          .then((value) => { const accepted = value ? cacheAvatar(key, value) : false; pendingAvatars.delete(key); return accepted ? value : null; });
        pendingAvatars.set(key, request);
      }
      void request.then((value) => { if (alive && visible) setImage(value); });
    };
    if (typeof IntersectionObserver === "undefined") { visible = true; load(); }
    else {
      const observer = new IntersectionObserver((entries) => {
        visible = entries.some((entry) => entry.isIntersecting);
        if (visible) load();
        else setImage(null);
      }, { rootMargin: "180px" });
      observer.observe(target);
      return () => { alive = false; observer.disconnect(); };
    }
    return () => { alive = false; };
  }, [key, offline, offlineAuthorCover, work.authorId, work.id, work.source]);
  return <span className="work-author-avatar" ref={node} aria-hidden="true">{image ? <img src={image} alt="" onError={() => { discardAvatar(key); setImage(null); }} /> : <UserRound />}</span>;
}

function displayDate(value: string | null | undefined): string | null {
  if (!value) return null;
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? null : date.toLocaleDateString("ja-JP");
}

export function WorkCard({ work, onRead, onDetail, offline = false, offlineAuthorCover = false, snapshotGeneration, savedState, prefix }: {
  work: PcWork;
  onRead: () => void;
  onDetail: () => void;
  offline?: boolean;
  offlineAuthorCover?: boolean;
  snapshotGeneration?: number;
  savedState?: { label: string; changed: boolean } | null;
  prefix?: string;
}) {
  const date = displayDate(work.sourceCreatedAt ?? work.downloadedAt);
  return <article className="work-row rich-work-card">
    <button className="work-card-open" type="button" aria-label={`${work.title}を読む`} onClick={onRead}>
      <span className="work-card-main"><WorkThumbnail work={work} offline={offline} snapshotGeneration={snapshotGeneration} /><span className="work-details">
      {work.seriesTitle && <span className="work-series">{work.seriesTitle}</span>}
      <strong>{work.title}</strong>
      <span className="work-author"><AuthorAvatar work={work} offline={offline} offlineAuthorCover={offlineAuthorCover} snapshotGeneration={snapshotGeneration} /><span>{work.authorName}</span><ProviderMark source={work.source} compact /></span>
      <MobileTagRow tags={work.tags ?? []} />
    </span></span>
    </button>
    <div className="work-card-footer">
      {prefix && <span>{prefix}</span>}
      {date && <span className="work-meta"><CalendarDays aria-hidden="true" />{date}</span>}
      <span className="work-meta"><FileText aria-hidden="true" />{work.textLength.toLocaleString("ja-JP")} 字</span>
      {work.assetCount > 0 && <span className="work-meta"><Images aria-hidden="true" />{work.assetCount}</span>}
      {savedState && <span className={savedState.changed ? "saved-state changed" : "saved-state"}>{savedState.label}</span>}
      <button className="work-card-detail" type="button" onClick={onDetail} aria-label={`${work.title}の詳細`}><MoreHorizontal aria-hidden="true" /><span>詳細</span></button>
    </div>
  </article>;
}
