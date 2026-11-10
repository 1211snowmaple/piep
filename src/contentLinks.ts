import { openUrl } from "@tauri-apps/plugin-opener";
import { getPcEntityDetail, listOfflineWorks, lookupPcWork } from "./pcApi";
import type { AppNavigation } from "./appHistory";

export type ContentLinkTarget =
  | { kind: "work"; source: string; sourceId: string }
  | { kind: "person" | "series"; source: string; sourceKey: string };

export function normalizeContentLinkUrl(raw: string): string {
  const match = raw.trim().match(/^pixiv:\/\/(novels?|users?)\/(\d+)\/?$/i);
  if (!match) return raw.trim();
  return /^novels?$/i.test(match[1]) ? `https://www.pixiv.net/novel/show.php?id=${match[2]}` : `https://www.pixiv.net/users/${match[2]}`;
}

/** FANBOX の宛先に含まれるクリエイター名。サブドメインと `/@名前` の両方を読む。 */
function fanboxCreator(url: URL): string | null {
  const host = url.hostname.replace(/^www\./, "");
  if (host.endsWith(".fanbox.cc")) return host.slice(0, -".fanbox.cc".length) || null;
  const inPath = url.pathname.match(/^\/@([^/]+)/);
  return inPath?.[1] ?? null;
}

/**
 * 本文中のリンクが指す作品・作者・シリーズ。アプリが扱う宛先でなければ null。
 *
 * pixiv の説明文はアプリ向けの `pixiv://` で書かれることが多く、ブラウザでも開けない。
 * そうした宛先こそアプリ内で開けるようにする。
 */
export function contentLinkTarget(raw: string): ContentLinkTarget | null {
  let url: URL;
  try {
    url = new URL(normalizeContentLinkUrl(raw.trim()));
  } catch {
    return null;
  }
  // 取得元の内部リンクとして扱うのは、ブラウザで同じサービスへ安全に渡せる
  // 形だけ。`http:` や資格情報付きの見た目だけ FANBOX の URL を、手元の作品へ
  // 解決して本物らしく見せない。
  if (url.protocol !== "https:" || url.port || url.username || url.password) return null;
  const host = url.hostname.replace(/^www\./, "");

  if (host === "pixiv.net") {
    const series = url.pathname.match(/\/novel\/series\/(\d+)/);
    if (series) return { kind: "series", source: "pixiv", sourceKey: series[1] };
    const user = url.pathname.match(/^\/(?:[a-z]{2}\/)?users\/(\d+)/);
    if (user) return { kind: "person", source: "pixiv", sourceKey: user[1] };
    const novelId = url.pathname.match(/^\/(?:[a-z]{2}\/)?novel\/show\.php$/) ? url.searchParams.get("id") : null;
    const novelPath = url.pathname.match(/^\/(?:[a-z]{2}\/)?novels?\/(\d+)/);
    const id = novelId ?? novelPath?.[1];
    if (id && /^\d+$/.test(id)) return { kind: "work", source: "pixiv", sourceId: id };
    return null;
  }

  if (host === "fanbox.cc" || host.endsWith(".fanbox.cc")) {
    const post = url.pathname.match(/\/posts\/(\d+)/);
    if (post) return { kind: "work", source: "fanbox", sourceId: post[1] };
    const creator = fanboxCreator(url);
    if (creator) return { kind: "person", source: "fanbox", sourceKey: creator };
    return null;
  }

  return null;
}


/** サーバーにある宛先を優先し、圏外では保存済みの作品・所属を探す。 */
export async function followContentLink(raw: string, navigation: AppNavigation): Promise<void> {
  const url = normalizeContentLinkUrl(raw);
  const target = contentLinkTarget(url);
  if (target) {
    if (navigation.canBrowseServer) {
      try {
        if (target.kind === "work") {
          const work = await lookupPcWork(target.source, target.sourceId);
          if (work) { navigation.push({ detail: null, reader: { kind: "online", work } }); return; }
        } else {
          const entity = await getPcEntityDetail(target.kind === "person" ? "people" : "series", target.source, target.sourceKey);
          navigation.openEntity(target.kind, target.source, target.sourceKey, entity.displayName);
          return;
        }
      } catch { /* 旧サーバー・圏外では保存済みの宛先を探す。 */ }
    }
    try {
      const saved = await listOfflineWorks();
      if (target.kind === "work") {
        const work = saved.find((item) => item.work.source === target.source && item.work.sourceId === target.sourceId);
        if (work) { navigation.push({ detail: null, reader: { kind: "offline", work } }); return; }
      } else {
        const kind = target.kind === "person" ? "people" : "series";
        const present = saved.some((item) => kind === "people"
          ? (item.people?.some((person) => person.source === target.source && person.key === target.sourceKey) || item.work.source === target.source && item.work.authorId === target.sourceKey)
          : (item.series?.some((series) => series.source === target.source && series.key === target.sourceKey) || item.work.source === target.source && item.work.seriesId === target.sourceKey));
        if (present) { navigation.push({ page: { name: "offline", tab: kind, groupId: `${target.source}:${target.sourceKey}` }, detail: null, reader: null }); return; }
      }
    } catch { /* 端末にない宛先は外部で開く。 */ }
  }
  if (/^https?:\/\//i.test(url)) await openUrl(url);
}
