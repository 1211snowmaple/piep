import { CSSProperties, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ask, save } from "@tauri-apps/plugin-dialog";
import { store } from "../../store";
import {
  BookIcon,
  CloseIcon,
  ExportIcon,
  FanboxIcon,
  ImageIcon,
  LinkIcon,
  PixivIcon,
  RefreshIcon,
  SkebIcon,
  TrashIcon,
  XIcon,
} from "../icons/Icons";

interface DownloadEntry {
  id: number;
  source: string;
  sourceId: string;
  title: string;
  authorName: string;
  authorId?: string;
  coverPath: string | null;
  downloadedAt: string;
  sourceCreatedAt: string | null;
  sourceUpdatedAt?: string | null;
  currentVersion?: number;
  watchUpdates?: boolean;
  textLength: number;
  assetCount: number;
  fileSizeBytes: number;
  contentType?: string;
  tags?: string | string[] | null;
  excerpt?: string | null;
  seriesId?: string | null;
  seriesTitle?: string | null;
  personId?: string | null;
}

interface PersonEntry {
  source: string;
  sourceKey: string;
  displayName: string;
  iconPath: string | null;
  coverPath: string | null;
  description: string | null;
  linksJson: string | null;
  currentVersion: number;
  lastCheckedAt: string | null;
  workCount: number | null;
}

interface SeriesEntry {
  source: string;
  sourceKey: string;
  title: string;
  description: string | null;
  coverPath: string | null;
  currentVersion: number;
  lastCheckedAt: string | null;
  workCount: number | null;
}

interface EntityVersion {
  id: number;
  version: number;
}

interface Props {
  source: string;
  sourceKey: string;
  showToast: (msg: string, type: "success" | "error" | "info") => void;
  onViewDetail: (id: number) => void;
  onViewSeries: (source: string, sourceKey: string) => void;
  onOpenSourceUrl: (url: string) => void;
  onOpenEpubForWorks: (works: DownloadEntry[]) => void;
  onLibraryChanged: () => void;
  epubSidebarOpen?: boolean;
}

const ENTITY_CARD_WIDTH = 338;
const ENTITY_CARD_GAP = 16;
const ENTITY_MAX_COLUMNS = 4;

function useEntityLayoutStyle() {
  const ref = useRef<HTMLDivElement | null>(null);
  const [columns, setColumns] = useState(2);

  useEffect(() => {
    const element = ref.current;
    const parent = element?.parentElement;
    if (!parent) return;

    const update = () => {
      const available = parent.clientWidth;
      const nextColumns = Math.max(
        1,
        Math.min(
          ENTITY_MAX_COLUMNS,
          Math.floor((available + ENTITY_CARD_GAP) / (ENTITY_CARD_WIDTH + ENTITY_CARD_GAP)),
        ),
      );
      setColumns(nextColumns);
    };

    update();
    const observer = new ResizeObserver(update);
    observer.observe(parent);
    return () => observer.disconnect();
  }, []);

  const width = columns * ENTITY_CARD_WIDTH + (columns - 1) * ENTITY_CARD_GAP;
  return {
    ref,
    style: {
      "--entity-columns": String(columns),
      "--entity-content-width": `${width}px`,
    } as CSSProperties,
  };
}

function formatDate(value: string | null | undefined): string {
  if (!value) return "未確認";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString("ja-JP", { year: "numeric", month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
}

function formatBytes(bytes: number): string {
  if (!bytes) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  const idx = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  return `${(bytes / Math.pow(1024, idx)).toFixed(1)} ${units[idx]}`;
}

function parseLinks(value: string | null): string[] {
  if (!value) return [];
  try {
    const parsed = JSON.parse(value);
    return Array.isArray(parsed) ? parsed.map(String).filter(Boolean) : [];
  } catch {
    return [];
  }
}

function defaultSourceLink(source: string, sourceKey: string, type: "person" | "series"): string | null {
  if (source === "pixiv") {
    return type === "series"
      ? `https://www.pixiv.net/novel/series/${sourceKey}`
      : `https://www.pixiv.net/users/${sourceKey}`;
  }
  if (source === "fanbox" && type === "person") {
    return `https://${sourceKey}.fanbox.cc/`;
  }
  return null;
}

function linkKind(value: string): "pixiv" | "x" | "skeb" | "fanbox" | "other" {
  try {
    const host = new URL(value).hostname.replace(/^www\./, "").toLowerCase();
    if (host === "pixiv.net" || host.endsWith(".pixiv.net")) return "pixiv";
    if (host === "x.com" || host === "twitter.com") return "x";
    if (host === "skeb.jp") return "skeb";
    if (host === "fanbox.cc" || host.endsWith(".fanbox.cc")) return "fanbox";
  } catch {}
  return "other";
}

function displayLink(value: string): string {
  try {
    const url = new URL(value);
    return `${url.hostname}${url.pathname}`.replace(/\/$/, "");
  } catch {
    return value.replace(/^https?:\/\//, "");
  }
}

function normalizePixivNovelId(item: any): string {
  return String(item.id ?? item.detail?.id ?? "");
}

function normalizePixivSeries(item: any): { id: string; title: string } | null {
  const series = item.series && "id" in item.series ? item.series : null;
  const nav = item.seriesNavigation ?? item.series_navigation ?? item.detail?.seriesNavigation ?? item.detail?.series_navigation;
  const id = String(item.seriesId ?? item.series_id ?? item.detail?.seriesId ?? item.detail?.series_id ?? series?.id ?? nav?.seriesId ?? nav?.series_id ?? "");
  const title = String(item.seriesTitle ?? item.series_title ?? item.detail?.seriesTitle ?? item.detail?.series_title ?? series?.title ?? nav?.seriesTitle ?? nav?.series_title ?? "");
  return id ? { id, title: title || "シリーズ" } : null;
}

function pixivTags(data: any): string[] {
  const tags = data.detail?.tags ?? data.tags ?? [];
  if (Array.isArray(tags)) return tags.map((t: any) => t.name || t).filter(Boolean);
  if (Array.isArray(tags.tags)) return tags.tags.map((t: any) => t.name || t).filter(Boolean);
  return [];
}

function profileStat(profileJson: any, key: string): number | null {
  const value = profileJson?.stats?.[key];
  return typeof value === "number" ? value : null;
}

function useImage(path: string | null | undefined) {
  const [src, setSrc] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    if (!path) {
      setSrc(null);
      return;
    }
    invoke<string>("read_image_base64", { path })
      .then((value) => {
        if (!cancelled) setSrc(value);
      })
      .catch(() => {
        if (!cancelled) setSrc(null);
      });
    return () => {
      cancelled = true;
    };
  }, [path]);
  return src;
}

function EntityLinkIconButton({ link, onOpenSourceUrl }: { link: string; onOpenSourceUrl: (url: string) => void }) {
  const kind = linkKind(link);
  const icon = kind === "pixiv" ? <PixivIcon /> : kind === "x" ? <XIcon /> : kind === "skeb" ? <SkebIcon /> : kind === "fanbox" ? <FanboxIcon /> : <LinkIcon />;
  return (
    <button
      type="button"
      className={`entity-link-icon ${kind}`}
      onClick={() => onOpenSourceUrl(link)}
      title={displayLink(link)}
      aria-label={displayLink(link)}
    >
      {icon}
    </button>
  );
}

function EntityActionBar({
  works,
  selectedCount,
  deleteMode,
  epubMode,
  allSelected,
  busy,
  refreshing,
  epubSidebarOpen,
  onRefresh,
  onOpenEpub,
  onEnterDelete,
  onConfirmDelete,
  onCancelDelete,
  onSelectAll,
  onSelectNone,
  onBackup,
}: {
  works: DownloadEntry[];
  selectedCount: number;
  deleteMode: boolean;
  epubMode: boolean;
  allSelected: boolean;
  busy: boolean;
  refreshing: boolean;
  epubSidebarOpen?: boolean;
  onRefresh: () => void;
  onOpenEpub: () => void;
  onEnterDelete: () => void;
  onConfirmDelete: () => void;
  onCancelDelete: () => void;
  onSelectAll: () => void;
  onSelectNone: () => void;
  onBackup: () => void;
}) {
  const selectionActive = deleteMode || epubMode;
  const selectionLabel = deleteMode ? "削除" : "EPUB";
  return (
    <div className={`viewer-actions entity-action-bar ${deleteMode ? "delete-active" : ""} ${epubMode ? "epub-active" : ""}`}>
      <div className="viewer-action-group file-action-group">
        {selectionActive ? (
          <>
            <div className={`entity-selection-controls ${deleteMode ? "delete-selection-group" : "epub-selection-group"}`}>
              <span className={`epub-selection-badge entity-selection-badge ${selectedCount === 0 ? "empty" : ""}`}>
                {selectionLabel}: {selectedCount} 件選択
              </span>
              <button
                type="button"
                className={`toolbar-btn entity-select-toggle ${allSelected ? "active" : ""}`}
                onClick={allSelected ? onSelectNone : onSelectAll}
                disabled={busy || works.length === 0}
                title={allSelected ? "すべての選択を解除" : "このページの作品をすべて選択"}
              >
                {allSelected ? "全解除" : "全選択"}
              </button>
            </div>
            {deleteMode && (
              <button
                type="button"
                className="icon-btn entity-delete-confirm-btn"
                onClick={onConfirmDelete}
                disabled={busy || selectedCount === 0}
                title="選択した作品を削除します"
                aria-label="削除確定"
              >
                <TrashIcon />
              </button>
            )}
            <button
              type="button"
              className="icon-btn"
              onClick={onCancelDelete}
              disabled={busy}
              title={deleteMode ? "削除選択をキャンセルします" : "EPUB選択を終了します"}
              aria-label={deleteMode ? "削除キャンセル" : "EPUB選択終了"}
            >
              <CloseIcon />
            </button>
          </>
        ) : (
          <>
            <button
              type="button"
              className="icon-btn"
              onClick={onRefresh}
              disabled={busy || refreshing}
              title="プロフィールと新作を確認します"
              aria-label="更新チェック"
            >
              {refreshing ? <span className="spinner-mini" style={{ display: "inline-block" }} /> : <RefreshIcon />}
            </button>
            <button
              type="button"
              className="icon-btn"
              onClick={onEnterDelete}
              disabled={busy || works.length === 0}
              title="作品を選択して削除します"
              aria-label="削除"
            >
              <TrashIcon />
            </button>
          </>
        )}
          <button
            type="button"
            className="icon-btn"
            onClick={onBackup}
            disabled={busy}
            title="このページの作品だけをZIPバックアップとして作成します"
            aria-label="バックアップ作成"
          >
            <ExportIcon />
          </button>
      </div>
      <div className="viewer-action-group epub-action-group">
        <button
          type="button"
          className={`icon-btn sidebar-toggle-btn ${epubSidebarOpen || epubMode ? "primary" : ""}`}
          onClick={onOpenEpub}
          disabled={busy || works.length === 0}
          title={epubSidebarOpen || epubMode ? "EPUB選択を終了します" : "EPUBにする作品を選択します"}
          aria-label="EPUB"
        >
          <BookIcon />
        </button>
      </div>
    </div>
  );
}

function EntityWorkGrid({
  works,
  selectedIds,
  deleteMode,
  epubMode,
  onToggleSelect,
  onViewDetail,
  onViewSeries,
}: {
  works: DownloadEntry[];
  selectedIds: Set<number>;
  deleteMode: boolean;
  epubMode: boolean;
  onToggleSelect: (id: number) => void;
  onViewDetail: (id: number) => void;
  onViewSeries: (source: string, sourceKey: string) => void;
}) {
  return (
    <div className="entity-work-grid">
      {works.map((work) => {
        const selected = selectedIds.has(work.id);
        const selectionMode = deleteMode || epubMode;
        return (
          <button
            key={work.id}
            type="button"
            className={`entity-work-card ${deleteMode ? "delete-mode" : ""} ${epubMode ? "epub-mode" : ""} ${selected ? "checked" : ""} ${epubMode && selected ? "epub-checked" : ""}`}
            onClick={() => selectionMode ? onToggleSelect(work.id) : onViewDetail(work.id)}
          >
            {deleteMode && (
              <span className="entity-card-delete-overlay">
                <span className={`entity-card-delete-wrapper ${selected ? "active" : ""}`}>
                  <TrashIcon />
                </span>
              </span>
            )}
            {epubMode && (
              <span className="entity-card-epub-overlay">
                <span className={`card-epub-check-wrapper ${selected ? "active" : ""}`}>
                  <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round">
                    <polyline points="20 6 9 17 4 12" />
                  </svg>
                </span>
              </span>
            )}
            <EntityWorkCover path={work.coverPath} />
            <div className="entity-work-info">
              {work.seriesId && work.seriesTitle && (
                <span
                  className="entity-work-series clickable"
                  onClick={(e) => {
                    e.stopPropagation();
                    if (!selectionMode) onViewSeries(work.source, work.seriesId!);
                  }}
                  title="シリーズページを開く"
                >
                  {work.seriesTitle}
                </span>
              )}
              <strong title={work.title}>{work.title}</strong>
              <span>{work.authorName}</span>
              <small>{formatDate(work.sourceCreatedAt || work.downloadedAt)} · {work.textLength.toLocaleString()}字 · {work.assetCount} assets · {formatBytes(work.fileSizeBytes)}</small>
            </div>
          </button>
        );
      })}
    </div>
  );
}

function EntityWorkCover({ path }: { path: string | null }) {
  const src = useImage(path);
  return (
    <span className="entity-work-cover">
      {src ? <img src={src} alt="" /> : <ImageIcon />}
    </span>
  );
}

function useEntityTools({
  works,
  entityType,
  source,
  sourceKey,
  displayName,
  showToast,
  onOpenEpubForWorks,
  onLibraryChanged,
  load,
}: {
  works: DownloadEntry[];
  entityType: "person" | "series";
  source: string;
  sourceKey: string;
  displayName: string;
  showToast: Props["showToast"];
  onOpenEpubForWorks: (works: DownloadEntry[]) => void;
  onLibraryChanged: () => void;
  load: () => Promise<void>;
}) {
  const [selectionMode, setSelectionMode] = useState<"delete" | "epub" | null>(null);
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set());
  const [busy, setBusy] = useState(false);
  const selectedWorks = useCallback((ids: Set<number>) => works.filter(work => ids.has(work.id)), [works]);

  const toggleSelect = useCallback((id: number) => {
    setSelectedIds(prev => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id); else next.add(id);
      if (selectionMode === "epub") {
        onOpenEpubForWorks(selectedWorks(next));
      }
      return next;
    });
  }, [onOpenEpubForWorks, selectedWorks, selectionMode]);

  const selectAll = useCallback(() => {
    const next = new Set(works.map(work => work.id));
    setSelectedIds(next);
    if (selectionMode === "epub") {
      onOpenEpubForWorks(works);
    }
  }, [onOpenEpubForWorks, selectionMode, works]);

  const selectNone = useCallback(() => {
    const next = new Set<number>();
    setSelectedIds(next);
    if (selectionMode === "epub") {
      onOpenEpubForWorks([]);
    }
  }, [onOpenEpubForWorks, selectionMode]);

  const cancelSelection = useCallback(() => {
    if (selectionMode === "epub") {
      onOpenEpubForWorks(selectedWorks(selectedIds));
    }
    setSelectionMode(null);
    setSelectedIds(new Set());
  }, [onOpenEpubForWorks, selectedIds, selectedWorks, selectionMode]);

  const confirmDelete = useCallback(async () => {
    const ids = Array.from(selectedIds);
    if (ids.length === 0) return;
    const confirmed = await ask(
      `選択された ${ids.length} 件の作品を完全に削除してもよろしいですか？\nアセットファイルや関連データも完全に消去されます。`,
      { title: "削除の確認", kind: "warning", okLabel: "削除する", cancelLabel: "キャンセル" },
    );
    if (!confirmed) return;
    setBusy(true);
    try {
      await Promise.all(ids.map(id => invoke("db_delete_download", { id })));
      showToast(`${ids.length} 件の作品を完全に削除しました`, "success");
      cancelSelection();
      onLibraryChanged();
      await load();
    } catch (e) {
      showToast(`削除エラー: ${e}`, "error");
    } finally {
      setBusy(false);
    }
  }, [cancelSelection, load, onLibraryChanged, selectedIds, showToast]);

  const createBackup = useCallback(async () => {
    try {
      const safeName = displayName.replace(/[<>:"/\\|?*\u0000-\u001F]/g, "_").trim() || sourceKey;
      const path = await save({ filters: [{ name: "ZIP", extensions: ["zip"] }], defaultPath: `piep_${entityType}_${safeName}.zip` });
      if (!path) return;
      setBusy(true);
      showToast("このページのバックアップを作成中...", "info");
      await invoke("export_entity_zip", { entityType, source, sourceKey, zipPath: path });
      showToast("このページのバックアップ作成が完了しました", "success");
    } catch (e) {
      showToast(`バックアップ作成エラー: ${e}`, "error");
    } finally {
      setBusy(false);
    }
  }, [displayName, entityType, source, sourceKey, showToast]);

  const openEpub = useCallback(() => {
    if (works.length === 0) {
      showToast("EPUBにできる保存済み作品がありません", "info");
      return;
    }
    if (selectionMode === "epub") {
      onOpenEpubForWorks(selectedWorks(selectedIds));
      setSelectionMode(null);
      setSelectedIds(new Set());
      return;
    }
    const allIds = new Set(works.map(work => work.id));
    setSelectionMode("epub");
    setSelectedIds(allIds);
    onOpenEpubForWorks(works);
  }, [onOpenEpubForWorks, selectedIds, selectedWorks, selectionMode, showToast, works]);

  const enterDelete = useCallback(() => {
    setSelectionMode("delete");
    setSelectedIds(new Set());
  }, []);

  return {
    busy,
    deleteMode: selectionMode === "delete",
    epubMode: selectionMode === "epub",
    selectedIds,
    allSelected: works.length > 0 && works.every(work => selectedIds.has(work.id)),
    toggleSelect,
    selectAll,
    selectNone,
    cancelSelection,
    confirmDelete,
    createBackup,
    openEpub,
    enterDelete,
  };
}

async function refreshRelatedEntities(entry: DownloadEntry, refreshToken: string, cookie: string, userAgent: string) {
  const targets = [
    { entityType: "person", source: entry.source, sourceKey: entry.personId || entry.authorId },
    entry.seriesId ? { entityType: "series", source: entry.source, sourceKey: entry.seriesId } : null,
  ].filter((target): target is { entityType: string; source: string; sourceKey: string } => !!target?.sourceKey);

  for (const target of targets) {
    try {
      await invoke("refresh_entity_profile", {
        params: {
          ...target,
          force: false,
          refreshToken,
          cookie,
          userAgent,
        },
      });
    } catch {
      // Profile refresh is best effort after saving a work.
    }
  }
}

async function savePixivNovel(item: any, refreshToken: string, fallbackAuthorName: string, fallbackAuthorId: string): Promise<{ saved: DownloadEntry; wasExisting: DownloadEntry | null }> {
  const sourceId = normalizePixivNovelId(item);
  const wasExisting = await invoke<DownloadEntry | null>("db_get_download_by_source", { source: "pixiv", sourceId });
  const data: any = await invoke("fetch_pixiv_novel", { novelId: sourceId, refreshToken });
  const saved = await invoke<DownloadEntry>("download_and_save", {
    data,
    source: "pixiv",
    sourceId,
    title: data.detail?.title || item.title || "pixiv_novel",
    authorName: data.detail?.user?.name || item.user?.name || fallbackAuthorName || "unknown",
    authorId: String(data.detail?.user?.id || item.user?.id || fallbackAuthorId || "0"),
    contentType: "novel",
    tags: pixivTags(data),
    excerpt: data.detail?.caption || null,
    sourceCreatedAt: data.detail?.create_date || item.createDate || item.create_date || null,
    cookie: null,
    userAgent: null,
  });
  return { saved, wasExisting };
}

async function saveFanboxPost(item: any, cookie: string, userAgent: string, fallbackAuthorName: string, fallbackAuthorId: string): Promise<{ saved: DownloadEntry; wasExisting: DownloadEntry | null }> {
  const sourceId = String(item.id ?? "");
  const wasExisting = await invoke<DownloadEntry | null>("db_get_download_by_source", { source: "fanbox", sourceId });
  const data: any = await invoke("fetch_fanbox_post", { postId: sourceId, cookie, userAgent });
  const saved = await invoke<DownloadEntry>("download_and_save", {
    data,
    source: "fanbox",
    sourceId,
    title: data.title || item.title || "fanbox_post",
    authorName: data.user?.name || item.user?.name || fallbackAuthorName || "unknown",
    authorId: data.creatorId || item.creatorId || data.user?.userId || fallbackAuthorId || "0",
    contentType: data.type || item.type || "article",
    tags: data.tags || item.tags || [],
    excerpt: null,
    sourceCreatedAt: data.publishedDatetime || item.publishedDatetime || null,
    cookie,
    userAgent,
  });
  return { saved, wasExisting };
}

function summarizeSave(wasExisting: DownloadEntry | null, saved: DownloadEntry) {
  if (!wasExisting) return "new";
  if ((saved.currentVersion ?? 0) > (wasExisting.currentVersion ?? 0)) return "updated";
  return "skipped";
}

export function PersonView({ source, sourceKey, showToast, onViewDetail, onViewSeries, onOpenSourceUrl, onOpenEpubForWorks, onLibraryChanged, epubSidebarOpen }: Props) {
  const [person, setPerson] = useState<PersonEntry | null>(null);
  const [works, setWorks] = useState<DownloadEntry[]>([]);
  const [versions, setVersions] = useState<EntityVersion[]>([]);
  const [profileJson, setProfileJson] = useState<any>(null);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const icon = useImage(person?.iconPath);
  const cover = useImage(person?.coverPath);
  const layout = useEntityLayoutStyle();
  const links = useMemo(() => {
    const parsed = parseLinks(person?.linksJson || null);
    const fallback = defaultSourceLink(source, sourceKey, "person");
    return Array.from(new Set(parsed.length > 0 ? parsed : fallback ? [fallback] : []));
  }, [person?.linksJson, source, sourceKey]);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [personData, workData, versionData, latestJson] = await Promise.all([
        invoke<PersonEntry>("db_get_person", { source, sourceKey }),
        invoke<DownloadEntry[]>("db_search_downloads", {
          params: { limit: 500, offset: 0, sortBy: "published", sortOrder: "desc", personSource: source, personKey: sourceKey },
        }),
        invoke<EntityVersion[]>("db_list_entity_versions", { entityType: "person", source, sourceKey }),
        invoke<any | null>("db_get_latest_entity_profile_json", { entityType: "person", source, sourceKey }).catch(() => null),
      ]);
      setPerson(personData);
      setWorks(workData);
      setVersions(versionData);
      setProfileJson(latestJson);
    } catch (e) {
      showToast(`人物情報を読み込めませんでした: ${e}`, "error");
    } finally {
      setLoading(false);
    }
  }, [source, sourceKey, showToast]);

  useEffect(() => {
    document.querySelector(".library-main-content")?.scrollTo({ top: 0 });
    load();
  }, [load]);

  const tools = useEntityTools({
    works,
    entityType: "person",
    source,
    sourceKey,
    displayName: person?.displayName || sourceKey,
    showToast,
    onOpenEpubForWorks,
    onLibraryChanged,
    load,
  });

  const refresh = async () => {
    setRefreshing(true);
    showToast("プロフィールと新作を確認中...", "info");
    try {
      const refreshToken = await store.get<string>("pixiv_refresh_token") || "";
      const cookie = await store.get<string>("fanbox_session_id") || "";
      const userAgent = await store.get<string>("fanbox_user_agent") || "Mozilla/5.0";
      await invoke("refresh_entity_profile", {
        params: { entityType: "person", source, sourceKey, force: true, refreshToken, cookie, userAgent },
      });

      let items: any[] = [];
      if (source === "pixiv") {
        if (!refreshToken) throw new Error("Pixivの認証情報がありません");
        items = await invoke<any[]>("fetch_pixiv_user_novels", { userId: sourceKey, refreshToken });
      } else if (source === "fanbox") {
        if (!cookie) throw new Error("FANBOXの認証情報がありません");
        items = await invoke<any[]>("fetch_fanbox_creator_posts", { creatorId: sourceKey, cookie, userAgent });
      }

      let newCount = 0;
      let updatedCount = 0;
      let skippedCount = 0;
      let failedCount = 0;
      for (const item of items) {
        try {
          const result = source === "pixiv"
            ? await savePixivNovel(item, refreshToken, person?.displayName || "", sourceKey)
            : await saveFanboxPost(item, cookie, userAgent, person?.displayName || "", sourceKey);
          const status = summarizeSave(result.wasExisting, result.saved);
          if (status === "new") newCount++;
          else if (status === "updated") updatedCount++;
          else skippedCount++;
          await refreshRelatedEntities(result.saved, refreshToken, cookie, userAgent);
        } catch {
          failedCount++;
        }
      }

      showToast(`プロフィール更新完了 / 新規 ${newCount} 件 / 更新 ${updatedCount} 件 / 既存 ${skippedCount} 件 / 失敗 ${failedCount} 件`, failedCount > 0 ? "error" : "success");
      onLibraryChanged();
      await load();
    } catch (e) {
      showToast(`更新チェックに失敗しました: ${e}`, "error");
    } finally {
      setRefreshing(false);
    }
  };

  const totalNovels = profileStat(profileJson, "totalNovels");
  const totalNovelSeries = profileStat(profileJson, "totalNovelSeries");
  const profileItems = Array.isArray(profileJson?.profileItems) ? profileJson.profileItems.length : null;

  return (
    <div className="entity-view" ref={layout.ref} style={layout.style}>
      <EntityActionBar
        works={works}
        selectedCount={tools.selectedIds.size}
        deleteMode={tools.deleteMode}
        epubMode={tools.epubMode}
        allSelected={tools.allSelected}
        busy={tools.busy}
        refreshing={refreshing}
        epubSidebarOpen={epubSidebarOpen}
        onRefresh={refresh}
        onOpenEpub={tools.openEpub}
        onEnterDelete={tools.enterDelete}
        onConfirmDelete={tools.confirmDelete}
        onCancelDelete={tools.cancelSelection}
        onSelectAll={tools.selectAll}
        onSelectNone={tools.selectNone}
        onBackup={tools.createBackup}
      />
      <section className="entity-hero">
        {cover && <img className="entity-cover" src={cover} alt="" />}
        <div className="entity-hero-content">
          <span className="entity-avatar">{icon ? <img src={icon} alt="" /> : <ImageIcon />}</span>
          <div className="entity-heading">
            <div className="entity-title-row">
              <span className={`source-tag ${source}`}>{source === "pixiv" ? "Pixiv" : "FANBOX"}</span>
              <h2>{person?.displayName || sourceKey}</h2>
            </div>
            <p>{person?.description || (loading ? "読み込み中..." : "保存済み作品から作成された人物ページです。")}</p>
            <div className="entity-meta entity-stat-row">
              <span>{person?.workCount ?? works.length} 保存済み</span>
              {totalNovels !== null && <span>{totalNovels.toLocaleString()} 公開小説</span>}
              {totalNovelSeries !== null && <span>{totalNovelSeries.toLocaleString()} シリーズ</span>}
              {profileItems !== null && <span>{profileItems.toLocaleString()} プロフィール項目</span>}
              <span>確認: {formatDate(person?.lastCheckedAt)}</span>
              <span>{versions.length} 履歴</span>
            </div>
          </div>
        </div>
        {links.length > 0 && (
          <div className="entity-links entity-icon-links">
            {links.slice(0, 8).map((link) => (
              <EntityLinkIconButton key={link} link={link} onOpenSourceUrl={onOpenSourceUrl} />
            ))}
          </div>
        )}
      </section>
      <div className="entity-section-title">
        <h3>保存済み作品</h3>
        <span>{works.length} 件</span>
      </div>
      <EntityWorkGrid works={works} selectedIds={tools.selectedIds} deleteMode={tools.deleteMode} epubMode={tools.epubMode} onToggleSelect={tools.toggleSelect} onViewDetail={onViewDetail} onViewSeries={onViewSeries} />
      {!loading && works.length === 0 && (
        <div className="entity-empty">この人物に紐づく保存済み作品はまだありません。</div>
      )}
    </div>
  );
}

export function SeriesView({ source, sourceKey, showToast, onViewDetail, onViewSeries, onOpenSourceUrl, onOpenEpubForWorks, onLibraryChanged, epubSidebarOpen }: Props) {
  const [series, setSeries] = useState<SeriesEntry | null>(null);
  const [works, setWorks] = useState<DownloadEntry[]>([]);
  const [versions, setVersions] = useState<EntityVersion[]>([]);
  const [profileJson, setProfileJson] = useState<any>(null);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [lastCheckSummary, setLastCheckSummary] = useState<string | null>(null);
  const cover = useImage(series?.coverPath);
  const layout = useEntityLayoutStyle();
  const sourceLink = defaultSourceLink(source, sourceKey, "series");

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [seriesData, workData, versionData, latestJson] = await Promise.all([
        invoke<SeriesEntry>("db_get_series", { source, sourceKey }),
        invoke<DownloadEntry[]>("db_search_downloads", {
          params: { limit: 500, offset: 0, sortBy: "series_order", sortOrder: "asc", seriesSource: source, seriesKey: sourceKey },
        }),
        invoke<EntityVersion[]>("db_list_entity_versions", { entityType: "series", source, sourceKey }),
        invoke<any | null>("db_get_latest_entity_profile_json", { entityType: "series", source, sourceKey }).catch(() => null),
      ]);
      setSeries(seriesData);
      setWorks(workData);
      setVersions(versionData);
      setProfileJson(latestJson);
    } catch (e) {
      showToast(`シリーズ情報を読み込めませんでした: ${e}`, "error");
    } finally {
      setLoading(false);
    }
  }, [source, sourceKey, showToast]);

  useEffect(() => {
    document.querySelector(".library-main-content")?.scrollTo({ top: 0 });
    load();
  }, [load]);

  const tools = useEntityTools({
    works,
    entityType: "series",
    source,
    sourceKey,
    displayName: series?.title || profileJson?.title || sourceKey,
    showToast,
    onOpenEpubForWorks,
    onLibraryChanged,
    load,
  });

  const refresh = async () => {
    setRefreshing(true);
    showToast("シリーズ情報と新作を確認中...", "info");
    try {
      const refreshToken = await store.get<string>("pixiv_refresh_token") || "";
      const cookie = await store.get<string>("fanbox_session_id") || "";
      const userAgent = await store.get<string>("fanbox_user_agent") || "Mozilla/5.0";
      await invoke("refresh_entity_profile", {
        params: { entityType: "series", source, sourceKey, force: true, refreshToken, cookie, userAgent },
      });

      let newCount = 0;
      let updatedCount = 0;
      let skippedCount = 0;
      let failedCount = 0;
      if (source === "pixiv") {
        if (!refreshToken) throw new Error("Pixivの認証情報がありません");
        const items = await invoke<any[]>("fetch_pixiv_series_novels", { seriesId: sourceKey, refreshToken });
        for (const item of items) {
          try {
            const result = await savePixivNovel(item, refreshToken, "", "");
            const seriesInfo = normalizePixivSeries(item);
            if (seriesInfo && seriesInfo.id !== sourceKey) skippedCount++;
            const status = summarizeSave(result.wasExisting, result.saved);
            if (status === "new") newCount++;
            else if (status === "updated") updatedCount++;
            else skippedCount++;
            await refreshRelatedEntities(result.saved, refreshToken, cookie, userAgent);
          } catch {
            failedCount++;
          }
        }
      } else {
        skippedCount = works.length;
      }

      const summary = `新規 ${newCount} 件 / 更新 ${updatedCount} 件 / 既存 ${skippedCount} 件 / 失敗 ${failedCount} 件`;
      setLastCheckSummary(summary);
      showToast(`シリーズ更新チェック完了 / ${summary}`, failedCount > 0 ? "error" : "success");
      onLibraryChanged();
      await load();
    } catch (e) {
      showToast(`シリーズ更新チェックに失敗しました: ${e}`, "error");
    } finally {
      setRefreshing(false);
    }
  };

  return (
    <div className="entity-view" ref={layout.ref} style={layout.style}>
      <EntityActionBar
        works={works}
        selectedCount={tools.selectedIds.size}
        deleteMode={tools.deleteMode}
        epubMode={tools.epubMode}
        allSelected={tools.allSelected}
        busy={tools.busy}
        refreshing={refreshing}
        epubSidebarOpen={epubSidebarOpen}
        onRefresh={refresh}
        onOpenEpub={tools.openEpub}
        onEnterDelete={tools.enterDelete}
        onConfirmDelete={tools.confirmDelete}
        onCancelDelete={tools.cancelSelection}
        onSelectAll={tools.selectAll}
        onSelectNone={tools.selectNone}
        onBackup={tools.createBackup}
      />
      <section className="entity-hero series-entity-hero">
        {cover && <img className="entity-cover" src={cover} alt="" />}
        <div className="entity-hero-content">
          <div className="entity-heading">
            <div className="entity-title-row">
              <span className={`source-tag ${source}`}>{source === "pixiv" ? "Pixiv" : "FANBOX"}</span>
              <h2>{series?.title || profileJson?.title || (loading ? "読み込み中..." : sourceKey)}</h2>
            </div>
            <p>{series?.description || profileJson?.description || "保存済み作品から作成されたシリーズページです。"}</p>
            <div className="entity-meta entity-stat-row">
              <span>{series?.workCount ?? works.length} 保存済み</span>
              <span>v{series?.currentVersion ?? 0}</span>
              <span>確認: {formatDate(series?.lastCheckedAt)}</span>
              <span>{versions.length} 履歴</span>
              {lastCheckSummary && <span>{lastCheckSummary}</span>}
            </div>
          </div>
        </div>
        {sourceLink && (
          <div className="entity-links entity-icon-links">
            <EntityLinkIconButton link={sourceLink} onOpenSourceUrl={onOpenSourceUrl} />
          </div>
        )}
      </section>
      <div className="entity-section-title">
        <h3>シリーズ内の保存済み作品</h3>
        <span>{works.length} 件</span>
      </div>
      <EntityWorkGrid works={works} selectedIds={tools.selectedIds} deleteMode={tools.deleteMode} epubMode={tools.epubMode} onToggleSelect={tools.toggleSelect} onViewDetail={onViewDetail} onViewSeries={onViewSeries} />
      {!loading && works.length === 0 && (
        <div className="entity-empty">このシリーズに紐づく保存済み作品はまだありません。</div>
      )}
    </div>
  );
}
