import { useEffect, useRef, useState } from "react";
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
import { ErrorState, LoadingState } from "@/components/AsyncState";
import { JobProgress } from "@/components/JobProgress";
import { PageHeader } from "@/components/PageHeader";
import { RuntimeNotice } from "@/components/RuntimeNotice";
import { startOperation, type OperationController } from "@/features/jobs/operationJobs";
import { errorMessage, formatBytes, formatNumber } from "@/lib/format";
import { getLibraryDiagnostics, isTauriRuntime, maintainLibrary, optimizeSearchIndex } from "@/services/dbApi";
import { onTauriEvent } from "@/services/eventBus";
import type { LibraryDiagnostics } from "@/types/library";

const previewDiagnostics: LibraryDiagnostics = {
  measuredAt: "2026-08-09T12:00:00.000Z", totalDownloads: 1284, totalAssets: 8241, totalVersions: 1392, totalTextLength: 31_400_000,
  databaseSizeBytes: 94_000_000, walSizeBytes: 1_200_000, storageSizeBytes: 14_680_000_000, lexicalIndexSizeBytes: 36_000_000, lexicalIndexFileCount: 120, lexicalIndexSegmentCount: 8, semanticIndexSizeBytes: 210_000_000,
  sqlitePageCount: 22949, sqliteFreePages: 140, sqliteCacheSizeBytes: 8_000_000, liveDatabaseBytes: 93_400_000, fragmentationPercent: 0.61,
  orphanAssetRows: 0, orphanAssetBytes: 0, orphanAssetFiles: 0, orphanAssetFileBytes: 0, processMemoryBytes: 286_000_000, listFirstPageMs: 8.3, listP50Ms: 8.1, listP95Ms: 10.7,
  lexicalSearchMs: 43.2, lexicalSearchP50Ms: 42.8, lexicalSearchP95Ms: 51.4, exactAuthorP50Ms: 5.2, exactAuthorP95Ms: 7.8, benchmarkQuery: "preview author",
  searchIndex: { totalDownloads: 1284, indexedDownloads: 1284, pendingDownloads: 0, isComplete: true, phase: "ready", indexedChunks: 4192, semanticIndexedChunks: 4192, semanticModelReady: true, embeddingProvider: "DirectML (preview)", gpuEnabled: true, throughputPerSec: null },
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

function MetricCard({ label, value, detail, icon: Icon, color = "blue" }: { label: string; value: string; detail: string; icon: LucideIcon; color?: string }) {
  return <Card p="lg"><Group wrap="nowrap" align="flex-start"><ThemeIcon size={44} variant="light" color={color}><Icon size={20} /></ThemeIcon><Box miw={0}><Text size="xs" c="dimmed">{label}</Text><Text fz="xl" fw={760} className="line-clamp-1">{value}</Text><Text size="xs" c="dimmed" mt={4}>{detail}</Text></Box></Group></Card>;
}

function PerformanceRow({ name, current, p50, p95 }: { name: string; current: number | null; p50: number | null; p95: number | null }) {
  return <Table.Tr><Table.Td><Text fw={650} size="sm">{name}</Text></Table.Td><Table.Td><Badge variant="light" color={scoreColor(current)}>{formatLatency(current)}</Badge></Table.Td><Table.Td>{formatLatency(p50)}</Table.Td><Table.Td>{formatLatency(p95)}</Table.Td></Table.Tr>;
}

/** Named so the wait reads as work in progress rather than a stalled screen. */
const MEASURE_PHASE_LABEL: Record<string, string> = {
  database: "データベースの統計を読み取っています",
  "list-benchmark": "一覧表示の応答時間を計測しています",
  "search-benchmark": "全文検索の応答時間を計測しています",
  "author-benchmark": "作者の絞り込みを計測しています",
  "storage-scan": "保存フォルダーを走査しています（作品数が多いと時間がかかります）",
  "index-size": "索引と保存領域のサイズを集計しています",
};

const OPTIMIZE_PHASE_LABEL: Record<string, string> = {
  preflight: "空き容量を検査しています",
  merging: "セグメントを統合しています",
  complete: "完了しました",
  failed: "失敗しました",
};

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
export default function DiagnosticsPage({ embedded = false }: { embedded?: boolean } = {}) {
  const runtime = isTauriRuntime();
  const queryClient = useQueryClient();
  const operationRef = useRef<OperationController | null>(null);
  const retryRef = useRef<(compact: boolean) => void>(() => undefined);
  const indexRetryRef = useRef<() => void>(() => undefined);
  const [measurePhase, setMeasurePhase] = useState<{ phase: string; step: number; total: number } | null>(null);
  const [optimizePhase, setOptimizePhase] = useState<{ phase: string; segments: number } | null>(null);
  const diagnostics = useQuery({
    queryKey: ["library-diagnostics"],
    queryFn: () => runtime ? getLibraryDiagnostics() : Promise.resolve(previewDiagnostics),
    staleTime: 60_000,
  });
  useEffect(() => {
    if (!runtime) return;
    let dispose: (() => void) | undefined;
    onTauriEvent<{ phase: string; step: number; total: number }>("library-diagnostics-progress", ({ payload }) => setMeasurePhase(payload))
      .then((fn) => { dispose = fn; })
      .catch(() => undefined);
    return () => dispose?.();
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
      queryClient.invalidateQueries({ queryKey: ["library-diagnostics"] });
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
      queryClient.invalidateQueries({ queryKey: ["library-diagnostics"] });
      queryClient.invalidateQueries({ queryKey: ["library"] });
      notifications.show({ color: "green", title: result.optimized ? "全文検索索引を最適化しました" : "最適化は不要でした", message: detail });
    },
    onError: (error) => { operationRef.current = null; notifications.show({ color: "red", title: "索引を最適化できません", message: errorMessage(error) }); },
  });
  indexRetryRef.current = () => indexOptimization.mutate();
  const data = diagnostics.data;
  const indexRatio = data?.searchIndex.totalDownloads ? data.searchIndex.indexedDownloads / data.searchIndex.totalDownloads * 100 : 100;
  const reclaimable = data ? Math.max(0, data.databaseSizeBytes - data.liveDatabaseBytes) : 0;

  const confirmCompact = () => data && modals.openConfirmModal({
    title: "データベースの空き領域を回収しますか？",
    children: <Stack gap="sm"><Text size="sm">処理中はライブラリを操作できません。開始前に必要な空き容量を検査し、安全に実行できない場合は何も変更せず中止します。</Text><Alert color="blue" icon={<Icons.storage size={IconSize.action} />}>現在 {formatBytes(data.databaseSizeBytes)} / 推定回収可能 {formatBytes(reclaimable)}</Alert></Stack>,
    labels: { confirm: "検査して圧縮", cancel: "キャンセル" }, confirmProps: { color: "orange" },
    onConfirm: () => maintenance.mutate(true),
  });

  const confirmIndexOptimization = () => data && modals.openConfirmModal({
    title: "全文検索索引を統合しますか？",
    children: <Stack gap="sm"><Text size="sm">{formatNumber(data.lexicalIndexSegmentCount)}個のセグメントを1個へ安全に統合します。処理中も既存索引は保持され、開始前に一時領域の空き容量を検査します。大規模ライブラリでは数分以上かかる場合があります。</Text><Alert color="blue" icon={<Icons.search size={IconSize.action} />}>索引 {formatBytes(data.lexicalIndexSizeBytes)} · {formatNumber(data.lexicalIndexFileCount)}ファイル</Alert></Stack>,
    labels: { confirm: "検査して索引を最適化", cancel: "キャンセル" },
    confirmProps: { color: "orange" },
    onConfirm: () => indexOptimization.mutate(),
  });

  return <div className={embedded ? "diagnostics-page" : "page page--contained diagnostics-page"}>
    <PageHeader title="ライブラリ診断" description="実データで容量、索引完成度、検索時間、SQLiteの健全性を測定します。" actions={<Group><Button variant="default" leftSection={<Icons.retry size={IconSize.menu} />} loading={diagnostics.isFetching} onClick={() => diagnostics.refetch()}>再計測</Button><Button variant="light" leftSection={<Icons.optimize size={IconSize.menu} />} loading={maintenance.isPending} disabled={!runtime || indexOptimization.isPending} onClick={() => maintenance.mutate(false)}>DBを最適化</Button><Button color="orange" variant="light" leftSection={<Icons.search size={IconSize.menu} />} loading={indexOptimization.isPending} disabled={!runtime || maintenance.isPending || !data || data.lexicalIndexSegmentCount <= 1} onClick={confirmIndexOptimization}>検索索引を最適化</Button></Group>} />
    <RuntimeNotice />
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
              color="orange"
              note="統合の途中でも既存の索引はそのまま検索できます。"
            />
          </Box>
        )}
      </Card>
    )}
    {diagnostics.isLoading ? <LoadingState label="実ライブラリを計測しています" /> : diagnostics.error ? <ErrorState error={diagnostics.error} retry={() => diagnostics.refetch()} /> : data && <Stack gap="lg" mt="lg">
      <SimpleGrid cols={{ base: 1, sm: 2, lg: 4 }}>
        <MetricCard label="作品" value={`${formatNumber(data.totalDownloads)}件`} detail={`${formatNumber(data.totalTextLength)}文字 · ${formatNumber(data.totalVersions)}版`} icon={Icons.database} />
        <MetricCard label="保存データ" value={formatBytes(data.storageSizeBytes)} detail={`${formatNumber(data.totalAssets)}アセット`} icon={Icons.storage} color="indigo" />
        <MetricCard label="検索索引" value={`${indexRatio.toFixed(1)}%`} detail={`${formatNumber(data.searchIndex.indexedDownloads)} / ${formatNumber(data.searchIndex.totalDownloads)}作品`} icon={Icons.search} color={data.searchIndex.isComplete ? "green" : "yellow"} />
        <MetricCard label="プロセスメモリ" value={data.processMemoryBytes === null ? "取得不可" : formatBytes(data.processMemoryBytes)} detail={`Cache ${formatBytes(data.sqliteCacheSizeBytes)}`} icon={Icons.activity} color="cyan" />
      </SimpleGrid>

      {data.fragmentationPercent >= 20 && <Alert color={data.fragmentationPercent >= 70 ? "red" : "yellow"} icon={<Icons.warning size={IconSize.nav} />} title={`DBの${data.fragmentationPercent.toFixed(2)}%が再利用待ち領域です`}>
        <Group justify="space-between" align="flex-end"><Text size="sm" maw={720}>論理使用量は約{formatBytes(data.liveDatabaseBytes)}ですが、物理ファイルは{formatBytes(data.databaseSizeBytes)}あります。検索結果の正しさには影響しませんが、バックアップ容量とディスクI/Oを増やします。</Text><Button color="orange" loading={maintenance.isPending} disabled={!runtime} onClick={confirmCompact}>空き容量を検査して圧縮</Button></Group>
      </Alert>}

      {data.lexicalIndexSegmentCount >= 16 && <Alert color={data.lexicalIndexSegmentCount >= 64 ? "red" : "yellow"} icon={<Icons.warning size={IconSize.nav} />} title={`全文検索索引が${formatNumber(data.lexicalIndexSegmentCount)}セグメントに分割されています`}>
        <Group justify="space-between" align="flex-end"><Text size="sm" maw={720}>検索のたびに多数のセグメントを横断するため、作品数が多いほど待ち時間とメモリ使用量が増えます。統合は検索内容を変えず、索引ファイルを整理します。</Text><Button color="orange" loading={indexOptimization.isPending} disabled={!runtime || maintenance.isPending} onClick={confirmIndexOptimization}>空き容量を検査して統合</Button></Group>
      </Alert>}

      <SimpleGrid cols={{ base: 1, lg: 2 }}>
        <Card p="lg"><Group justify="space-between"><Box><Title order={3}>実データ性能</Title><Text size="sm" c="dimmed">同じ処理を複数回測定したアプリ内部時間</Text></Box><ThemeIcon size={44} variant="light" color="green"><Icons.diagnostics size={IconSize.feature} /></ThemeIcon></Group><Table mt="lg" verticalSpacing="sm"><Table.Thead><Table.Tr><Table.Th>処理</Table.Th><Table.Th>今回</Table.Th><Table.Th>p50</Table.Th><Table.Th>p95</Table.Th></Table.Tr></Table.Thead><Table.Tbody><PerformanceRow name="一覧の先頭80件" current={data.listFirstPageMs} p50={data.listP50Ms} p95={data.listP95Ms} /><PerformanceRow name="最多作者名の全文検索" current={data.lexicalSearchMs} p50={data.lexicalSearchP50Ms} p95={data.lexicalSearchP95Ms} /><PerformanceRow name="作者の完全一致絞り込み" current={data.exactAuthorP50Ms} p50={data.exactAuthorP50Ms} p95={data.exactAuthorP95Ms} /></Table.Tbody></Table>{data.benchmarkQuery && <Text size="xs" c="dimmed" mt="md">作者名の実データを端末内だけで使用して計測しました。</Text>}</Card>
        <Card p="lg"><Title order={3}>索引と保存領域</Title><Stack gap="md" mt="lg"><Box><Group justify="space-between"><Box><Text size="sm">全文索引</Text><Text size="xs" c="dimmed">{formatNumber(data.lexicalIndexSegmentCount)}セグメント · {formatNumber(data.lexicalIndexFileCount)}ファイル</Text></Box><Text size="sm" fw={650}>{formatBytes(data.lexicalIndexSizeBytes)}</Text></Group><Group justify="space-between" mt="sm"><Text size="sm">意味検索索引</Text><Text size="sm" fw={650}>{formatBytes(data.semanticIndexSizeBytes)}</Text></Group></Box><Divider /><Box><Group justify="space-between"><Text size="sm">索引完成度</Text><Text size="xs" c="dimmed">{data.searchIndex.pendingDownloads}件待機</Text></Group><Progress value={indexRatio} mt="xs" color={data.searchIndex.isComplete ? "green" : "yellow"} /></Box><Divider /><Group justify="space-between"><Box><Text size="sm">孤立アセットDB行</Text><Text size="xs" c="dimmed">作品との関連を失った記録</Text></Box><Badge variant="light" color={data.orphanAssetRows ? "red" : "green"}>{data.orphanAssetRows ? `${formatNumber(data.orphanAssetRows)}件 · ${formatBytes(data.orphanAssetBytes)}` : "0件"}</Badge></Group><Group justify="space-between"><Box><Text size="sm">孤立アセットファイル</Text><Text size="xs" c="dimmed">保存フォルダーにある未参照ファイル</Text></Box><Badge variant="light" color={data.orphanAssetFiles ? "yellow" : "green"}>{data.orphanAssetFiles ? `${formatNumber(data.orphanAssetFiles)}件 · ${formatBytes(data.orphanAssetFileBytes)}` : "0件"}</Badge></Group></Stack></Card>
      </SimpleGrid>
      <Text size="xs" c="dimmed">最終計測: {new Date(data.measuredAt).toLocaleString("ja-JP")} · 計測値はこの端末から送信されません。</Text>
    </Stack>}
  </div>;
}
