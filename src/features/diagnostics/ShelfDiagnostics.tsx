import { Alert, Badge, Box, Button, Card, Group, SimpleGrid, Stack, Text, ThemeIcon } from "@mantine/core";
import { modals } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useAppNavigate } from "@/app/router";
import { ErrorState } from "@/components/AsyncState";
import { startOperation } from "@/features/jobs/operationJobs";
import { searchShelf, useShelfWritable } from "@/features/library/shelfV2";
import { Icons, IconSize } from "@/lib/icons";
import { errorMessage, formatBytes, formatNumber } from "@/lib/format";
import {
  libraryV2Diagnostics,
  libraryV2Maintain,
  libraryV2OptimizeSearchIndex,
  libraryV2Reclaim,
  libraryV2Verify,
} from "@/services/shelfHealthApi";
import { MaintenanceTaskCard, MetricCard } from "./diagnosticsParts";

/** 切り替える前の棚で押せない手入れの理由。 */
const READ_ONLY = "新しい棚では変えられません";

/**
 * 新しい棚の診断と手入れ。
 *
 * 棚のファイルは中身の指紋で台帳と結ばれているので、旧棚の「保存フォルダーとの照合」は
 * 「台帳と全ファイルの照合」に、「未登録ファイル」は「参照されない実体の回収」に置き換わる。
 * 数えるのは台帳と索引の大きさだけで、ファイルの中身は読まない（開いたときに測る）。
 */
export function ShelfDiagnosticsSection({ runtime, busy }: { runtime: boolean; busy: boolean }) {
  const navigate = useAppNavigate();
  const queryClient = useQueryClient();
  const writable = useShelfWritable(runtime);
  const diagnostics = useQuery({
    queryKey: ["library-v2-diagnostics"],
    queryFn: libraryV2Diagnostics,
    enabled: runtime,
    staleTime: 30_000,
  });
  // 一部しか保存できていない作品。更新確認で取り直せるので、一覧のその絞り込みへ案内する。
  const partial = useQuery({
    queryKey: ["library-v2-partial-count"],
    queryFn: () => searchShelf({ acquisitionFilter: "incomplete", limit: 1 }).then((page) => page.totalEstimate ?? 0),
    enabled: runtime,
    staleTime: 60_000,
  });
  const refresh = () => {
    void diagnostics.refetch();
    void queryClient.invalidateQueries({ queryKey: ["search-index-status"] });
  };

  const run = <T,>(label: string, detail: string, action: () => Promise<T>, done: (result: T) => string) =>
    async () => {
      const operation = startOperation({ kind: "maintenance", label, detail });
      try {
        const result = await action();
        const message = done(result);
        operation.complete(message);
        notifications.show({ color: "green", title: label, message });
        refresh();
        return result;
      } catch (error) {
        operation.fail(error);
        throw error;
      }
    };
  const failed = (title: string) => (error: unknown) =>
    notifications.show({ color: "red", title, message: errorMessage(error) });

  const verify = useMutation({
    mutationFn: run("台帳とファイルを照合", "棚の台帳と、保存した全ファイルの中身を照合しています", libraryV2Verify,
      (files) => `${formatNumber(files)}ファイルを照合し、問題は見つかりませんでした`),
    onError: failed("照合で問題が見つかりました"),
  });
  const reclaim = useMutation({
    mutationFn: run("使われていない実体を片付け", "どこからも参照されないファイルを隔離し、期限の過ぎたものを消しています", libraryV2Reclaim,
      (report) => report.quarantined_chunks || report.deleted_chunks
        ? `${formatNumber(report.quarantined_chunks)}件を隔離、${formatBytes(report.deleted_bytes)}を解放しました`
        : "片付けるものはありませんでした"),
    onError: failed("片付けられません"),
  });
  const maintain = useMutation({
    mutationFn: (compact: boolean) => run(compact ? "データベースを圧縮" : "データベースを最適化", compact ? "使われていない領域を片付けて、ファイルを小さくします" : "データベースの統計を整理します", () => libraryV2Maintain(compact),
      (result) => result.compacted ? `${formatBytes(result.beforeBytes)} → ${formatBytes(result.afterBytes)}` : "最適化が完了しました")(),
    onError: failed("保守操作に失敗しました"),
  });
  const optimize = useMutation({
    mutationFn: run("全文検索索引を最適化", "分割された索引を安全に統合しています", libraryV2OptimizeSearchIndex,
      (result) => result.optimized
        ? `${formatNumber(result.beforeSegments)} → ${formatNumber(result.afterSegments)}セグメント · ${formatBytes(result.reclaimedBytes)}回収`
        : "索引はすでに最適です"),
    onError: failed("索引を最適化できません"),
  });
  const running = busy || verify.isPending || reclaim.isPending || maintain.isPending || optimize.isPending;
  const lock = !runtime || running;
  const writeHint = writable ? undefined : READ_ONLY;

  const confirmCompact = () => modals.openConfirmModal({
    title: "データベースの空き領域を回収しますか？",
    children: <Text size="sm">処理中はライブラリを操作できません。開始前に必要な空き容量を検査し、安全に実行できない場合は何も変更せず中止します。</Text>,
    labels: { confirm: "検査して圧縮", cancel: "キャンセル" }, confirmProps: { color: "yellow" },
    onConfirm: () => maintain.mutate(true),
  });

  const data = diagnostics.data;
  const index = data?.searchIndex;
  const indexRatio = index?.totalDownloads ? index.indexedDownloads / index.totalDownloads * 100 : 100;
  return <>
    <Card p="lg" mt="lg">
      <Group justify="space-between" align="center" wrap="wrap">
        <Group gap="sm"><ThemeIcon variant="light" size={40}><Icons.diagnostics size={IconSize.feature} /></ThemeIcon><Box>
          <Text fw={700}>ライブラリの状態</Text>
          <Text size="xs" c="dimmed" mt={3}>{diagnostics.isFetching ? "数えています" : data ? "最終確認: " + new Date(data.measuredAt).toLocaleString("ja-JP") : "台帳と索引の大きさを数えます。"}</Text>
        </Box></Group>
        <Button size="sm" variant="default" leftSection={<Icons.retry size={IconSize.menu} />} loading={diagnostics.isFetching} onClick={() => void diagnostics.refetch()}>再確認</Button>
      </Group>
    </Card>

    {(partial.data ?? 0) > 0 && <Box mt="lg"><MaintenanceTaskCard
      icon={Icons.warning}
      title="一部しか保存できていない作品"
      description="画像や添付の一部を保存できなかった作品です。更新を確認すると取り直せます。"
      status={<Badge color="yellow" variant="light">{formatNumber(partial.data ?? 0)}作品</Badge>}
      action={<Button variant="light" color="yellow" leftSection={<Icons.forward size={IconSize.menu} />} onClick={() => navigate("/library?acquisition=incomplete")}>一覧で見る</Button>}
    /></Box>}

    {diagnostics.error ? <Box mt="lg"><ErrorState error={diagnostics.error} retry={() => void diagnostics.refetch()} /></Box> : data && <Stack gap="lg" mt="xl">
      {data.fragmentationPercent >= 20 && <Alert color={data.fragmentationPercent >= 70 ? "red" : "yellow"} icon={<Icons.warning size={IconSize.nav} />} title={`DBの${data.fragmentationPercent.toFixed(2)}%が使われていない領域です`}>
        <Group justify="space-between" align="flex-end"><Text size="sm" maw={720}>中身は約{formatBytes(data.liveDatabaseBytes)}ですが、ファイルは{formatBytes(data.databaseBytes)}あります。検索結果には影響しませんが、バックアップの容量と読み書きが増えます。</Text><Button color="yellow" loading={maintain.isPending} disabled={lock || !writable} title={writeHint} aria-label={writable ? undefined : `空き容量を検査して圧縮（${READ_ONLY}）`} onClick={confirmCompact}>空き容量を検査して圧縮</Button></Group>
      </Alert>}
      <SimpleGrid cols={{ base: 1, sm: 2, xl: 4 }}>
        <MetricCard label="作品" value={`${formatNumber(data.works)}件`} detail={`${formatNumber(data.textLength)}文字 · ${formatNumber(data.versions)}版`} icon={Icons.database} />
        <MetricCard label="保存ファイル" value={formatBytes(data.storedBytes)} detail={`同じ中身は1回だけ保存 · ${formatNumber(data.files)}ファイル（元 ${formatBytes(data.logicalBytes)}）`} icon={Icons.storage} color="gray" />
        <MetricCard label="検索索引" value={`${indexRatio.toFixed(1)}%`} detail={`${formatNumber(index?.indexedDownloads ?? 0)} / ${formatNumber(index?.totalDownloads ?? 0)}作品`} icon={Icons.search} color={index?.isComplete ? "green" : "yellow"} />
        <MetricCard label="アプリ全体メモリ" value={data.processPrivateMemoryBytes === null ? (data.processMemoryBytes === null ? "取得不可" : formatBytes(data.processMemoryBytes)) : formatBytes(data.processPrivateMemoryBytes)} detail={`GPU ${data.gpuDedicatedMemoryBytes === null ? "取得不可" : formatBytes(data.gpuDedicatedMemoryBytes)}`} icon={Icons.activity} color="gray" />
      </SimpleGrid>
      <Card p="lg"><Text fw={700}>索引と台帳</Text><Stack gap="sm" mt="md">
        <Group justify="space-between"><Box><Text size="sm">全文索引</Text><Text size="xs" c="dimmed">{formatNumber(data.lexicalIndexSegments)}セグメント · 過去の版 {formatBytes(data.historyIndexBytes)}</Text></Box><Text size="sm" fw={650}>{formatBytes(data.lexicalIndexBytes)}</Text></Group>
        <Group justify="space-between"><Box><Text size="sm">意味検索索引</Text><Text size="xs" c="dimmed">{index?.semanticEnabled ? `${formatNumber(index.semanticIndexedDownloads)} / ${formatNumber(index.totalDownloads)}作品` : "意味検索は使っていません"}</Text></Box><Text size="sm" fw={650}>{formatBytes(data.semanticIndexBytes)}</Text></Group>
        <Group justify="space-between"><Box><Text size="sm">データベース</Text><Text size="xs" c="dimmed">中身 {formatBytes(data.liveDatabaseBytes)} · 書き込み途中 {formatBytes(data.walBytes)}</Text></Box><Text size="sm" fw={650}>{formatBytes(data.databaseBytes)}</Text></Group>
      </Stack></Card>
      <Text size="xs" c="dimmed">最終計測: {new Date(data.measuredAt).toLocaleString("ja-JP")} · 計測値はこの端末から送信されません。</Text>
    </Stack>}

    <SimpleGrid cols={{ base: 1, md: 2 }} mt="lg">
      <MaintenanceTaskCard
        icon={Icons.success}
        title="台帳とファイルの照合"
        description="保存した全ファイルの中身を読み、台帳の記録と一致するか確かめます。何も書き換えません。"
        status={<Badge color="blue" variant="light">読み取りだけ</Badge>}
        action={<Button variant="light" leftSection={<Icons.success size={IconSize.menu} />} loading={verify.isPending} disabled={lock} onClick={() => verify.mutate()}>照合する</Button>}
      />
      <MaintenanceTaskCard
        icon={Icons.storage}
        title="使われていない実体"
        description="どこからも参照されないファイルを隔離し、7日たっても使われなければ消します。"
        status={<Badge color="blue" variant="light">日常的な保守</Badge>}
        action={<Button variant="light" leftSection={<Icons.storage size={IconSize.menu} />} loading={reclaim.isPending} disabled={lock || !writable} title={writeHint} aria-label={writable ? undefined : `片付ける（${READ_ONLY}）`} onClick={() => reclaim.mutate()}>片付ける</Button>}
      />
      <MaintenanceTaskCard
        icon={Icons.database}
        title="データベース"
        description="検索統計を軽く整理します。"
        status={<Badge color="blue" variant="light">日常的な保守</Badge>}
        action={<Button variant="light" leftSection={<Icons.optimize size={IconSize.menu} />} loading={maintain.isPending} disabled={lock || !writable} title={writeHint} aria-label={writable ? undefined : `DBを最適化（${READ_ONLY}）`} onClick={() => maintain.mutate(false)}>DBを最適化</Button>}
      />
      <MaintenanceTaskCard
        icon={Icons.search}
        title="全文検索索引"
        description="分割された索引を統合し、検索時の負担を減らします。"
        status={!data ? <Badge color="gray" variant="light">状態不明</Badge> : data.lexicalIndexSegments <= 1 ? <Badge color="green" variant="light">最適です</Badge> : <Badge color={data.lexicalIndexSegments >= 16 ? "yellow" : "blue"} variant="light">{formatNumber(data.lexicalIndexSegments)}セグメント</Badge>}
        action={<Button variant="light" leftSection={<Icons.search size={IconSize.menu} />} loading={optimize.isPending} disabled={lock || !writable || !data || data.lexicalIndexSegments <= 1} title={writeHint} aria-label={writable ? undefined : `検索索引を最適化（${READ_ONLY}）`} onClick={() => optimize.mutate()}>検索索引を最適化</Button>}
      />
    </SimpleGrid>
  </>;
}
