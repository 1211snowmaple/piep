import { openUrl } from "@tauri-apps/plugin-opener";
import { getPcEntityDetail, listOfflineWorks, lookupPcWork } from "./pcApi";
import type { AppNavigation } from "./appHistory";
import { contentLinkTarget, normalizeContentLinkUrl } from "@shared/contentLinkTarget";

export { contentLinkTarget, normalizeContentLinkUrl };
export type { ContentLinkTarget } from "@shared/contentLinkTarget";

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
