import { useMemo } from "react";
import { normalizeFanboxPostPayload } from "@/features/browser/downloadMetadata";
import { store } from "@/store";
import {
  getDownload,
  getDownloadBySource,
  markUpdateTargetChecked as markUpdateTargetCheckedDb,
  refreshEntityProfile,
  upsertUpdateTarget as upsertUpdateTargetDb,
} from "@/services/dbApi";
import {
  downloadAndSave,
  fetchFanboxCreatorPosts,
  fetchFanboxPost,
  fetchPixivNovel,
  fetchPixivNovelMetadata,
  fetchPixivSeriesNovels,
  fetchPixivUserNovels,
} from "@/services/downloadApi";

export type UpdateSource = "pixiv" | "fanbox";
export type UpdateTargetType = "work" | "author" | "series";
export type UpdateLogType = "info" | "success" | "warn" | "error";

export interface UpdateCredentials {
  refreshToken: string;
  fanboxCookie: string;
  fanboxUserAgent: string;
}

export interface UpdateDownloadEntry {
  id: number;
  source: UpdateSource;
  sourceId: string;
  title: string;
  authorName: string;
  authorId: string;
  sourceUpdatedAt: string | null;
  currentVersion: number;
  watchUpdates: boolean;
  personId?: string | null;
  seriesId?: string | null;
}

export interface UpdateTarget {
  id?: number;
  targetType: UpdateTargetType;
  source: UpdateSource;
  sourceKey: string;
  displayName: string;
  enabled: boolean;
  lastCheckedAt?: string | null;
  lastSeenSourceId?: string | null;
  lastSeenSourceUpdatedAt?: string | null;
  metadataJson?: string | null;
}

export interface UpdateCandidate {
  key: string;
  source: UpdateSource;
  sourceId: string;
  title: string;
  subtitle: string;
  targetLabel: string;
  targetType: UpdateTargetType;
  originalData: any;
  selected: boolean;
  status?: "pending" | "saving" | "saved" | "failed" | "skipped";
}

export interface UpdateWorkflowResult {
  updatedCount: number;
  foundCount?: number;
  newWorksCount?: number;
  errorCount: number;
}

export const UPDATE_CHECK_DELAY_MS = 1200;
export const UPDATE_SAVE_DELAY_MS = 1000;

export const delay = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));

export function labelForType(type: UpdateTargetType): string {
  if (type === "work") return "作品";
  if (type === "author") return "著者";
  return "シリーズ";
}

export function sourceLabel(source: UpdateSource): string {
  return source === "pixiv" ? "Pixiv" : "FANBOX";
}

export function normalizePixivNovelId(item: any): string {
  return String(item.id ?? item.detail?.id ?? "");
}

export function normalizePixivSeries(item: any): { id: string; title: string } | null {
  const series = item.series && "id" in item.series ? item.series : null;
  const nav = item.seriesNavigation ?? item.series_navigation ?? item.detail?.seriesNavigation ?? item.detail?.series_navigation;
  const id = String(item.seriesId ?? item.series_id ?? item.detail?.seriesId ?? item.detail?.series_id ?? series?.id ?? nav?.seriesId ?? nav?.series_id ?? "");
  const title = String(item.seriesTitle ?? item.series_title ?? item.detail?.seriesTitle ?? item.detail?.series_title ?? series?.title ?? nav?.seriesTitle ?? nav?.series_title ?? "");
  return id ? { id, title: title || "シリーズ" } : null;
}

export function pixivTags(data: any): string[] {
  const tags = data.detail?.tags ?? data.tags ?? [];
  if (Array.isArray(tags)) return tags.map((tag: any) => tag.name || tag).filter(Boolean);
  if (Array.isArray(tags.tags)) return tags.tags.map((tag: any) => tag.name || tag).filter(Boolean);
  return [];
}

export async function getUpdateCredentials(): Promise<UpdateCredentials> {
  return {
    refreshToken: await store.get<string>("pixiv_refresh_token") || "",
    fanboxCookie: await store.get<string>("fanbox_session_id") || "",
    fanboxUserAgent: await store.get<string>("fanbox_user_agent") || "Mozilla/5.0",
  };
}

export async function upsertUpdateTarget(
  targetType: UpdateTargetType,
  source: UpdateSource,
  sourceKey: string,
  displayName: string,
  enabled = true,
  metadata: any = null,
) {
  await upsertUpdateTargetDb({
    targetType,
    source,
    sourceKey,
    displayName,
    enabled,
    metadataJson: metadata ? JSON.stringify(metadata) : null,
  });
}

export async function markUpdateTargetChecked(
  target: Pick<UpdateTarget, "targetType" | "source" | "sourceKey">,
  lastSeenId?: string,
  lastSeenUpdated?: string | null,
) {
  await markUpdateTargetCheckedDb({
    targetType: target.targetType,
    source: target.source,
    sourceKey: target.sourceKey,
    lastSeenSourceId: lastSeenId || null,
    lastSeenSourceUpdatedAt: lastSeenUpdated || null,
  });
}

export async function checkWatchedWorks({
  watchedWorks,
  credentials,
  onLog,
  countOnlyVersionBumps = false,
  delayMs = UPDATE_CHECK_DELAY_MS,
}: {
  watchedWorks: UpdateDownloadEntry[];
  credentials: UpdateCredentials;
  onLog?: (type: UpdateLogType, text: string) => void;
  countOnlyVersionBumps?: boolean;
  delayMs?: number;
}): Promise<UpdateWorkflowResult> {
  let updatedCount = 0;
  let errorCount = 0;

  for (const dl of watchedWorks) {
    try {
      if (dl.source === "pixiv") {
        if (!credentials.refreshToken) {
          onLog?.("warn", "Pixiv未連携のためPixiv作品をスキップしました");
          continue;
        }
        const metadata: any = await fetchPixivNovelMetadata(dl.sourceId, credentials.refreshToken);
        const metadataUpdatedAt = metadata.create_date || null;
        if (dl.sourceUpdatedAt === metadataUpdatedAt) {
          onLog?.("info", `最新: ${dl.title}`);
          continue;
        }

        const data: any = await fetchPixivNovel(dl.sourceId, credentials.refreshToken);
        await downloadAndSave({
          data,
          source: "pixiv",
          sourceId: dl.sourceId,
          title: data.detail?.title || dl.title,
          authorName: data.detail?.user?.name || dl.authorName,
          authorId: String(data.detail?.user?.id || dl.authorId),
          contentType: "novel",
          tags: pixivTags(data),
          excerpt: data.detail?.caption || null,
          sourceCreatedAt: data.detail?.create_date || null,
          cookie: null,
          userAgent: null,
        });
      } else {
        if (!credentials.fanboxCookie) {
          onLog?.("warn", "FANBOX未連携のためFANBOX作品をスキップしました");
          continue;
        }
        const post = normalizeFanboxPostPayload<any>(await fetchFanboxPost(dl.sourceId, credentials.fanboxCookie, credentials.fanboxUserAgent));
        const postUpdatedAt = post.updatedDatetime || post.updated_datetime || null;
        if (dl.sourceUpdatedAt === postUpdatedAt) {
          onLog?.("info", `最新: ${dl.title}`);
          continue;
        }
        await downloadAndSave({
          data: post,
          source: "fanbox",
          sourceId: dl.sourceId,
          title: post.title || dl.title,
          authorName: post.user?.name || dl.authorName,
          authorId: post.creatorId || post.creator_id || dl.authorId,
          contentType: post.type || post.postType || post.post_type || "article",
          tags: post.tags || [],
          excerpt: post.excerpt || null,
          sourceCreatedAt: post.publishedDatetime || post.published_datetime || null,
          cookie: credentials.fanboxCookie,
          userAgent: credentials.fanboxUserAgent,
        });
      }

      if (countOnlyVersionBumps) {
        const currentDl = await getDownload<UpdateDownloadEntry>(dl.id);
        if (currentDl.currentVersion > dl.currentVersion) updatedCount++;
      } else {
        updatedCount++;
      }
      onLog?.("success", `更新を保存: ${dl.title}`);
    } catch (error) {
      errorCount++;
      onLog?.("error", `${dl.title}: ${error}`);
    }
    await delay(delayMs);
  }

  return { updatedCount, errorCount };
}

export async function fetchCollectionItems(target: UpdateTarget, credentials: UpdateCredentials): Promise<any[]> {
  if (target.source === "pixiv" && target.targetType === "author") {
    if (!credentials.refreshToken) return [];
    return fetchPixivUserNovels<any[]>(target.sourceKey, credentials.refreshToken);
  }
  if (target.source === "pixiv" && target.targetType === "series") {
    if (!credentials.refreshToken) return [];
    return fetchPixivSeriesNovels<any[]>(target.sourceKey, credentials.refreshToken);
  }
  if (target.source === "fanbox" && target.targetType === "author") {
    if (!credentials.fanboxCookie) return [];
    return fetchFanboxCreatorPosts<any[]>(target.sourceKey, credentials.fanboxCookie, credentials.fanboxUserAgent);
  }
  return [];
}

export function candidateFromTargetItem(target: UpdateTarget, item: any): UpdateCandidate | null {
  const sourceId = target.source === "pixiv" ? normalizePixivNovelId(item) : String(item.id || "");
  if (!sourceId) return null;
  return {
    key: `${target.targetType}:${target.source}:${target.sourceKey}:${sourceId}`,
    source: target.source,
    sourceId,
    title: item.title || "無題",
    subtitle: target.source === "pixiv"
      ? `${item.user?.name || target.displayName} / ${item.create_date || item.createDate || ""}`
      : `${item.user?.name || target.displayName} / ${item.publishedDatetime || item.published_datetime || ""}`,
    targetLabel: target.displayName,
    targetType: target.targetType,
    originalData: item,
    selected: true,
  };
}

export async function findNewCandidatesForTargets({
  targets,
  credentials,
  onLog,
  delayMs = UPDATE_CHECK_DELAY_MS,
}: {
  targets: UpdateTarget[];
  credentials: UpdateCredentials;
  onLog?: (type: UpdateLogType, text: string) => void;
  delayMs?: number;
}): Promise<{ candidates: UpdateCandidate[]; errorCount: number }> {
  const candidates: UpdateCandidate[] = [];
  let errorCount = 0;

  for (const target of targets) {
    try {
      onLog?.("info", `${labelForType(target.targetType)}を確認中: ${target.displayName}`);
      const items = await fetchCollectionItems(target, credentials);

      for (const item of items) {
        const candidate = candidateFromTargetItem(target, item);
        if (!candidate) continue;
        const existing = await getDownloadBySource<UpdateDownloadEntry>(candidate.source, candidate.sourceId);
        if (!existing) candidates.push(candidate);
      }

      const first = items[0];
      const firstId = target.source === "pixiv" ? normalizePixivNovelId(first || {}) : String(first?.id || "");
      const firstUpdated = target.source === "pixiv"
        ? first?.create_date || first?.createDate || null
        : first?.updatedDatetime || first?.updated_datetime || null;
      await markUpdateTargetChecked(target, firstId, firstUpdated);
    } catch (error) {
      errorCount++;
      onLog?.("error", `${target.displayName}: ${error}`);
    }
    await delay(delayMs);
  }

  return { candidates, errorCount };
}

export async function saveUpdateCandidate(
  candidate: UpdateCandidate,
  credentials: UpdateCredentials,
  options: { autoWatchSeries?: boolean } = {},
): Promise<UpdateDownloadEntry | null> {
  const existing = await getDownloadBySource<UpdateDownloadEntry>(candidate.source, candidate.sourceId);
  if (existing) return null;

  if (candidate.source === "pixiv") {
    const data: any = await fetchPixivNovel(candidate.sourceId, credentials.refreshToken);
    const savedEntry = await downloadAndSave<UpdateDownloadEntry>({
      data,
      source: "pixiv",
      sourceId: candidate.sourceId,
      title: data.detail?.title || candidate.title,
      authorName: data.detail?.user?.name || candidate.originalData.user?.name || "unknown",
      authorId: String(data.detail?.user?.id || candidate.originalData.user?.id || "0"),
      contentType: "novel",
      tags: pixivTags(data),
      excerpt: data.detail?.caption || null,
      sourceCreatedAt: data.detail?.create_date || candidate.originalData.create_date || null,
      cookie: null,
      userAgent: null,
    });

    if (options.autoWatchSeries) {
      const series = normalizePixivSeries(candidate.originalData);
      if (series) await upsertUpdateTarget("series", "pixiv", series.id, series.title, true);
    }

    return savedEntry;
  }

  const post = normalizeFanboxPostPayload<any>(await fetchFanboxPost(candidate.sourceId, credentials.fanboxCookie, credentials.fanboxUserAgent));
  return downloadAndSave<UpdateDownloadEntry>({
    data: post,
    source: "fanbox",
    sourceId: candidate.sourceId,
    title: post.title || candidate.title,
    authorName: post.user?.name || candidate.originalData.user?.name || "unknown",
    authorId: post.creatorId || post.creator_id || candidate.originalData.creatorId || candidate.originalData.creator_id || "0",
    contentType: post.type || post.postType || post.post_type || "article",
    tags: post.tags || [],
    excerpt: post.excerpt || null,
    sourceCreatedAt: post.publishedDatetime || post.published_datetime || null,
    cookie: credentials.fanboxCookie,
    userAgent: credentials.fanboxUserAgent,
  });
}

export async function refreshEntityProfilesForEntries(
  entries: UpdateDownloadEntry[],
  credentials: UpdateCredentials,
  onLog?: (type: UpdateLogType, text: string) => void,
) {
  const queue = new Map<string, { entityType: "person" | "series"; source: UpdateSource; sourceKey: string }>();
  for (const entry of entries) {
    const personKey = entry.personId || entry.authorId;
    if (personKey) {
      queue.set(`person:${entry.source}:${personKey}`, {
        entityType: "person",
        source: entry.source,
        sourceKey: personKey,
      });
    }
    if (entry.seriesId) {
      queue.set(`series:${entry.source}:${entry.seriesId}`, {
        entityType: "series",
        source: entry.source,
        sourceKey: entry.seriesId,
      });
    }
  }

  if (queue.size > 0) onLog?.("info", `作者・シリーズ情報を確認中... (${queue.size}件)`);
  for (const target of queue.values()) {
    try {
      await refreshEntityProfile({
        entityType: target.entityType,
        source: target.source,
        sourceKey: target.sourceKey,
        force: false,
        refreshToken: credentials.refreshToken,
        cookie: credentials.fanboxCookie,
        userAgent: credentials.fanboxUserAgent,
      });
    } catch (error) {
      onLog?.("warn", `${target.entityType}:${target.source}:${target.sourceKey} の確認をスキップ: ${error}`);
    }
  }
}

export async function runUpdateCheckAndAutoSave({
  watchedWorks,
  collectionTargets,
  countOnlyVersionBumps = false,
  onLog,
  onNewWorkSaving,
}: {
  watchedWorks: UpdateDownloadEntry[];
  collectionTargets: UpdateTarget[];
  countOnlyVersionBumps?: boolean;
  onLog?: (type: UpdateLogType, text: string) => void;
  onNewWorkSaving?: (candidate: UpdateCandidate) => void;
}): Promise<Required<Pick<UpdateWorkflowResult, "updatedCount" | "errorCount">> & { newWorksCount: number }> {
  const credentials = await getUpdateCredentials();
  const workResult = await checkWatchedWorks({
    watchedWorks,
    credentials,
    countOnlyVersionBumps,
    onLog,
  });
  const candidateResult = await findNewCandidatesForTargets({
    targets: collectionTargets,
    credentials,
    onLog,
  });

  let newWorksCount = 0;
  let saveErrorCount = 0;
  const savedEntries: UpdateDownloadEntry[] = [];

  for (const candidate of candidateResult.candidates) {
    try {
      onNewWorkSaving?.(candidate);
      const savedEntry = await saveUpdateCandidate(candidate, credentials, { autoWatchSeries: true });
      if (savedEntry) {
        savedEntries.push(savedEntry);
        newWorksCount++;
      }
    } catch (error) {
      saveErrorCount++;
      onLog?.("error", `${candidate.title}: ${error}`);
    }
    await delay(UPDATE_SAVE_DELAY_MS);
  }

  await refreshEntityProfilesForEntries(savedEntries, credentials, onLog);

  return {
    updatedCount: workResult.updatedCount,
    newWorksCount,
    errorCount: workResult.errorCount + candidateResult.errorCount + saveErrorCount,
  };
}

export function useUpdateWorkflow() {
  return useMemo(() => ({
    getUpdateCredentials,
    checkWatchedWorks,
    findNewCandidatesForTargets,
    saveUpdateCandidate,
    refreshEntityProfilesForEntries,
    runUpdateCheckAndAutoSave,
    upsertUpdateTarget,
    markUpdateTargetChecked,
    delay,
  }), []);
}
