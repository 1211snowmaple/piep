import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { store } from "../../store";
import { DownloadIcon, PanelRightIcon, RefreshIcon } from "../icons/Icons";

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

type TargetType = "work" | "author" | "series";
type SourceType = "pixiv" | "fanbox";

interface DownloadEntry {
  id: number;
  source: SourceType;
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

interface UpdateTarget {
  id: number;
  targetType: TargetType;
  source: SourceType;
  sourceKey: string;
  displayName: string;
  enabled: boolean;
  lastCheckedAt: string | null;
  lastSeenSourceId: string | null;
  lastSeenSourceUpdatedAt: string | null;
  metadataJson: string | null;
}

interface DownloadRelation {
  relationType: "author" | "series";
  source: SourceType;
  relationId: string;
  relationName: string;
  workCount: number | null;
}

interface Candidate {
  key: string;
  source: SourceType;
  sourceId: string;
  title: string;
  subtitle: string;
  targetLabel: string;
  targetType: TargetType;
  originalData: any;
  selected: boolean;
  status?: "pending" | "saving" | "saved" | "failed" | "skipped";
}

interface CheckLog {
  id: number;
  type: "info" | "success" | "warn" | "error";
  text: string;
}

interface Props {
  isOpen: boolean;
  showToast: (text: string, type: "success" | "error" | "info") => void;
  onClose: () => void;
  refreshKey?: number;
  onLibraryChanged?: () => void;
}

const delay = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));

function labelForType(type: TargetType): string {
  if (type === "work") return "作品";
  if (type === "author") return "著者";
  return "シリーズ";
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

export function UpdateSidebar({ isOpen, showToast, onClose, refreshKey = 0, onLibraryChanged }: Props) {
  const tauriAvailable = isTauriRuntime();
  const [activeTab, setActiveTab] = useState<TargetType | "results">("work");
  const [targets, setTargets] = useState<UpdateTarget[]>([]);
  const [watchedWorks, setWatchedWorks] = useState<DownloadEntry[]>([]);
  const [relations, setRelations] = useState<DownloadRelation[]>([]);
  const [candidates, setCandidates] = useState<Candidate[]>([]);
  const [logs, setLogs] = useState<CheckLog[]>([]);
  const [busy, setBusy] = useState(false);

  const addLog = useCallback((type: CheckLog["type"], text: string) => {
    setLogs(prev => [...prev.slice(-199), { id: Date.now() + Math.random(), type, text }]);
  }, []);

  const loadData = useCallback(async () => {
    if (!tauriAvailable) return;
    const [nextTargets, nextWorks, nextRelations] = await Promise.all([
      invoke<UpdateTarget[]>("db_list_update_targets", { targetType: null, enabledOnly: false }),
      invoke<DownloadEntry[]>("db_get_watched_downloads"),
      invoke<DownloadRelation[]>("db_list_download_relations", { relationType: null }),
    ]);
    setTargets(nextTargets);
    setWatchedWorks(nextWorks);
    setRelations(nextRelations);
  }, [tauriAvailable]);

  useEffect(() => {
    loadData().catch(e => {
      console.error(e);
      showToast(`更新管理の読み込みに失敗しました: ${e}`, "error");
    });
  }, [loadData, refreshKey, showToast]);

  const enabledTargets = useMemo(() => targets.filter(t => t.enabled), [targets]);
  const authorTargets = useMemo(() => targets.filter(t => t.targetType === "author"), [targets]);
  const seriesTargets = useMemo(() => targets.filter(t => t.targetType === "series"), [targets]);

  const candidateRelations = useMemo(() => {
    const existing = new Set(targets.map(t => `${t.targetType}:${t.source}:${t.sourceKey}`));
    return relations.filter(rel => !existing.has(`${rel.relationType}:${rel.source}:${rel.relationId}`));
  }, [relations, targets]);

  const upsertTarget = async (targetType: TargetType, source: SourceType, sourceKey: string, displayName: string, enabled = true, metadata: any = null) => {
    setBusy(true);
    try {
      await invoke("db_upsert_update_target", {
        target: {
          targetType,
          source,
          sourceKey,
          displayName,
          enabled,
          metadataJson: metadata ? JSON.stringify(metadata) : null,
        },
      });
      await loadData();
      showToast(`${displayName} を更新監視に追加しました`, "success");
    } catch (e: any) {
      showToast(`更新監視への追加に失敗しました: ${e}`, "error");
    } finally {
      setBusy(false);
    }
  };

  const setTargetEnabled = async (target: UpdateTarget, enabled: boolean) => {
    setBusy(true);
    try {
      await invoke("db_set_update_target_enabled", {
        targetType: target.targetType,
        source: target.source,
        sourceKey: target.sourceKey,
        enabled,
      });
      await loadData();
    } catch (e: any) {
      showToast(`更新監視の切り替えに失敗しました: ${e}`, "error");
    } finally {
      setBusy(false);
    }
  };

  const deleteTarget = async (target: UpdateTarget) => {
    setBusy(true);
    try {
      await invoke("db_delete_update_target", {
        targetType: target.targetType,
        source: target.source,
        sourceKey: target.sourceKey,
      });
      await loadData();
      showToast(`${target.displayName} を更新監視から削除しました`, "success");
    } catch (e: any) {
      showToast(`更新監視の削除に失敗しました: ${e}`, "error");
    } finally {
      setBusy(false);
    }
  };

  const setAllCandidatesSelected = (selected: boolean) => {
    setCandidates(prev => prev.map(candidate => {
      if (candidate.status === "saved" || candidate.status === "saving") return candidate;
      return { ...candidate, selected };
    }));
  };

  const markChecked = async (target: Pick<UpdateTarget, "targetType" | "source" | "sourceKey">, lastSeenId?: string, lastSeenUpdated?: string | null) => {
    await invoke("db_mark_update_target_checked", {
      targetType: target.targetType,
      source: target.source,
      sourceKey: target.sourceKey,
      lastSeenSourceId: lastSeenId || null,
      lastSeenSourceUpdatedAt: lastSeenUpdated || null,
    });
  };

  const checkWorks = async () => {
    const refreshToken = await store.get<string>("pixiv_refresh_token") || "";
    const fanboxCookie = await store.get<string>("fanbox_session_id") || "";
    const fanboxUserAgent = await store.get<string>("fanbox_user_agent") || "Mozilla/5.0";
    let updatedCount = 0;
    let errorCount = 0;

    for (const dl of watchedWorks) {
      try {
        if (dl.source === "pixiv") {
          if (!refreshToken) {
            addLog("warn", "Pixiv未連携のためPixiv作品をスキップしました");
            continue;
          }
          const metadata: any = await invoke("fetch_pixiv_novel_metadata", { novelId: dl.sourceId, refreshToken });
          const metadataUpdatedAt = metadata.create_date || null;
          if (dl.sourceUpdatedAt === metadataUpdatedAt) {
            addLog("info", `最新: ${dl.title}`);
            continue;
          }
          const data: any = await invoke("fetch_pixiv_novel", { novelId: dl.sourceId, refreshToken });
          await invoke("download_and_save", {
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
          updatedCount++;
          addLog("success", `更新を保存: ${dl.title}`);
        } else {
          if (!fanboxCookie) {
            addLog("warn", "FANBOX未連携のためFANBOX作品をスキップしました");
            continue;
          }
          const post: any = await invoke("fetch_fanbox_post", { postId: dl.sourceId, cookie: fanboxCookie, userAgent: fanboxUserAgent });
          const postUpdatedAt = post.updatedDatetime || post.updated_datetime || null;
          if (dl.sourceUpdatedAt === postUpdatedAt) {
            addLog("info", `最新: ${dl.title}`);
            continue;
          }
          await invoke("download_and_save", {
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
            cookie: fanboxCookie,
            userAgent: fanboxUserAgent,
          });
          updatedCount++;
          addLog("success", `更新を保存: ${dl.title}`);
        }
      } catch (e) {
        errorCount++;
        addLog("error", `${dl.title}: ${e}`);
      }
      await delay(1200);
    }
    return { updatedCount, errorCount };
  };

  const checkCollectionTargets = async (targetType: "author" | "series") => {
    const refreshToken = await store.get<string>("pixiv_refresh_token") || "";
    const fanboxCookie = await store.get<string>("fanbox_session_id") || "";
    const fanboxUserAgent = await store.get<string>("fanbox_user_agent") || "Mozilla/5.0";
    const sourceTargets = enabledTargets.filter(t => t.targetType === targetType);
    const found: Candidate[] = [];
    let errorCount = 0;

    for (const target of sourceTargets) {
      try {
        addLog("info", `${labelForType(targetType)}を確認中: ${target.displayName}`);
        let items: any[] = [];
        if (target.source === "pixiv" && targetType === "author") {
          if (!refreshToken) continue;
          items = await invoke<any[]>("fetch_pixiv_user_novels", { userId: target.sourceKey, refreshToken });
        } else if (target.source === "pixiv" && targetType === "series") {
          if (!refreshToken) continue;
          items = await invoke<any[]>("fetch_pixiv_series_novels", { seriesId: target.sourceKey, refreshToken });
        } else if (target.source === "fanbox" && targetType === "author") {
          if (!fanboxCookie) continue;
          items = await invoke<any[]>("fetch_fanbox_creator_posts", { creatorId: target.sourceKey, cookie: fanboxCookie, userAgent: fanboxUserAgent });
        }

        for (const item of items) {
          const sourceId = target.source === "pixiv" ? normalizePixivNovelId(item) : String(item.id || "");
          if (!sourceId) continue;
          const existing = await invoke<DownloadEntry | null>("db_get_download_by_source", { source: target.source, sourceId });
          if (existing) continue;
          found.push({
            key: `${target.targetType}:${target.source}:${target.sourceKey}:${sourceId}`,
            source: target.source,
            sourceId,
            title: item.title || "無題",
            subtitle: target.source === "pixiv"
              ? `${item.user?.name || target.displayName} / ${item.create_date || item.createDate || ""}`
              : `${item.user?.name || target.displayName} / ${item.publishedDatetime || item.published_datetime || ""}`,
            targetLabel: target.displayName,
            targetType,
            originalData: item,
            selected: true,
          });
        }

        const first = items[0];
        const firstId = target.source === "pixiv" ? normalizePixivNovelId(first || {}) : String(first?.id || "");
        const firstUpdated = target.source === "pixiv"
          ? first?.create_date || first?.createDate || null
          : first?.updatedDatetime || first?.updated_datetime || null;
        await markChecked(target, firstId, firstUpdated);
      } catch (e) {
        errorCount++;
        addLog("error", `${target.displayName}: ${e}`);
      }
      await delay(1200);
    }

    setCandidates(prev => {
      const seen = new Set(prev.map(c => c.key));
      return [...prev, ...found.filter(c => !seen.has(c.key))];
    });
    if (found.length > 0) addLog("success", `${found.length}件の新作候補を検出しました`);
    return { foundCount: found.length, errorCount };
  };

  const runCheck = async (scope: "all" | TargetType) => {
    setBusy(true);
    setCandidates([]);
    try {
      let updatedCount = 0;
      let foundCount = 0;
      let errorCount = 0;
      if (scope === "all" || scope === "work") {
        const result = await checkWorks();
        updatedCount += result.updatedCount;
        errorCount += result.errorCount;
      }
      if (scope === "all" || scope === "author") {
        const result = await checkCollectionTargets("author");
        foundCount += result.foundCount;
        errorCount += result.errorCount;
      }
      if (scope === "all" || scope === "series") {
        const result = await checkCollectionTargets("series");
        foundCount += result.foundCount;
        errorCount += result.errorCount;
      }
      await loadData();
      onLibraryChanged?.();
      showToast(`更新チェック完了: 更新 ${updatedCount} 件 / 新作候補 ${foundCount} 件 / エラー ${errorCount} 件`, errorCount ? "error" : "success");
      setActiveTab("results");
    } catch (e: any) {
      showToast(`更新チェックエラー: ${e}`, "error");
    } finally {
      setBusy(false);
    }
  };

  const saveCandidates = async () => {
    const selected = candidates.filter(c => c.selected);
    if (selected.length === 0) return;
    setBusy(true);
    const refreshToken = await store.get<string>("pixiv_refresh_token") || "";
    const fanboxCookie = await store.get<string>("fanbox_session_id") || "";
    const fanboxUserAgent = await store.get<string>("fanbox_user_agent") || "Mozilla/5.0";
    let saved = 0;
    let failed = 0;
    const entityRefreshQueue = new Map<string, { entityType: "person" | "series"; source: SourceType; sourceKey: string }>();
    const enqueueEntityRefresh = (entry: DownloadEntry) => {
      const personKey = entry.personId || entry.authorId;
      if (personKey) {
        entityRefreshQueue.set(`person:${entry.source}:${personKey}`, {
          entityType: "person",
          source: entry.source,
          sourceKey: personKey,
        });
      }
      if (entry.seriesId) {
        entityRefreshQueue.set(`series:${entry.source}:${entry.seriesId}`, {
          entityType: "series",
          source: entry.source,
          sourceKey: entry.seriesId,
        });
      }
    };
    try {
      for (const candidate of selected) {
        setCandidates(prev => prev.map(c => c.key === candidate.key ? { ...c, status: "saving" } : c));
        try {
          const existing = await invoke<DownloadEntry | null>("db_get_download_by_source", { source: candidate.source, sourceId: candidate.sourceId });
          if (existing) {
            setCandidates(prev => prev.map(c => c.key === candidate.key ? { ...c, status: "skipped" } : c));
            continue;
          }
          if (candidate.source === "pixiv") {
            const data: any = await invoke("fetch_pixiv_novel", { novelId: candidate.sourceId, refreshToken });
            const savedEntry = await invoke<DownloadEntry>("download_and_save", {
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
            enqueueEntityRefresh(savedEntry);
            const series = normalizePixivSeries(candidate.originalData);
            if (series) {
              await upsertTarget("series", "pixiv", series.id, series.title, true);
            }
          } else {
            const post: any = await invoke("fetch_fanbox_post", { postId: candidate.sourceId, cookie: fanboxCookie, userAgent: fanboxUserAgent });
            const savedEntry = await invoke<DownloadEntry>("download_and_save", {
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
              cookie: fanboxCookie,
              userAgent: fanboxUserAgent,
            });
            enqueueEntityRefresh(savedEntry);
          }
          saved++;
          setCandidates(prev => prev.map(c => c.key === candidate.key ? { ...c, status: "saved" } : c));
        } catch (e) {
          failed++;
          addLog("error", `${candidate.title}: ${e}`);
          setCandidates(prev => prev.map(c => c.key === candidate.key ? { ...c, status: "failed" } : c));
        }
        await delay(1000);
      }
      if (entityRefreshQueue.size > 0) {
        addLog("info", `作者・シリーズ情報を確認中... (${entityRefreshQueue.size}件)`);
        for (const target of entityRefreshQueue.values()) {
          try {
            await invoke("refresh_entity_profile", {
              params: {
                entityType: target.entityType,
                source: target.source,
                sourceKey: target.sourceKey,
                force: false,
                refreshToken,
                cookie: fanboxCookie,
                userAgent: fanboxUserAgent,
              },
            });
          } catch (e) {
            addLog("warn", `${target.entityType}:${target.source}:${target.sourceKey} の確認をスキップ: ${e}`);
          }
        }
      }
      onLibraryChanged?.();
      showToast(`${saved}件を保存しました${failed ? ` (${failed}件失敗)` : ""}`, failed ? "error" : "success");
    } finally {
      setBusy(false);
    }
  };

  const renderTargetList = (type: "author" | "series") => {
    const list = type === "author" ? authorTargets : seriesTargets;
    const candidatesForType = candidateRelations.filter(r => r.relationType === type);
    return (
      <>
        <div className="update-side-section">
          <div className="update-side-section-title">監視中</div>
          {list.length === 0 ? <div className="update-empty-row">まだ監視対象がありません</div> : list.map(target => (
            <div key={target.id} className={`update-target-row ${target.enabled ? "" : "disabled"}`}>
              <div className="update-target-info">
                <strong>{target.displayName}</strong>
                <span>{target.source.toUpperCase()} / {target.sourceKey}</span>
              </div>
              <div className="update-target-actions">
                <button className="update-text-action" onClick={() => setTargetEnabled(target, !target.enabled)} disabled={busy} title={target.enabled ? "一時停止" : "再開"}>
                  {target.enabled ? "停止" : "再開"}
                </button>
                <button className="update-text-action danger" onClick={() => deleteTarget(target)} disabled={busy} title="更新監視から削除">
                  削除
                </button>
              </div>
            </div>
          ))}
        </div>
        <div className="update-side-section">
          <div className="update-side-section-title">ライブラリから追加</div>
          {candidatesForType.length === 0 ? <div className="update-empty-row">追加候補はありません</div> : candidatesForType.map(rel => (
            <div key={`${rel.relationType}:${rel.source}:${rel.relationId}`} className="update-target-row">
              <div className="update-target-info">
                <strong>{rel.relationName}</strong>
                <span>{rel.source.toUpperCase()} / {rel.workCount || 0} 作品</span>
              </div>
              <button className="update-text-action primary" onClick={() => upsertTarget(rel.relationType, rel.source, rel.relationId, rel.relationName, true)} disabled={busy}>
                追加
              </button>
            </div>
          ))}
        </div>
      </>
    );
  };

  return (
    <aside className={`epub-sidebar update-sidebar ${isOpen ? "open" : ""}`}>
      <div className="epub-sidebar-header">
        <div className="epub-sidebar-title">
          <RefreshIcon />
          <span>更新管理</span>
        </div>
        <button className="epub-sidebar-close-btn" onClick={onClose} title="サイドバーを閉じる">
          <PanelRightIcon />
        </button>
      </div>

      <div className="epub-sidebar-body">
        <div className="update-tabs">
          {(["work", "author", "series", "results"] as const).map(tab => (
            <button key={tab} className={activeTab === tab ? "active" : ""} onClick={() => setActiveTab(tab)}>
              {tab === "results" ? "結果" : labelForType(tab)}
            </button>
          ))}
        </div>

        <div className="update-action-grid">
          <button className="epub-export-action-btn" onClick={() => runCheck("all")} disabled={busy}>
            {busy ? "確認中..." : "すべてチェック"}
          </button>
          <button className="toolbar-btn" onClick={() => runCheck(activeTab === "results" ? "all" : activeTab)} disabled={busy}>
            <RefreshIcon /> 表示中だけ
          </button>
        </div>

        {activeTab === "work" && (
          <div className="update-side-section">
            <div className="update-side-section-title">個別作品</div>
            {watchedWorks.length === 0 ? <div className="update-empty-row">カードをクリックして作品の更新監視をONにできます</div> : watchedWorks.map(work => (
              <div key={work.id} className="update-target-row">
                <div className="update-target-info">
                  <strong>{work.title}</strong>
                  <span>{work.source.toUpperCase()} / {work.authorName}</span>
                </div>
                <span className="update-status-pill">監視中</span>
              </div>
            ))}
          </div>
        )}

        {activeTab === "author" && renderTargetList("author")}
        {activeTab === "series" && renderTargetList("series")}

        {activeTab === "results" && (
          <>
            <div className="update-side-section">
              <div className="update-side-section-title update-section-title-row">
                <span>新作候補</span>
                {candidates.length > 0 && (
                  <span className="update-inline-actions">
                    <button type="button" onClick={() => setAllCandidatesSelected(true)} disabled={busy}>全選択</button>
                    <button type="button" onClick={() => setAllCandidatesSelected(false)} disabled={busy}>全解除</button>
                  </span>
                )}
              </div>
              {candidates.length === 0 ? <div className="update-empty-row">チェック後に候補が表示されます</div> : candidates.map(candidate => (
                <label key={candidate.key} className={`update-candidate-row ${candidate.status || ""}`}>
                  <input
                    type="checkbox"
                    checked={candidate.selected}
                    disabled={busy || candidate.status === "saved" || candidate.status === "saving"}
                    onChange={e => setCandidates(prev => prev.map(c => c.key === candidate.key ? { ...c, selected: e.target.checked } : c))}
                  />
                  <div className="update-target-info">
                    <strong>{candidate.title}</strong>
                    <span>{candidate.targetLabel} / {candidate.subtitle}</span>
                  </div>
                </label>
              ))}
              {candidates.length > 0 && (
                <button className="epub-export-action-btn" onClick={saveCandidates} disabled={busy || candidates.every(c => !c.selected)}>
                  <DownloadIcon /> 選択候補を保存
                </button>
              )}
            </div>
            <div className="epub-log-panel">
              <div className="epub-log-header">
                <span>チェックログ</span>
                <button className="epub-log-clear" onClick={() => setLogs([])}>クリア</button>
              </div>
              <div className="epub-log-entries">
                {logs.length === 0 ? <div className="epub-log-empty">ログはありません</div> : logs.map(log => (
                  <div key={log.id} className={`epub-log-entry ${log.type}`}>
                    <span className="epub-log-text">{log.text}</span>
                  </div>
                ))}
              </div>
            </div>
          </>
        )}
      </div>
    </aside>
  );
}
