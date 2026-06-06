import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { DownloadIcon, PanelRightIcon, RefreshIcon } from "../icons/Icons";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import { Progress } from "@/components/ui/progress";
import { Skeleton } from "@/components/ui/skeleton";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  labelForType,
  sourceLabel,
  upsertUpdateTarget,
  type UpdateDownloadEntry as DownloadEntry,
  type UpdateSource as SourceType,
  type UpdateTarget,
  type UpdateTargetType as TargetType,
} from "@/features/updates/updateWorkflow";
import {
  isUpdateJobTerminal,
  useUpdateJobs,
  type UpdateJobCandidate,
  type UpdateJobLog,
  type UpdateJobMode,
  type UpdateJobStatus,
} from "@/features/updates/updateJobs";
import { cn } from "@/lib/utils";
import {
  deleteUpdateTarget,
  getWatchedDownloads,
  listDownloadRelations,
  listUpdateTargets,
  setUpdateTargetEnabled,
} from "@/services/dbApi";

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

interface DownloadRelation {
  relationType: "author" | "series";
  source: SourceType;
  relationId: string;
  relationName: string;
  workCount: number | null;
}

interface Props {
  isOpen: boolean;
  showToast: (text: string, type: "success" | "error" | "info") => void;
  onClose: () => void;
  refreshKey?: number;
  onLibraryChanged?: () => void;
}

function candidateStatusLabel(status: UpdateJobCandidate["status"]): string {
  if (status === "queued") return "保存待ち";
  if (status === "running") return "保存中";
  if (status === "saved") return "保存済み";
  if (status === "failed") return "失敗";
  if (status === "skipped") return "スキップ";
  if (status === "done") return "完了";
  return "候補";
}

function candidateStatusVariant(status: UpdateJobCandidate["status"]): "default" | "secondary" | "outline" | "destructive" {
  if (status === "failed") return "destructive";
  if (status === "saved") return "default";
  if (status === "queued" || status === "running") return "secondary";
  return "outline";
}

function jobStatusLabel(status: UpdateJobStatus): string {
  if (status === "queued") return "待機中";
  if (status === "running") return "実行中";
  if (status === "paused") return "一時停止";
  if (status === "auth_required") return "認証待ち";
  if (status === "canceling") return "停止中";
  if (status === "canceled") return "キャンセル";
  if (status === "failed") return "失敗";
  return "完了";
}

function logBadgeVariant(type: UpdateJobLog["logType"]): "default" | "outline" | "destructive" {
  if (type === "error") return "destructive";
  if (type === "success") return "default";
  return "outline";
}

export function UpdateSidebar({ isOpen, showToast, onClose, refreshKey = 0, onLibraryChanged }: Props) {
  const tauriAvailable = isTauriRuntime();
  const [activeTab, setActiveTab] = useState<TargetType | "results">("work");
  const [targets, setTargets] = useState<UpdateTarget[]>([]);
  const [watchedWorks, setWatchedWorks] = useState<DownloadEntry[]>([]);
  const [relations, setRelations] = useState<DownloadRelation[]>([]);
  const [selectedCandidateIds, setSelectedCandidateIds] = useState<Set<number>>(new Set());
  const [busy, setBusy] = useState(false);
  const announcedJobRef = useRef<string | null>(null);
  const candidateListRef = useRef<HTMLDivElement>(null);
  const logListRef = useRef<HTMLDivElement>(null);
  const updateJobs = useUpdateJobs();
  const loadJobs = updateJobs.loadJobs;

  const activeJob = updateJobs.activeSnapshot;
  const candidates = activeJob?.candidates ?? [];
  const logs = activeJob?.logs ?? [];
  const jobBusy = busy || !!activeJob && ["queued", "running", "canceling"].includes(activeJob.status);
  const progress = activeJob && activeJob.totals > 0
    ? Math.min(100, Math.max(activeJob.status === "running" ? 8 : 0, Math.round((activeJob.processed / activeJob.totals) * 100)))
    : jobBusy ? 12 : 0;

  const loadData = useCallback(async () => {
    if (!tauriAvailable) return;
    const [nextTargets, nextWorks, nextRelations] = await Promise.all([
      listUpdateTargets<UpdateTarget>(null, false),
      getWatchedDownloads<DownloadEntry>(),
      listDownloadRelations<DownloadRelation>(null),
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
    loadJobs().catch(() => undefined);
  }, [loadData, refreshKey, showToast, loadJobs]);

  useEffect(() => {
    setSelectedCandidateIds(prev => {
      const next = new Set(prev);
      for (const candidate of candidates) {
        if (candidate.status === "candidate" || candidate.status === "failed") {
          next.add(candidate.id);
        }
      }
      return next;
    });
  }, [candidates]);

  useEffect(() => {
    if (!activeJob || !isUpdateJobTerminal(activeJob.status)) return;
    const announcementKey = `${activeJob.jobId}:${activeJob.status}:${activeJob.savedCount}:${activeJob.errorCount}`;
    if (announcedJobRef.current === announcementKey) return;
    announcedJobRef.current = announcementKey;
    loadData().catch(() => undefined);
    onLibraryChanged?.();
    if (activeJob.status === "completed") {
      showToast(`更新ジョブ完了: 保存 ${activeJob.savedCount} 件 / 候補 ${activeJob.candidateCount} 件`, "success");
    } else if (activeJob.status === "failed") {
      showToast(`更新ジョブで ${activeJob.errorCount} 件のエラーが発生しました`, "error");
    }
  }, [activeJob, loadData, onLibraryChanged, showToast]);

  const authorTargets = useMemo(() => targets.filter(t => t.targetType === "author"), [targets]);
  const seriesTargets = useMemo(() => targets.filter(t => t.targetType === "series"), [targets]);
  const selectedCandidateCount = useMemo(
    () => candidates.filter(candidate => selectedCandidateIds.has(candidate.id) && candidate.status !== "saved" && candidate.status !== "running").length,
    [candidates, selectedCandidateIds],
  );

  const candidateRelations = useMemo(() => {
    const existing = new Set(targets.map(t => `${t.targetType}:${t.source}:${t.sourceKey}`));
    return relations.filter(rel => !existing.has(`${rel.relationType}:${rel.source}:${rel.relationId}`));
  }, [relations, targets]);

  const candidateVirtualizer = useVirtualizer({
    count: candidates.length,
    getScrollElement: () => candidateListRef.current,
    estimateSize: () => 76,
    overscan: 8,
  });
  const logVirtualizer = useVirtualizer({
    count: logs.length,
    getScrollElement: () => logListRef.current,
    estimateSize: () => 42,
    overscan: 12,
  });
  const candidateVirtualItems = candidateVirtualizer.getVirtualItems();
  const logVirtualItems = logVirtualizer.getVirtualItems();

  const runCheck = async (scope: "all" | TargetType, mode: UpdateJobMode = "check_only") => {
    setBusy(true);
    try {
      const snapshot = await updateJobs.start({ scope, mode });
      setActiveTab("results");
      showToast(`更新ジョブを開始しました: ${jobStatusLabel(snapshot.status)}`, "info");
      await updateJobs.loadJobs();
    } catch (e: any) {
      showToast(`更新ジョブの開始に失敗しました: ${e}`, "error");
    } finally {
      setBusy(false);
    }
  };

  const upsertTarget = async (targetType: TargetType, source: SourceType, sourceKey: string, displayName: string, enabled = true, metadata: any = null) => {
    setBusy(true);
    try {
      await upsertUpdateTarget(targetType, source, sourceKey, displayName, enabled, metadata);
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
      await setUpdateTargetEnabled(target.targetType, target.source, target.sourceKey, enabled);
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
      await deleteUpdateTarget(target.targetType, target.source, target.sourceKey);
      await loadData();
      showToast(`${target.displayName} を更新監視から削除しました`, "success");
    } catch (e: any) {
      showToast(`更新監視の削除に失敗しました: ${e}`, "error");
    } finally {
      setBusy(false);
    }
  };

  const setAllCandidatesSelected = (selected: boolean) => {
    setSelectedCandidateIds(() => {
      if (!selected) return new Set();
      return new Set(candidates.filter(candidate => candidate.status !== "saved" && candidate.status !== "running").map(candidate => candidate.id));
    });
  };

  const saveCandidates = async () => {
    if (!activeJob || selectedCandidateCount === 0) return;
    setBusy(true);
    try {
      const ids = candidates
        .filter(candidate => selectedCandidateIds.has(candidate.id) && candidate.status !== "saved" && candidate.status !== "running")
        .map(candidate => candidate.id);
      await updateJobs.saveCandidates(activeJob.jobId, ids);
      showToast(`${ids.length}件の候補を保存キューに追加しました`, "success");
      await updateJobs.loadJobs();
    } catch (e: any) {
      showToast(`候補保存の開始に失敗しました: ${e}`, "error");
    } finally {
      setBusy(false);
    }
  };

  const controlJob = async (action: "pause" | "resume" | "cancel") => {
    if (!activeJob) return;
    setBusy(true);
    try {
      if (action === "pause") await updateJobs.pause(activeJob.jobId);
      if (action === "resume") await updateJobs.resume(activeJob.jobId, activeJob.status === "failed");
      if (action === "cancel") await updateJobs.cancel(activeJob.jobId);
      await updateJobs.loadJobs();
    } catch (e: any) {
      showToast(`ジョブ操作に失敗しました: ${e}`, "error");
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
            <Card key={target.id} className={cn("update-target-row", target.enabled ? "" : "disabled")}>
              <div className="update-target-info">
                <strong>{target.displayName}</strong>
                <span>{sourceLabel(target.source)} / {target.sourceKey}</span>
              </div>
              <div className="update-target-actions">
                <Badge variant={target.enabled ? "default" : "outline"}>{target.enabled ? "ON" : "OFF"}</Badge>
                <Button type="button" variant="outline" size="sm" className="update-text-action" onClick={() => setTargetEnabled(target, !target.enabled)} disabled={jobBusy} title={target.enabled ? "一時停止" : "再開"}>
                  {target.enabled ? "停止" : "再開"}
                </Button>
                <Button type="button" variant="destructive" size="sm" className="update-text-action danger" onClick={() => deleteTarget(target)} disabled={jobBusy} title="更新監視から削除">
                  削除
                </Button>
              </div>
            </Card>
          ))}
        </div>
        <div className="update-side-section">
          <div className="update-side-section-title">ライブラリから追加</div>
          {candidatesForType.length === 0 ? <div className="update-empty-row">追加候補はありません</div> : candidatesForType.map(rel => (
            <Card key={`${rel.relationType}:${rel.source}:${rel.relationId}`} className="update-target-row">
              <div className="update-target-info">
                <strong>{rel.relationName}</strong>
                <span>{sourceLabel(rel.source)} / {rel.workCount || 0} 作品</span>
              </div>
              <Button type="button" size="sm" className="update-text-action" onClick={() => upsertTarget(rel.relationType, rel.source, rel.relationId, rel.relationName, true)} disabled={jobBusy}>
                追加
              </Button>
            </Card>
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
        <Button type="button" variant="ghost" size="icon" className="epub-sidebar-close-btn" onClick={onClose} title="サイドバーを閉じる">
          <PanelRightIcon />
        </Button>
      </div>

      <div className="epub-sidebar-body">
        <Tabs value={activeTab} onValueChange={value => setActiveTab(value as TargetType | "results")}>
        <TabsList className="update-tabs grid w-full grid-cols-4">
          {(["work", "author", "series", "results"] as const).map(tab => {
            const count = tab === "work"
              ? watchedWorks.length
              : tab === "author"
                ? authorTargets.length
                : tab === "series"
                  ? seriesTargets.length
                  : candidates.length;
            return (
              <TabsTrigger
                key={tab}
                value={tab}
                className={cn(activeTab === tab && "active")}
              >
                {tab === "results" ? "結果" : labelForType(tab)}
                <Badge variant={activeTab === tab ? "default" : "outline"} className="ml-1 px-1.5">
                  {count}
                </Badge>
              </TabsTrigger>
            );
          })}
        </TabsList>
        </Tabs>

        <div className="update-action-grid">
          <Button type="button" className="epub-export-action-btn" onClick={() => runCheck("all", "check_only")} disabled={jobBusy}>
            {jobBusy ? "実行中..." : "すべてチェック"}
          </Button>
          <Button type="button" variant="secondary" size="sm" onClick={() => runCheck("all", "auto_save")} disabled={jobBusy}>
            <DownloadIcon /> 自動保存込み
          </Button>
          <Button type="button" variant="outline" size="sm" onClick={() => runCheck(activeTab === "results" ? "all" : activeTab, "check_only")} disabled={jobBusy}>
            <RefreshIcon /> 表示中だけ
          </Button>
        </div>

        {activeJob && (
          <Card className="space-y-3 rounded-md border bg-card/70 p-3">
            <div className="flex items-center justify-between gap-2 text-xs">
              <div className="min-w-0">
                <div className="flex items-center gap-2">
                  <Badge variant={activeJob.status === "failed" ? "destructive" : activeJob.status === "completed" ? "default" : "secondary"}>
                    {jobStatusLabel(activeJob.status)}
                  </Badge>
                  <span className="truncate font-medium">{activeJob.activeLabel || "更新ジョブ"}</span>
                </div>
                <p className="mt-1 text-muted-foreground">
                  {activeJob.processed}/{activeJob.totals} 件 / 候補 {activeJob.candidateCount} 件 / 保存 {activeJob.savedCount} 件 / エラー {activeJob.errorCount} 件
                </p>
              </div>
              <div className="flex shrink-0 flex-wrap justify-end gap-1">
                {activeJob.status === "running" || activeJob.status === "queued" ? (
                  <Button type="button" variant="outline" size="sm" onClick={() => controlJob("pause")} disabled={busy}>停止</Button>
                ) : null}
                {activeJob.status === "paused" || activeJob.status === "auth_required" || activeJob.status === "failed" ? (
                  <Button type="button" variant="outline" size="sm" onClick={() => controlJob("resume")} disabled={busy}>再開</Button>
                ) : null}
                {!isUpdateJobTerminal(activeJob.status) ? (
                  <Button type="button" variant="destructive" size="sm" onClick={() => controlJob("cancel")} disabled={busy}>キャンセル</Button>
                ) : null}
              </div>
            </div>
            <Progress value={progress} />
          </Card>
        )}

        {activeTab === "work" && (
          <div className="update-side-section">
            <div className="update-side-section-title">個別作品</div>
            {watchedWorks.length === 0 ? <div className="update-empty-row">カードをクリックして作品の更新監視をONにできます</div> : watchedWorks.map(work => (
              <Card key={work.id} className="update-target-row">
                <div className="update-target-info">
                  <strong>{work.title}</strong>
                  <span>{sourceLabel(work.source)} / {work.authorName}</span>
                </div>
                <Badge className="update-status-pill">監視中</Badge>
              </Card>
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
                    <Badge variant="outline">{selectedCandidateCount} / {candidates.length}</Badge>
                    <Button type="button" variant="outline" size="sm" onClick={() => setAllCandidatesSelected(true)} disabled={jobBusy}>全選択</Button>
                    <Button type="button" variant="outline" size="sm" onClick={() => setAllCandidatesSelected(false)} disabled={jobBusy}>全解除</Button>
                  </span>
                )}
              </div>
              {candidates.length === 0 ? <div className="update-empty-row">チェック後に候補が表示されます</div> : (
                <div ref={candidateListRef} className="update-virtual-list update-candidate-virtual-list">
                  <div className="update-virtual-spacer" style={{ height: candidateVirtualizer.getTotalSize() }}>
                    {candidateVirtualItems.map(virtualItem => {
                      const candidate = candidates[virtualItem.index];
                      const disabled = jobBusy || candidate.status === "saved" || candidate.status === "running" || candidate.status === "queued";
                      return (
                        <Card
                          key={candidate.id}
                          className={cn("update-candidate-row update-virtual-row", candidate.status)}
                          style={{ transform: `translateY(${virtualItem.start}px)` }}
                        >
                          <label className="flex min-w-0 flex-1 items-start gap-3">
                            <Checkbox
                              checked={selectedCandidateIds.has(candidate.id)}
                              disabled={disabled}
                              onCheckedChange={checked => setSelectedCandidateIds(prev => {
                                const next = new Set(prev);
                                if (checked === true) next.add(candidate.id);
                                else next.delete(candidate.id);
                                return next;
                              })}
                            />
                            <div className="update-target-info">
                              <strong>{candidate.title}</strong>
                              <span>{candidate.targetLabel} / {candidate.subtitle}</span>
                            </div>
                          </label>
                          <Badge variant={candidateStatusVariant(candidate.status)}>{candidateStatusLabel(candidate.status)}</Badge>
                        </Card>
                      );
                    })}
                  </div>
                </div>
              )}
              {candidates.length > 0 && (
                <Button type="button" className="epub-export-action-btn" onClick={saveCandidates} disabled={jobBusy || selectedCandidateCount === 0}>
                  <DownloadIcon /> 選択候補を保存
                </Button>
              )}
            </div>

            <div className="epub-log-panel">
              <div className="epub-log-header">
                <span>チェックログ</span>
                <Badge variant="outline">{logs.length}</Badge>
              </div>
              <div ref={logListRef} className="epub-log-entries update-virtual-list update-log-virtual-list">
                {jobBusy && logs.length === 0 ? (
                  <div className="space-y-2 p-2">
                    <Skeleton className="h-8 w-full" />
                    <Skeleton className="h-8 w-4/5" />
                  </div>
                ) : logs.length === 0 ? <div className="epub-log-empty">ログはありません</div> : (
                  <div className="update-virtual-spacer" style={{ height: logVirtualizer.getTotalSize() }}>
                    {logVirtualItems.map(virtualItem => {
                      const log = logs[virtualItem.index];
                      return (
                        <div
                          key={log.id}
                          className={`epub-log-entry update-virtual-row ${log.logType}`}
                          style={{ transform: `translateY(${virtualItem.start}px)` }}
                        >
                          <Badge variant={logBadgeVariant(log.logType)}>
                            {log.logType}
                          </Badge>
                          <span className="epub-log-text">{log.message}</span>
                        </div>
                      );
                    })}
                  </div>
                )}
              </div>
            </div>
          </>
        )}
      </div>
    </aside>
  );
}
