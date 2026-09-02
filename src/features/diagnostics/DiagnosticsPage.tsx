import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  Alert,
  Badge,
  Box,
  Button,
  Card,
  Divider,
  Group,
  Progress,
  SimpleGrid,
  Stack,
  Table,
  Text,
  ThemeIcon,
  Title,
} from "@mantine/core";
import { modals } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Icons, IconSize, type LucideIcon } from "@/lib/icons";
import { ErrorState } from "@/components/AsyncState";
import { JobProgress } from "@/components/JobProgress";
import { PageHeader } from "@/components/PageHeader";
import { RuntimeNotice } from "@/components/RuntimeNotice";
import { useAppNavigate } from "@/app/router";
import { startOperation, type OperationController } from "@/features/jobs/operationJobs";
import { errorMessage, formatBytes, formatNumber } from "@/lib/format";
import { getStoragePath } from "@/services/archiveApi";
import {
  cancelEntityProfileRepair,
  getEntityProfileRepairStatus,
  getLibraryDiagnostics,
  isTauriRuntime,
  maintainLibrary,
  optimizeSearchIndex,
  repairIncompleteEntityProfiles,
  scanAndReimportDownloads,
  type EntityProfileRepairProgress,
} from "@/services/dbApi";
import { getUpdateJobCredentials } from "@/services/updateJobApi";
import { onTauriEvent, subscribeTauriEvent } from "@/services/eventBus";
import { openFilesystemPath } from "@/services/openerApi";
import type { LibraryDiagnostics, LibraryFileIssue } from "@/types/library";

export const previewDiagnostics: LibraryDiagnostics = {
  measuredAt: "2026-08-09T12:00:00.000Z", totalDownloads: 1284, totalAssets: 8241, totalVersions: 1392, totalTextLength: 31_400_000,
  databaseSizeBytes: 94_000_000, walSizeBytes: 1_200_000, storageSizeBytes: 14_680_000_000, lexicalIndexSizeBytes: 36_000_000, lexicalIndexFileCount: 120, lexicalIndexSegmentCount: 8, semanticIndexSizeBytes: 210_000_000,
  sqlitePageCount: 22949, sqliteFreePages: 140, sqliteCacheSizeBytes: 8_000_000, liveDatabaseBytes: 93_400_000, fragmentationPercent: 0.61,
  orphanAssetRows: 0, orphanAssetBytes: 0, orphanAssetFiles: 0, orphanAssetFileBytes: 0, processMemoryBytes: 286_000_000, processPrivateMemoryBytes: 244_000_000, processCount: 7, webviewProcessCount: 6, gpuDedicatedMemoryBytes: 67_000_000, gpuSharedMemoryBytes: 9_000_000, listFirstPageMs: 8.3, listP50Ms: 8.1, listP95Ms: 10.7,
  checkedFileReferences: 9633, missingJsonFiles: 0, missingAssetFiles: 0, missingProfileFiles: 0, unsafeReferencedFiles: 0, unreadableReferencedFiles: 0, emptyReferencedFiles: 0, mismatchedAssetFiles: 0, transientFiles: 0, transientFileBytes: 0, fileIssueSamples: [],
  lexicalSearchMs: 43.2, lexicalSearchP50Ms: 42.8, lexicalSearchP95Ms: 51.4, exactAuthorP50Ms: 5.2, exactAuthorP95Ms: 7.8, benchmarkQuery: "preview author",
  searchIndex: { totalDownloads: 1284, indexedDownloads: 1284, pendingDownloads: 0, isComplete: true, phase: "ready", semanticIndexedChunks: 1284, semanticIndexedDownloads: 1284, semanticPendingDownloads: 0, semanticEnabled: true, semanticModelReady: true, embeddingProvider: "DirectML (preview)", gpuEnabled: true, throughputPerSec: null },
};

function scoreColor(ms: number | null) {
  if (ms === null) return "gray";
  if (ms < 100) return "green";
  if (ms < 500) return "yellow";
  return "red";
}

function formatLatency(ms: number | null) {
  if (ms === null) return "—";
  return ms >= 1_000 ? `${(ms / 1_000).toFixed(2)} s` : `${ms.toFixed(1)} ms`;
}

function MetricCard({ label, value, detail, icon: Icon, color = "piep" }: { label: string; value: string; detail: string; icon: LucideIcon; color?: string }) {
  return <Card p="lg"><Group wrap="nowrap" align="flex-start" gap="sm"><ThemeIcon size={38} variant="light" color={color} style={{ flex: "0 0 auto" }}><Icon size={18} /></ThemeIcon><Box miw={0} style={{ flex: "1 1 auto" }}><Text size="xs" c="dimmed" className="line-clamp-1">{label}</Text><Text fz="xl" fw={760} className="metric-card__value">{value}</Text><Text size="xs" c="dimmed" mt={4}>{detail}</Text></Box></Group></Card>;
}

function MaintenanceTaskCard({ icon: Icon, title, description, status, action }: { icon: LucideIcon; title: string; description: string; status: ReactNode; action: ReactNode }) {
  return <Card p="lg" className="maintenance-task-card">
    <Stack gap="md" h="100%">
      <Group wrap="nowrap" align="flex-start" gap="sm">
        <ThemeIcon size={40} variant="light" color="piep" style={{ flex: "0 0 auto" }}><Icon size={19} /></ThemeIcon>
        <Box miw={0}>
          <Text fw={720}>{title}</Text>
          <Text size="xs" c="dimmed" mt={3}>{description}</Text>
        </Box>
      </Group>
      <Box>{status}</Box>
      <Box mt="auto">{action}</Box>
    </Stack>
  </Card>;
}

/**
 * 色は **p50 に付ける**。
 *
 * 「初回」は7回測ったうちの1回目で、索引のセグメントもページキャッシュも
 * 冷えている。**構造的に必ずいちばん遅い**ので、そこへ閾値の色を付けると
 * 何度測っても赤いままになり、壊れているように見える。実際に効いていたのは
 * これで、5,410 作品の棚で「初回 919ms・p50 366ms」が毎回赤く出ていた。
 *
 * 初回の数字自体は捨てない。アプリを開いて最初に引くときの体感がそれである。
 */
function PerformanceRow({ name, cold, p50, p95 }: { name: string; cold: number | null; p50: number | null; p95: number | null }) {
  return <Table.Tr><Table.Td><Text fw={650} size="sm">{name}</Text></Table.Td><Table.Td c="dimmed">{formatLatency(cold)}</Table.Td><Table.Td><Badge variant="light" color={scoreColor(p50)} tt="none">{formatLatency(p50)}</Badge></Table.Td><Table.Td>{formatLatency(p95)}</Table.Td></Table.Tr>;
}

/** Named so the wait reads as work in progress rather than a stalled screen. */
const MEASURE_PHASE_LABEL: Record<string, string> = {
  database: "データベースの統計を読み取っています",
  "list-benchmark": "一覧表示の応答時間を計測しています",
  "search-benchmark": "全文検索の応答時間を計測しています",
  "author-benchmark": "作者の絞り込みを計測しています",
  "file-integrity": "DB参照と保存ファイルを照合しています",
  "storage-scan": "保存フォルダーを走査しています（作品数が多いと時間がかかります）",
  "index-size": "索引と保存領域のサイズを集計しています",
};

const OPTIMIZE_PHASE_LABEL: Record<string, string> = {
  preflight: "空き容量を検査しています",
  merging: "セグメントを統合しています",
  complete: "完了しました",
  failed: "失敗しました",
};

const FILE_ISSUE_LABEL: Record<string, string> = {
  missing: "参照先がありません",
  unsafe: "許可領域外またはリンク",
  unreadable: "読み取れません",
  empty: "0バイトです",
  size_mismatch: "記録された容量と異なります",
  transient: "未完了の一時ファイル",
};

const FILE_CATEGORY_LABEL: Record<string, string> = {
  work_json: "作品データ",
  work_asset: "画像・添付",
  profile: "作者・シリーズ画像",
  entity_json: "作者・シリーズ情報",
  transient: "一時ファイル",
};

function parentDirectory(path: string): string | null {
  if (path.endsWith("…") || !/^(?:[A-Za-z]:[\\/]|\/)/.test(path)) return null;
  const trimmed = path.replace(/[\\/]+$/, "");
  const separator = Math.max(trimmed.lastIndexOf("\\"), trimmed.lastIndexOf("/"));
  return separator > 0 ? trimmed.slice(0, separator) : null;
}

function issueSizeDetail(issue: LibraryFileIssue): string | null {
  if (issue.issueType === "size_mismatch") {
    return `記録 ${formatBytes(issue.expectedSizeBytes ?? 0)} / 実ファイル ${formatBytes(issue.actualSizeBytes ?? 0)}`;
  }
  if (issue.actualSizeBytes !== null) return formatBytes(issue.actualSizeBytes);
  return null;
}

/** Ticks once a second so a phase without a percentage still shows movement. */
function useElapsedSeconds(active: boolean) {
  const [elapsed, setElapsed] = useState(0);
  useEffect(() => {
    if (!active) { setElapsed(0); return; }
    const startedAt = Date.now();
    const timer = window.setInterval(() => setElapsed((Date.now() - startedAt) / 1000), 500);
    return () => window.clearInterval(timer);
  }, [active]);
  return elapsed;
}

/**
 * Measurement and maintenance of the library.
 *
 * Reachable both as its own screen and as a settings section: it is about the
 * app's own housekeeping rather than about the works, so settings is where it
 * belongs - but the route stays valid for anything already pointing at it.
 */
export default function DiagnosticsPage({ embedded = false, previewData = previewDiagnostics }: { embedded?: boolean; previewData?: LibraryDiagnostics } = {}) {
  const runtime = isTauriRuntime();
  const navigate = useAppNavigate();
  const queryClient = useQueryClient();
  const operationRef = useRef<OperationController | null>(null);
  const profileRepairOperationRef = useRef<OperationController | null>(null);
  const retryRef = useRef<(compact: boolean) => void>(() => undefined);
  const indexRetryRef = useRef<() => void>(() => undefined);
  const reimportRetryRef = useRef<() => void>(() => undefined);
  const profileRepairRetryRef = useRef<() => void>(() => undefined);
  const [measurePhase, setMeasurePhase] = useState<{ phase: string; step: number; total: number } | null>(null);
  const [optimizePhase, setOptimizePhase] = useState<{ phase: string; segments: number } | null>(null);
  const [profileRepairProgress, setProfileRepairProgress] = useState<EntityProfileRepairProgress | null>(null);
  // Never on arrival. Measuring walks the whole storage folder and runs three
  // benchmarks against the real library; on a large one that is seconds of work
  // and disk, started by nothing more than opening a screen. A result already
  // in the cache is still shown - it is the *running* that waits to be asked
  // for, not the reading.
  const diagnostics = useQuery({
    queryKey: ["library-diagnostics"],
    queryFn: () => runtime ? getLibraryDiagnostics() : Promise.resolve(previewData),
    staleTime: Infinity,
    enabled: false,
  });
  const measured = Boolean(diagnostics.data);
  const profileRepairStatus = useQuery({
    queryKey: ["entity-profile-repair-status"],
    queryFn: () => runtime
      ? getEntityProfileRepairStatus()
      : Promise.resolve({ personCount: 0, seriesCount: 0, totalCount: 0 }),
    staleTime: 30_000,
  });
  useEffect(() => {
    if (!runtime) return;
    return subscribeTauriEvent<{ phase: string; step: number; total: number }>("library-diagnostics-progress", ({ payload }) => setMeasurePhase(payload));
  }, [runtime]);
  useEffect(() => { if (!diagnostics.isFetching) setMeasurePhase(null); }, [diagnostics.isFetching]);
  const measureElapsed = useElapsedSeconds(diagnostics.isFetching);
  const optimizeElapsed = useElapsedSeconds(Boolean(optimizePhase) && optimizePhase?.phase !== "complete" && optimizePhase?.phase !== "failed");
  const maintenance = useMutation({
    mutationFn: async (compact: boolean) => {
      operationRef.current = startOperation({ kind: "maintenance", label: compact ? "データベースを圧縮" : "ライブラリを最適化", detail: compact ? "空きページを回収して物理サイズを縮小します" : "SQLite統計とWALを最適化します", onRetry: () => retryRef.current(compact) });
      let dispose: (() => void) | undefined;
      if (runtime) dispose = await onTauriEvent<{ phase: string; error?: string | null }>("library-maintenance-progress", ({ payload }) => operationRef.current?.log(payload.error || `フェーズ: ${payload.phase}`, payload.error ? "error" : "info"));
      try { return await maintainLibrary(compact); }
      catch (error) { operationRef.current?.fail(error); throw error; }
      finally { dispose?.(); }
    },
    onSuccess: (result) => {
      operationRef.current?.complete(result.compacted ? `${formatBytes(result.reclaimedBytes)}を回収しました` : "最適化が完了しました");
      operationRef.current = null;
      // 計測クエリは enabled: false なので、invalidate しても再取得は起きない。
      // 印だけ古くなって、画面には最適化前の容量が残りつづける。取り込み直しの
      // 側（下の refetch）と同じく、ここも明示的に取り直す。
      diagnostics.refetch();
      notifications.show({ color: "green", title: result.compacted ? "データベースを圧縮しました" : "ライブラリを最適化しました", message: result.compacted ? `${formatBytes(result.beforeBytes)} → ${formatBytes(result.afterBytes)}` : "検索統計とWALを整理しました" });
    },
    onError: (error) => { operationRef.current = null; notifications.show({ color: "red", title: "保守操作に失敗しました", message: errorMessage(error) }); },
  });
  retryRef.current = (compact) => maintenance.mutate(compact);
  const indexOptimization = useMutation({
    mutationFn: async () => {
      operationRef.current = startOperation({ kind: "maintenance", label: "全文検索索引を最適化", detail: "分割された索引を安全に統合しています", onRetry: () => indexRetryRef.current() });
      setOptimizePhase({ phase: "preflight", segments: diagnostics.data?.lexicalIndexSegmentCount ?? 0 });
      let dispose: (() => void) | undefined;
      if (runtime) dispose = await onTauriEvent<{ phase: string; segments?: number; error?: string | null }>("search-index-optimization-progress", ({ payload }) => {
        setOptimizePhase({ phase: payload.phase, segments: payload.segments ?? 0 });
        operationRef.current?.log(payload.error || OPTIMIZE_PHASE_LABEL[payload.phase] || payload.phase, payload.error ? "error" : "info");
      });
      try { return await optimizeSearchIndex(); }
      catch (error) { operationRef.current?.fail(error); throw error; }
      finally { dispose?.(); setOptimizePhase(null); }
    },
    onSuccess: (result) => {
      const detail = result.optimized ? `${formatNumber(result.beforeSegments)} → ${formatNumber(result.afterSegments)}セグメント · ${formatBytes(result.reclaimedBytes)}回収` : "索引はすでに最適です";
      operationRef.current?.complete(detail);
      operationRef.current = null;
      diagnostics.refetch();
      queryClient.invalidateQueries({ queryKey: ["library"] });
      notifications.show({ color: "green", title: result.optimized ? "全文検索索引を最適化しました" : "最適化は不要でした", message: detail });
    },
    onError: (error) => { operationRef.current = null; notifications.show({ color: "red", title: "索引を最適化できません", message: errorMessage(error) }); },
  });
  indexRetryRef.current = () => indexOptimization.mutate();
  const reimport = useMutation({
    mutationFn: async () => {
      const operation = startOperation({
        kind: "maintenance",
        label: "保存フォルダーから再取り込み",
        detail: "DBにない完全な作品だけを検査します",
        onRetry: () => reimportRetryRef.current(),
      });
      try {
        const outcome = await scanAndReimportDownloads();
        outcome.skipped.forEach((reason) => operation.log(reason, "warn"));
        operation.complete(outcome.imported > 0 ? `${formatNumber(outcome.imported)}件を再取り込みしました` : "再取り込み対象はありませんでした");
        return outcome;
      } catch (error) {
        operation.fail(error);
        throw error;
      }
    },
    onSuccess: async (outcome) => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["dashboard"] }),
        queryClient.invalidateQueries({ queryKey: ["library"] }),
        queryClient.invalidateQueries({ queryKey: ["stats"] }),
      ]);
      await diagnostics.refetch();
      // 飛ばした作品があることは、件数と同じ行で伝える。黙っていると
      // 「全部戻ったはずなのに数が合わない」だけが残る。
      const skippedNote = outcome.skipped.length
        ? `${formatNumber(outcome.skipped.length)}件は読めずに飛ばしました。詳しい理由は操作の記録に出しています。`
        : "";
      notifications.show({
        color: outcome.skipped.length ? "yellow" : outcome.imported > 0 ? "green" : "blue",
        title: outcome.imported > 0 ? "再取り込みが完了しました" : "再取り込み対象はありませんでした",
        message: [
          outcome.imported > 0
            ? `${formatNumber(outcome.imported)}件をライブラリへ戻しました。診断結果も更新しました。`
            : "DBに登録済みの作品は上書きしません。参照切れはバックアップ復元または元サービスからの再保存を使用してください。",
          skippedNote,
        ].filter(Boolean).join(" "),
      });
    },
    onError: (error) => notifications.show({ color: "red", title: "再取り込みできませんでした", message: errorMessage(error) }),
  });
  reimportRetryRef.current = () => reimport.mutate();
  const profileRepair = useMutation({
    mutationFn: async () => {
      profileRepairOperationRef.current = startOperation({
        kind: "maintenance",
        label: "作者・シリーズ情報を修復",
        detail: "完全取得できていないプロフィールを残件から再取得します",
        total: profileRepairStatus.data?.totalCount ?? null,
        onCancel: async () => { await cancelEntityProfileRepair(); },
        onRetry: () => profileRepairRetryRef.current(),
      });
      let dispose: (() => void) | undefined;
      if (runtime) {
        dispose = await onTauriEvent<EntityProfileRepairProgress>("entity-profile-repair-progress", ({ payload }) => {
          setProfileRepairProgress(payload);
          profileRepairOperationRef.current?.progress(
            payload.completed,
            payload.total,
            payload.activeLabel ? `確認中: ${payload.activeLabel}` : undefined,
          );
          if (payload.error) profileRepairOperationRef.current?.log(`${payload.activeLabel ?? "プロフィール"}: ${payload.error}`, "warn");
        });
      }
      try {
        return await repairIncompleteEntityProfiles(await getUpdateJobCredentials());
      } catch (error) {
        profileRepairOperationRef.current?.fail(error);
        throw error;
      } finally {
        dispose?.();
      }
    },
    onSuccess: async (result) => {
      if (result.canceled) {
        profileRepairOperationRef.current?.cancel(`${formatNumber(result.attempted)}件を確認した時点で中止しました`);
      } else {
        profileRepairOperationRef.current?.complete(
          result.remaining
            ? `${formatNumber(result.repaired)}件を修復、${formatNumber(result.remaining)}件は次回再試行できます`
            : `${formatNumber(result.repaired)}件を修復し、未完了はなくなりました`,
        );
      }
      profileRepairOperationRef.current = null;
      setProfileRepairProgress(null);
      await profileRepairStatus.refetch();
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["library"] }),
        queryClient.invalidateQueries({ queryKey: ["entity"] }),
      ]);
      notifications.show({
        color: result.canceled ? "gray" : result.remaining ? "yellow" : "green",
        title: result.canceled ? "プロフィール修復を中止しました" : "プロフィール修復が完了しました",
        message: result.remaining
          ? `${formatNumber(result.repaired)}件を修復しました。残り${formatNumber(result.remaining)}件は、再実行すると続きから確認します。`
          : `${formatNumber(result.repaired)}件を修復しました。`,
      });
    },
    onError: (error) => {
      profileRepairOperationRef.current = null;
      setProfileRepairProgress(null);
      notifications.show({ color: "red", title: "作者・シリーズ情報を修復できません", message: errorMessage(error) });
    },
  });
  profileRepairRetryRef.current = () => profileRepair.mutate();
  const data = diagnostics.data;
  const indexRatio = data?.searchIndex.totalDownloads ? data.searchIndex.indexedDownloads / data.searchIndex.totalDownloads * 100 : 100;
  const reclaimable = data ? Math.max(0, data.databaseSizeBytes - data.liveDatabaseBytes) : 0;
  const fileIntegrityIssues = data ? data.missingJsonFiles + data.missingAssetFiles + data.missingProfileFiles + data.unsafeReferencedFiles + data.unreadableReferencedFiles + data.emptyReferencedFiles + data.mismatchedAssetFiles + data.transientFiles : 0;

  const confirmCompact = () => data && modals.openConfirmModal({
    title: "データベースの空き領域を回収しますか？",
    children: <Stack gap="sm"><Text size="sm">処理中はライブラリを操作できません。開始前に必要な空き容量を検査し、安全に実行できない場合は何も変更せず中止します。</Text><Alert color="piep" icon={<Icons.storage size={IconSize.action} />}>現在 {formatBytes(data.databaseSizeBytes)} / 推定回収可能 {formatBytes(reclaimable)}</Alert></Stack>,
    labels: { confirm: "検査して圧縮", cancel: "キャンセル" }, confirmProps: { color: "yellow" },
    onConfirm: () => maintenance.mutate(true),
  });

  const confirmIndexOptimization = () => data && modals.openConfirmModal({
    title: "全文検索索引を統合しますか？",
    children: <Stack gap="sm"><Text size="sm">{formatNumber(data.lexicalIndexSegmentCount)}個のセグメントを1個へ安全に統合します。処理中も既存索引は保持され、開始前に一時領域の空き容量を検査します。大規模ライブラリでは数分以上かかる場合があります。</Text><Alert color="piep" icon={<Icons.search size={IconSize.action} />}>索引 {formatBytes(data.lexicalIndexSizeBytes)} · {formatNumber(data.lexicalIndexFileCount)}ファイル</Alert></Stack>,
    labels: { confirm: "検査して索引を最適化", cancel: "キャンセル" },
    confirmProps: { color: "yellow" },
    onConfirm: () => indexOptimization.mutate(),
  });

  const confirmReimport = () => modals.openConfirmModal({
    title: "保存フォルダーから再取り込みしますか？",
    children: <Stack gap="sm">
      <Text size="sm">DBから消えた一方、完全な保存フォルダーが残っている作品だけを1件ずつ検証して戻します。お気に入りや編集を守るため、DBに登録済みの作品は上書きしません。</Text>
      <Alert color="blue" title="この操作で直せないもの">登録済み作品のJSON・画像が欠けている場合は、バックアップ復元または元サービスからの再保存が必要です。</Alert>
    </Stack>,
    labels: { confirm: "検査して再取り込み", cancel: "キャンセル" },
    onConfirm: () => reimport.mutate(),
  });

  const confirmProfileRepair = () => {
    const status = profileRepairStatus.data;
    if (!status?.totalCount) return;
    modals.openConfirmModal({
      title: "作者・シリーズ情報を修復しますか？",
      children: <Stack gap="sm">
        <Text size="sm">保存済み作品との紐付けは変えず、完全取得できていない作者{formatNumber(status.personCount)}件・シリーズ{formatNumber(status.seriesCount)}件の情報だけを取得元から埋め直します。</Text>
        <Alert color="blue" title="途中で止めても続きから再開できます">取得元への負荷を抑えるため1件ずつ間隔を空けます。現在の件数では数分かかる場合があります。成功した対象はその都度確定し、通信失敗・中止・アプリ終了後も残件だけを再実行できます。</Alert>
      </Stack>,
      labels: { confirm: "修復を開始", cancel: "キャンセル" },
      onConfirm: () => profileRepair.mutate(),
    });
  };

  const openStorageFolder = async () => {
    try {
      await openFilesystemPath(await getStoragePath());
    } catch (error) {
      notifications.show({ color: "red", title: "保存先を開けません", message: errorMessage(error) });
    }
  };

  const openIssueParent = async (issue: LibraryFileIssue) => {
    const parent = parentDirectory(issue.path);
    if (!parent) return;
    try {
      await openFilesystemPath(parent);
    } catch (error) {
      notifications.show({ color: "red", title: "親フォルダーを開けません", message: errorMessage(error) });
    }
  };

  const showFileIssues = () => data && modals.open({
    title: `確認が必要なファイル（最大${formatNumber(data.fileIssueSamples.length)}件）`,
    size: "xl",
    children: <Stack gap="md">
      <Alert color="blue" title="表示だけでは変更しません">
        DBやファイルを削除せず、診断で見つかった先頭{formatNumber(data.fileIssueSamples.length)}件を表示しています。全体は{formatNumber(fileIntegrityIssues)}件です。
      </Alert>
      {/* 縦に積む箱と、縦に流れる箱は別物である。Stack のまま高さを絞ると、
          中の Card が flex で縮み、Card は自分の中身を切り落とす - 帯の文字が
          上下で切れて重なっていたのはこれだった。流すのは外の箱の仕事にする。 */}
      <Box mah="55vh" style={{ overflowY: "auto" }} pr="xs">
        <Stack gap="sm">
          {data.fileIssueSamples.map((issue, index) => {
            const sizeDetail = issueSizeDetail(issue);
            const parent = parentDirectory(issue.path);
            return <Card key={`${issue.issueType}-${issue.path}-${index}`} withBorder p="sm">
              <Group justify="space-between" align="flex-start" wrap="nowrap">
                <Box miw={0}>
                  <Group gap="xs">
                    <Badge color={issue.issueType === "transient" || issue.issueType === "empty" || issue.issueType === "size_mismatch" ? "yellow" : "red"} variant="light">{FILE_ISSUE_LABEL[issue.issueType] ?? issue.issueType}</Badge>
                    <Badge color="gray" variant="light">{FILE_CATEGORY_LABEL[issue.category] ?? issue.category}</Badge>
                  </Group>
                  {issue.label && <Text mt="xs" size="sm" fw={650}>{issue.label}</Text>}
                  <Text mt="xs" size="xs" ff="monospace" style={{ overflowWrap: "anywhere" }}>{issue.path}</Text>
                  {sizeDetail && <Text mt={4} size="xs" c="dimmed">{sizeDetail}</Text>}
                </Box>
                <Button size="compact-xs" variant="default" disabled={!runtime || !parent} onClick={() => openIssueParent(issue)}>親フォルダーを開く</Button>
              </Group>
            </Card>;
          })}
        </Stack>
      </Box>
      {data.fileIssueSamples.length < fileIntegrityIssues && <Text size="xs" c="dimmed">画面負荷を抑えるため先頭{formatNumber(data.fileIssueSamples.length)}件だけ表示しています。原因別の総件数は診断画面のバッジで確認できます。</Text>}
    </Stack>,
  });

  return <div className={embedded ? "diagnostics-page" : "page page--contained diagnostics-page"}>
    <PageHeader title="ライブラリ診断" description="状態を確認し、必要な保守だけを実行します。計測は自動では始まりません。" />
    {/* Settings already carries one at the top of the screen. */}
    {!embedded && <RuntimeNotice />}

    <Box mt="lg">
      <SimpleGrid cols={{ base: 1, md: 3 }}>
        <MaintenanceTaskCard
          icon={Icons.diagnostics}
          title="ライブラリを計測"
          description="容量、ファイル整合性、検索時間を読み取ります。"
          status={measured
            ? <Badge color="green" variant="light">{`計測済み · ${new Date(data!.measuredAt).toLocaleString("ja-JP")}`}</Badge>
            : <><Badge color="gray" variant="light">未計測</Badge><Text size="xs" c="dimmed" mt={6}>保存フォルダーも走査するため、作品数が多いと数十秒かかります。</Text></>}
          action={<Button fullWidth variant={measured ? "default" : "filled"} leftSection={<Icons.retry size={IconSize.menu} />} loading={diagnostics.isFetching} onClick={() => diagnostics.refetch()}>{measured ? "再計測" : "計測する"}</Button>}
        />
        <MaintenanceTaskCard
          icon={Icons.database}
          title="データベース"
          description="検索統計と一時ログを軽く整理します。計測なしでも実行できます。"
          status={<Badge color="blue" variant="light">日常的な保守</Badge>}
          action={<Button fullWidth variant="light" leftSection={<Icons.optimize size={IconSize.menu} />} loading={maintenance.isPending} disabled={!runtime || indexOptimization.isPending} onClick={() => maintenance.mutate(false)}>DBを最適化</Button>}
        />
        <MaintenanceTaskCard
          icon={Icons.search}
          title="全文検索索引"
          description="分割された索引を統合し、検索時の負担を減らします。"
          status={!data
            ? <><Badge color="gray" variant="light">状態不明</Badge><Text size="xs" c="dimmed" mt={6}>計測すると最適化が必要か判断できます。</Text></>
            : data.lexicalIndexSegmentCount <= 1
              ? <Badge color="green" variant="light">最適です</Badge>
              : <Badge color={data.lexicalIndexSegmentCount >= 16 ? "yellow" : "blue"} variant="light">{formatNumber(data.lexicalIndexSegmentCount)}セグメント</Badge>}
          action={<Button fullWidth variant="light" color={data && data.lexicalIndexSegmentCount >= 16 ? "yellow" : "piep"} leftSection={<Icons.search size={IconSize.menu} />} loading={indexOptimization.isPending} disabled={!runtime || maintenance.isPending || !data || data.lexicalIndexSegmentCount <= 1} onClick={confirmIndexOptimization}>検索索引を最適化</Button>}
        />
      </SimpleGrid>
    </Box>

    {profileRepairStatus.error && <Alert mt="lg" color="red" icon={<Icons.warning size={IconSize.nav} />} title="作者・シリーズ情報の状態を確認できません">
      <Group justify="space-between"><Text size="sm">{errorMessage(profileRepairStatus.error)}</Text><Button size="xs" variant="default" onClick={() => profileRepairStatus.refetch()}>再確認</Button></Group>
    </Alert>}
    {profileRepairStatus.data && <Card p="lg" mt="lg" className="maintenance-integrity-card">
      <Group justify="space-between" align="flex-start" wrap="nowrap">
        <Group align="flex-start" wrap="nowrap" miw={0}>
          <ThemeIcon variant="light" color={profileRepairStatus.data.totalCount ? "yellow" : "green"}>{profileRepairStatus.data.totalCount ? <Icons.warning size={IconSize.menu} /> : <Icons.success size={IconSize.menu} />}</ThemeIcon>
          <Box miw={0}>
            <Group gap="xs">
              <Text fw={720}>作者・シリーズ情報</Text>
              <Badge color={profileRepairStatus.data.totalCount ? "yellow" : "green"} variant="light">{profileRepairStatus.data.totalCount ? `${formatNumber(profileRepairStatus.data.totalCount)}件 未完了` : "問題なし"}</Badge>
            </Group>
            <Text size="xs" c="dimmed" mt={5}>{profileRepairStatus.data.totalCount ? "保存の中断や通信失敗で簡易情報のまま残った項目があります。作品との紐付けは保たれています。" : "保存済み作品から参照される情報はすべて取得済みです。"}</Text>
            {profileRepairStatus.data.totalCount > 0 && <Group gap="xs" mt="xs"><Badge color="yellow" variant="light">作者 {formatNumber(profileRepairStatus.data.personCount)}</Badge><Badge color="yellow" variant="light">シリーズ {formatNumber(profileRepairStatus.data.seriesCount)}</Badge></Group>}
          </Box>
        </Group>
        {profileRepairStatus.data.totalCount > 0 && (profileRepair.isPending
          ? <Button color="red" variant="subtle" leftSection={<Icons.stop size={IconSize.menu} />} onClick={() => void cancelEntityProfileRepair()}>中止</Button>
          : <Button leftSection={<Icons.retry size={IconSize.menu} />} disabled={!runtime || maintenance.isPending || indexOptimization.isPending} onClick={confirmProfileRepair}>残件を修復</Button>)}
      </Group>
      {profileRepairProgress && <Box mt="md">
        <Group justify="space-between" mb={6}><Text size="xs" c="dimmed">{profileRepairProgress.activeLabel ? `確認中: ${profileRepairProgress.activeLabel}` : "修復を準備しています"}</Text><Text size="xs" c="dimmed">{formatNumber(profileRepairProgress.completed)} / {formatNumber(profileRepairProgress.total)}</Text></Group>
        <Progress value={profileRepairProgress.total ? profileRepairProgress.completed / profileRepairProgress.total * 100 : 0} animated />
        <Text size="xs" c="dimmed" mt={6}>修復 {formatNumber(profileRepairProgress.repaired)}件 · 今回失敗 {formatNumber(profileRepairProgress.failed)}件</Text>
      </Box>}
    </Card>}
    {(diagnostics.isFetching || optimizePhase) && (
      <Card p="md" mt="md">
        {diagnostics.isFetching && (
          <JobProgress
            phase={MEASURE_PHASE_LABEL[measurePhase?.phase ?? ""] ?? "実ライブラリを計測しています"}
            processed={measurePhase ? measurePhase.step : null}
            total={measurePhase ? measurePhase.total : null}
            elapsedSeconds={measureElapsed}
            unit="工程"
          />
        )}
        {optimizePhase && (
          <Box mt={diagnostics.isFetching ? "md" : undefined}>
            <JobProgress
              phase={`${OPTIMIZE_PHASE_LABEL[optimizePhase.phase] ?? optimizePhase.phase}${optimizePhase.segments ? `（${formatNumber(optimizePhase.segments)}セグメント）` : ""}`}
              elapsedSeconds={optimizeElapsed}
              color="yellow"
              note="統合の途中でも既存の索引はそのまま検索できます。"
            />
          </Box>
        )}
      </Card>
    )}
    {!measured && !diagnostics.isFetching && !diagnostics.error
      ? <Text size="sm" c="dimmed" mt="lg">まだ計測していません。上の「ライブラリを計測」から開始できます。</Text>
      : diagnostics.error ? <Box mt="lg"><ErrorState error={diagnostics.error} retry={() => diagnostics.refetch()} /></Box> : data && <Stack gap="lg" mt="xl">
      <Box>
        <Title order={2}>診断結果</Title>
        <Text size="sm" c="dimmed" mt={4}>計測結果と、対応が必要な項目だけを表示します。</Text>
      </Box>
      <SimpleGrid cols={{ base: 1, sm: 2, xl: 4 }}>
        <MetricCard label="作品" value={`${formatNumber(data.totalDownloads)}件`} detail={`${formatNumber(data.totalTextLength)}文字 · ${formatNumber(data.totalVersions)}版`} icon={Icons.database} />
        <MetricCard label="保存データ" value={formatBytes(data.storageSizeBytes)} detail={`${formatNumber(data.totalAssets)}アセット`} icon={Icons.storage} color="gray" />
        <MetricCard label="検索索引" value={`${indexRatio.toFixed(1)}%`} detail={`${formatNumber(data.searchIndex.indexedDownloads)} / ${formatNumber(data.searchIndex.totalDownloads)}作品`} icon={Icons.search} color={data.searchIndex.isComplete ? "green" : "yellow"} />
        <MetricCard label="アプリ全体メモリ" value={data.processPrivateMemoryBytes === null ? (data.processMemoryBytes === null ? "取得不可" : formatBytes(data.processMemoryBytes)) : formatBytes(data.processPrivateMemoryBytes)} detail={`${formatNumber(data.processCount)}プロセス · WebView ${formatNumber(data.webviewProcessCount)} · WS ${data.processMemoryBytes === null ? "取得不可" : formatBytes(data.processMemoryBytes)}`} icon={Icons.activity} color="gray" />
        <MetricCard label="GPUメモリ" value={data.gpuDedicatedMemoryBytes === null ? "取得不可" : formatBytes(data.gpuDedicatedMemoryBytes)} detail={`共有 ${data.gpuSharedMemoryBytes === null ? "取得不可" : formatBytes(data.gpuSharedMemoryBytes)}`} icon={Icons.activity} color="gray" />
      </SimpleGrid>

      {data.fragmentationPercent >= 20 && <Alert color={data.fragmentationPercent >= 70 ? "red" : "yellow"} icon={<Icons.warning size={IconSize.nav} />} title={`DBの${data.fragmentationPercent.toFixed(2)}%が再利用待ち領域です`}>
        <Group justify="space-between" align="flex-end"><Text size="sm" maw={720}>論理使用量は約{formatBytes(data.liveDatabaseBytes)}ですが、物理ファイルは{formatBytes(data.databaseSizeBytes)}あります。検索結果の正しさには影響しませんが、バックアップ容量とディスクI/Oを増やします。</Text><Button color="yellow" loading={maintenance.isPending} disabled={!runtime} onClick={confirmCompact}>空き容量を検査して圧縮</Button></Group>
      </Alert>}

      <Alert
        color={fileIntegrityIssues ? (data.missingJsonFiles + data.missingAssetFiles + data.unsafeReferencedFiles > 0 ? "red" : "yellow") : "green"}
        icon={fileIntegrityIssues ? <Icons.warning size={IconSize.nav} /> : <Icons.success size={IconSize.nav} />}
        title={fileIntegrityIssues ? `保存ファイルに${formatNumber(fileIntegrityIssues)}件の確認事項があります` : "DB参照と保存ファイルは一致しています"}
      >
        {fileIntegrityIssues ? <Stack gap="xs">
          <Text size="sm">手動で移動・削除・置換された可能性があるファイルを検出しました。診断は原本を自動削除・上書きしません。参照切れはバックアップからの復元または元サービスからの再保存、一時ファイルはアプリ再起動後の再計測を優先してください。</Text>
          <Group gap="xs">
            {data.missingJsonFiles > 0 && <Badge color="red" variant="light">JSON参照切れ {formatNumber(data.missingJsonFiles)}</Badge>}
            {data.missingAssetFiles > 0 && <Badge color="red" variant="light">アセット参照切れ {formatNumber(data.missingAssetFiles)}</Badge>}
            {data.missingProfileFiles > 0 && <Badge color="yellow" variant="light">プロフィール画像参照切れ {formatNumber(data.missingProfileFiles)}</Badge>}
            {data.unsafeReferencedFiles > 0 && <Badge color="red" variant="light">領域外・リンク {formatNumber(data.unsafeReferencedFiles)}</Badge>}
            {data.unreadableReferencedFiles > 0 && <Badge color="red" variant="light">読取不能 {formatNumber(data.unreadableReferencedFiles)}</Badge>}
            {data.emptyReferencedFiles > 0 && <Badge color="yellow" variant="light">0バイト {formatNumber(data.emptyReferencedFiles)}</Badge>}
            {data.mismatchedAssetFiles > 0 && <Badge color="yellow" variant="light">容量差 {formatNumber(data.mismatchedAssetFiles)}</Badge>}
            {data.transientFiles > 0 && <Badge color="yellow" variant="light">一時ファイル {formatNumber(data.transientFiles)} · {formatBytes(data.transientFileBytes)}</Badge>}
          </Group>
          <Card withBorder p="md" mt="xs">
            <Stack gap="sm">
              <Box>
                <Text fw={700} size="sm">次に行うこと</Text>
                <Text size="xs" c="dimmed" mt={2}>対象を確認してから、状態に合う安全な方法を選べます。診断画面から勝手に削除・参照解除はしません。</Text>
              </Box>
              <Group gap="xs">
                <Button size="xs" variant="filled" leftSection={<Icons.read size={IconSize.menu} />} disabled={!data.fileIssueSamples.length} onClick={showFileIssues}>対象ファイルを確認</Button>
                <Button size="xs" variant="default" leftSection={<Icons.openFolder size={IconSize.menu} />} disabled={!runtime} onClick={openStorageFolder}>保存先を開く</Button>
                <Button size="xs" variant="default" leftSection={<Icons.retry size={IconSize.menu} />} loading={reimport.isPending} disabled={!runtime || maintenance.isPending || indexOptimization.isPending} onClick={confirmReimport}>残ったフォルダーを再取り込み</Button>
                <Button size="xs" variant="default" leftSection={<Icons.import size={IconSize.menu} />} onClick={() => navigate("/settings?section=library")}>バックアップ復元へ</Button>
                <Button size="xs" variant="default" leftSection={<Icons.save size={IconSize.menu} />} onClick={() => navigate("/save")}>元サービスから再保存</Button>
              </Group>
              <Text size="xs" c="dimmed">
                JSON・画像の参照切れ／0バイト／容量差は「バックアップ復元」または「元サービスから再保存」。DBだけ消えた作品は「残ったフォルダーを再取り込み」。領域外・リンクは対象一覧で場所を確認し、管理フォルダー内の実ファイルとして再保存してください。一時ファイルはアプリ再起動後に再計測します。
              </Text>
            </Stack>
          </Card>
        </Stack> : <Text size="sm">{formatNumber(data.checkedFileReferences)}件のDB参照を照合し、参照切れ・領域外リンク・0バイト化・既知の容量差は見つかりませんでした。</Text>}
      </Alert>

      <SimpleGrid cols={{ base: 1, lg: 2 }}>
        <Card p="lg"><Group justify="space-between"><Box><Title order={3}>実データ性能</Title><Text size="sm" c="dimmed">同じ処理を複数回測定したアプリ内部時間</Text></Box><ThemeIcon size={44} variant="light" color="green"><Icons.diagnostics size={IconSize.feature} /></ThemeIcon></Group><Table.ScrollContainer minWidth={380} type="native"><Table mt="lg" verticalSpacing="sm"><Table.Thead><Table.Tr><Table.Th>処理</Table.Th><Table.Th>初回（冷えた状態）</Table.Th><Table.Th>p50</Table.Th><Table.Th>p95</Table.Th></Table.Tr></Table.Thead><Table.Tbody><PerformanceRow name="一覧の先頭80件" cold={data.listFirstPageMs} p50={data.listP50Ms} p95={data.listP95Ms} /><PerformanceRow name="最多作者名の全文検索" cold={data.lexicalSearchMs} p50={data.lexicalSearchP50Ms} p95={data.lexicalSearchP95Ms} /><PerformanceRow name="作者の完全一致絞り込み" cold={data.exactAuthorP50Ms} p50={data.exactAuthorP50Ms} p95={data.exactAuthorP95Ms} /></Table.Tbody></Table></Table.ScrollContainer>{data.benchmarkQuery && <Text size="xs" c="dimmed" mt="md">作者名の実データを端末内だけで使用して計測しました。</Text>}</Card>
        <Card p="lg"><Title order={3}>索引と保存領域</Title><Stack gap="md" mt="lg"><Box><Group justify="space-between"><Box><Text size="sm">全文索引</Text><Text size="xs" c="dimmed">{formatNumber(data.lexicalIndexSegmentCount)}セグメント · {formatNumber(data.lexicalIndexFileCount)}ファイル</Text></Box><Text size="sm" fw={650}>{formatBytes(data.lexicalIndexSizeBytes)}</Text></Group><Group justify="space-between" mt="sm"><Box><Text size="sm">意味検索索引</Text><Text size="xs" c="dimmed">{formatNumber(data.searchIndex.semanticIndexedDownloads)} / {formatNumber(data.searchIndex.totalDownloads)}作品{data.searchIndex.semanticPendingDownloads ? ` · ${formatNumber(data.searchIndex.semanticPendingDownloads)}件が未反映` : ""}</Text></Box><Text size="sm" fw={650}>{formatBytes(data.semanticIndexSizeBytes)}</Text></Group></Box><Divider /><Box><Group justify="space-between"><Text size="sm">索引完成度</Text><Text size="xs" c="dimmed">{data.searchIndex.pendingDownloads}件待機</Text></Group><Progress value={indexRatio} mt="xs" color={data.searchIndex.isComplete ? "green" : "yellow"} /></Box><Divider /><Group justify="space-between"><Box><Text size="sm">孤立アセットDB行</Text><Text size="xs" c="dimmed">作品との関連を失った記録</Text></Box><Badge variant="light" color={data.orphanAssetRows ? "red" : "green"}>{data.orphanAssetRows ? `${formatNumber(data.orphanAssetRows)}件 · ${formatBytes(data.orphanAssetBytes)}` : "0件"}</Badge></Group><Group justify="space-between"><Box><Text size="sm">孤立アセットファイル</Text><Text size="xs" c="dimmed">保存フォルダーにある未参照ファイル</Text></Box><Badge variant="light" color={data.orphanAssetFiles ? "yellow" : "green"}>{data.orphanAssetFiles ? `${formatNumber(data.orphanAssetFiles)}件 · ${formatBytes(data.orphanAssetFileBytes)}` : "0件"}</Badge></Group></Stack></Card>
      </SimpleGrid>
      <Text size="xs" c="dimmed">最終計測: {new Date(data.measuredAt).toLocaleString("ja-JP")} · 計測値はこの端末から送信されません。</Text>
    </Stack>}
  </div>;
}
