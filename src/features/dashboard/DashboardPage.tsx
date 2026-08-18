import { useMemo } from "react";
import {
  Alert,
  Badge,
  Box,
  Button,
  Card,
  Grid,
  Group,
  Progress,
  SimpleGrid,
  Stack,
  Text,
  ThemeIcon,
  Title,
} from "@mantine/core";
import { useForm } from "@mantine/form";
import { useQuery } from "@tanstack/react-query";
import { Icons, IconSize, type LucideIcon } from "@/lib/icons";
import { useAppNavigate } from "@/app/router";
import { WorkCard } from "@/components/WorkCard";
import { EmptyState, ErrorState, LoadingState } from "@/components/AsyncState";
import { RuntimeNotice } from "@/components/RuntimeNotice";
import { demoDashboard } from "@/mocks/demoData";
import { formatBytes, formatNumber } from "@/lib/format";
import { getProvider } from "@/lib/providers";
import { getDashboardSummary, getSearchIndexStatus, isTauriRuntime } from "@/services/dbApi";
import { store } from "@/store";

export type QuickSaveSource = "pixiv" | "fanbox";

export function quickSaveSource(value: string): QuickSaveSource | null {
  try {
    const url = new URL(value.trim());
    if (url.protocol !== "https:" && url.protocol !== "http:") return null;
    const host = url.hostname.toLowerCase();
    if (host === "pixiv.net" || host.endsWith(".pixiv.net")) return "pixiv";
    if (host === "fanbox.cc" || host.endsWith(".fanbox.cc")) return "fanbox";
    return null;
  } catch {
    return null;
  }
}

/** Enough to keep four rows however many columns the card's width allows: six
 *  columns of four at the widest. The card shows the first four rows and clips
 *  the rest, so the count only has to be generous, not exact. */
const TOP_TAG_COUNT = 24;

export default function DashboardPage() {
  const navigate = useAppNavigate();
  const runtime = isTauriRuntime();
  const summary = useQuery({
    queryKey: ["dashboard"],
    queryFn: () => runtime ? getDashboardSummary() : Promise.resolve(demoDashboard),
  });
  const index = useQuery({
    queryKey: ["search-index-status"],
    queryFn: () => runtime ? getSearchIndexStatus() : Promise.resolve({
      totalDownloads: demoDashboard.stats.totalDownloads,
      indexedDownloads: demoDashboard.stats.totalDownloads,
      pendingDownloads: 0,
      isComplete: true,
      phase: "ready",
      indexedChunks: 4192,
      semanticIndexedChunks: 4192,
      semanticModelReady: true,
      embeddingProvider: "preview",
      gpuEnabled: true,
    }),
  });
  const auth = useQuery({
    queryKey: ["auth-status"],
    queryFn: async () => runtime ? {
      pixiv: Boolean(await store.get<string>("pixiv_refresh_token")),
      fanbox: Boolean(await store.get<string>("fanbox_session_id")),
    } : { pixiv: true, fanbox: true },
  });
  const quickSave = useForm({
    mode: "uncontrolled",
    initialValues: { url: "" },
    validate: { url: (value) => quickSaveSource(value) ? null : "pixiv.netまたはfanbox.ccのURLを入力してください" },
  });
  const chartData = useMemo(() => buildTrend(summary.data?.monthlyDownloads ?? []), [summary.data]);

  if (summary.isLoading) return <div className="page"><LoadingState label="ホームを準備しています" /></div>;
  if (summary.error || !summary.data) return <div className="page"><ErrorState error={summary.error} retry={() => summary.refetch()} /></div>;
  const data = summary.data;
  const indexProgress = index.data?.totalDownloads ? Math.round(index.data.indexedDownloads / index.data.totalDownloads * 100) : 100;

  return (
    <div className="page page--contained dashboard-page">
      <Stack gap="md">
        <Title order={1} className="visually-hidden">ホーム</Title>
        <RuntimeNotice />
        <Card className="dashboard-hero dashboard-quick-save" padding="lg">
          <Group wrap="nowrap" align="center" gap="lg">
            <ThemeIcon size={52} radius="lg" variant="light" className="dashboard-quick-save__logo"><Icons.collect size={IconSize.feature} /></ThemeIcon>
            <Stack gap="sm" flex={1} miw={0}>
              {/* The status chips have a fixed intrinsic width; letting them
                  shrink cut their labels in half on a narrow window. The
                  description truncates instead. */}
              <Group justify="space-between" align="flex-start" wrap="nowrap" gap="sm"><Box miw={0}><Text size="sm" fw={750} c="piep.6">Webから保存</Text><Text size="sm" c="dimmed" className="line-clamp-2">URLを貼るだけで作品・シリーズ・作者を判定します</Text></Box><Group gap="xs" wrap="nowrap" visibleFrom="sm" style={{ flex: "0 0 auto" }}><ConnectionBadge label="pixiv" connected={auth.isError ? null : auth.data?.pixiv} /><ConnectionBadge label="FANBOX" connected={auth.isError ? null : auth.data?.fanbox} /></Group></Group>
              <form onSubmit={quickSave.onSubmit(({ url }) => { const source = quickSaveSource(url); if (source) navigate(`/save/${source}?url=${encodeURIComponent(url.trim())}`); })}>
                <Group align="flex-start" wrap="nowrap">
                  <Box flex={1}><input key={quickSave.key("url")} id="dashboard-save-url" className={`quick-save-input${quickSave.errors.url ? " quick-save-input--error" : ""}`} placeholder="作品ページのURLを貼り付け" aria-label="保存するページのURL" aria-invalid={Boolean(quickSave.errors.url)} aria-describedby={quickSave.errors.url ? "dashboard-save-url-error" : undefined} {...quickSave.getInputProps("url")} />{quickSave.errors.url && <Text id="dashboard-save-url-error" role="alert" size="xs" c="red" mt={4}>{quickSave.errors.url}</Text>}</Box>
                  <Button type="submit" rightSection={<Icons.forward size={IconSize.menu} />}>候補を開く</Button>
                </Group>
              </form>
              {auth.data && (!auth.data.pixiv || !auth.data.fanbox) && <Button variant="subtle" size="compact-xs" w="fit-content" onClick={() => navigate("/settings")}>未接続サービスを設定</Button>}
              {auth.isError && <Button variant="subtle" color="red" size="compact-xs" w="fit-content" onClick={() => auth.refetch()}>接続状態を再確認</Button>}
            </Stack>
          </Group>
        </Card>

        {/* Each tile opens the library already filtered to what it counts, so
            the number and its destination agree. */}
        <SimpleGrid cols={{ base: 2, md: 4 }} spacing="sm">
          <StatCard icon={Icons.database} label="保存作品" value={formatNumber(data.stats.totalDownloads)} hint={`pixiv ${formatNumber(data.stats.pixivCount)} · FANBOX ${formatNumber(data.stats.fanboxCount)}`} color="#0d86f4" onClick={() => navigate("/library")} />
          <StatCard icon={Icons.storage} label="使用容量" value={formatBytes(data.stats.totalSizeBytes)} hint={`${formatNumber(data.stats.totalAssets)} アセット`} color="#31b497" onClick={() => navigate("/library?sort=file_size_bytes")} />
          <StatCard icon={Icons.favorite} label="お気に入り" value={formatNumber(data.favoriteCount)} hint="すぐ読み返せる作品" color="#ef5b78" onClick={() => navigate("/library?favorite=1")} />
          <StatCard icon={Icons.updates} label="更新監視" value={formatNumber(data.watchedCount)} hint={`${formatNumber(data.updateTargetCount)} 作者・シリーズ`} color="#86b918" onClick={() => navigate("/library?watch=watched")} />
        </SimpleGrid>

        {/* Two columns from the medium breakpoint rather than the large one:
            at ordinary window widths everything used to stack, leaving three
            full-width strips down the page. */}
        <Grid gap="lg" align="stretch">
          <Grid.Col span={{ base: 12, md: 6, lg: 7 }}>
            <Card p="lg" h="100%" className="dashboard-trend-card">
              <Group justify="space-between" mb="md">
                <Box>
                  <Text fw={700}>保存の推移</Text>
                  <Text size="xs" c="dimmed" mt={3}>直近{chartData.length}か月 · {formatNumber(data.stats.totalDownloads)}件</Text>
                </Box>
                <Group gap="md"><TrendLegend color="#0096fa" label="pixiv" /><TrendLegend color="#d1a900" label="FANBOX" /></Group>
              </Group>
              <DownloadTrend data={chartData} />
              <SourceComposition breakdown={data.sourceBreakdown} total={data.stats.totalDownloads} />
            </Card>
          </Grid.Col>
          <Grid.Col span={{ base: 12, md: 6, lg: 5 }}>
            {/* Side by side once the column is full width, so neither ends up
                as a lonely strip. */}
            <div className="dashboard-side">
              <Card p="lg">
                <Group justify="space-between" mb="sm"><Group gap="xs"><Icons.search size={IconSize.action} /><Text fw={700}>検索インデックス</Text></Group><Badge color={index.isError ? "red" : indexProgress === 100 ? "green" : "yellow"}>{index.isError ? "取得失敗" : `${indexProgress}%`}</Badge></Group>
                {index.isError ? <Button variant="subtle" color="red" size="compact-xs" onClick={() => index.refetch()}>状態を再確認</Button> : <><Progress value={indexProgress} mb="sm" color={indexProgress === 100 ? "green" : "yellow"} aria-label={`検索インデックス ${indexProgress}%`} /><Text size="xs" c="dimmed">{index.data?.isComplete ? "全文・意味検索は最新です" : `${formatNumber(index.data?.pendingDownloads)}件を処理中`}</Text></>}
              </Card>
              <Card p="lg" className="dashboard-tags-card">
                <Text fw={700} mb="sm">よく使うタグ</Text>
                <div className="dashboard-tags">{data.topTags.slice(0, TOP_TAG_COUNT).map((tag) => <Badge className="dashboard-tag" key={tag.name} variant="light" color="gray" component="button" title={`${tag.name}（${formatNumber(tag.count)}件）`} onClick={() => navigate(`/library?q=${encodeURIComponent(tag.name)}`)}><span className="dashboard-tag__name">{tag.name}</span><span className="dashboard-tag__count">{formatNumber(tag.count)}</span></Badge>)}</div>
              </Card>
            </div>
          </Grid.Col>
        </Grid>

        <Box>
          <Group justify="space-between" mb="md"><Box><Title order={2}>最近の保存</Title><Text size="sm" c="dimmed">続きから読む、整理する</Text></Box><Button variant="subtle" rightSection={<Icons.forward size={IconSize.menu} />} onClick={() => navigate("/library")}>すべて表示</Button></Group>
          {data.recentDownloads.length ? <div className="work-grid">{data.recentDownloads.slice(0, 4).map((work) => <WorkCard key={work.id} work={work} />)}</div> : <EmptyState icon={Icons.database} title="まだ作品がありません" description="Webから最初の作品を保存すると、ここからすぐに読み返せます。" action={<Button leftSection={<Icons.collect size={IconSize.menu} />} onClick={() => navigate("/save/pixiv")}>作品を保存する</Button>} />}
        </Box>
      </Stack>
    </div>
  );
}

function ConnectionBadge({ label, connected }: { label: string; connected?: boolean | null }) {
  const status = connected === undefined ? "確認中" : connected === null ? "確認失敗" : connected ? "接続済み" : "未接続";
  return <Badge color={connected === null ? "red" : connected ? "green" : "gray"} variant="light" leftSection={<span className="status-dot" />}>{label} {status}</Badge>;
}

function StatCard({ icon: Icon, label, value, hint, color, onClick }: { icon: LucideIcon; label: string; value: string; hint: string; color: string; onClick: () => void }) {
  return (
    <Card component="button" type="button" className="stat-card" style={{ "--stat-accent": color }} onClick={onClick}>
      <Group justify="space-between" align="flex-start"><Text size="sm" c="dimmed" fw={600}>{label}</Text><ThemeIcon variant="light" color="gray"><Icon size={17} style={{ color }} /></ThemeIcon></Group>
      <Text className="stat-card__value" fz="20px" fw={760} mt={2} lts="-0.035em" ta="center">{value}</Text>
      <Group justify="space-between" gap="xs" wrap="nowrap"><Text size="xs" c="dimmed" mt={2} className="line-clamp-1">{hint}</Text><Icons.forward className="stat-card__arrow" size={14} /></Group>
    </Card>
  );
}

interface TrendMonth { key: string; label: string; total: number; pixiv: number; fanbox: number; other: number }

/**
 * The API returns only the months that have saves, so feeding it straight into
 * a fixed grid left a lone bar stranded in an empty chart. This rebuilds a
 * continuous run of months instead: it starts at the first month with a save,
 * never reaches back further than twelve months, and always ends at the
 * current month, so empty months read as genuine gaps.
 */
function buildTrend(points: { bucket: string; count: number; pixivCount: number; fanboxCount: number }[]): TrendMonth[] {
  if (!points.length) return [];
  const toIndex = (bucket: string) => {
    const [year, month] = bucket.split("-").map(Number);
    return year * 12 + (month - 1);
  };
  const now = new Date();
  const current = now.getFullYear() * 12 + now.getMonth();
  const earliest = Math.min(...points.map((point) => toIndex(point.bucket)));
  const start = Math.max(earliest, current - 11);
  const byIndex = new Map(points.map((point) => [toIndex(point.bucket), point]));
  const months: TrendMonth[] = [];
  for (let index = start; index <= current; index += 1) {
    const point = byIndex.get(index);
    const year = Math.floor(index / 12);
    const month = index % 12;
    months.push({
      key: `${year}-${month}`,
      label: new Intl.DateTimeFormat("ja-JP", { month: "short" }).format(new Date(year, month, 1)),
      total: point?.count ?? 0,
      pixiv: point?.pixivCount ?? 0,
      fanbox: point?.fanboxCount ?? 0,
      other: point ? Math.max(0, point.count - point.pixivCount - point.fanboxCount) : 0,
    });
  }
  return months;
}

function DownloadTrend({ data }: { data: TrendMonth[] }) {
  const maximum = Math.max(1, ...data.map((item) => item.total));
  const label = data.map((item) => `${item.label} ${item.total}件`).join("、");
  return (
    <Box className="download-trend" style={{ "--trend-columns": data.length } as React.CSSProperties} role="img" aria-label={`月別の保存数。${label}`}>
      {data.map((item) => (
        <Box className="download-trend__column" key={item.key} data-empty={item.total === 0 || undefined}>
          <Text size="xs" fw={700} c="dimmed">{item.total ? formatNumber(item.total) : ""}</Text>
          <Box className="download-trend__track" aria-hidden>
            {item.total > 0 && (
              <Box className="download-trend__stack" style={{ height: `${Math.max(6, item.total / maximum * 100)}%` }}>
                {item.other > 0 && <Box className="download-trend__segment" data-source="other" style={{ flexGrow: item.other }} />}
                {item.fanbox > 0 && <Box className="download-trend__segment" data-source="fanbox" style={{ flexGrow: item.fanbox }} />}
                {item.pixiv > 0 && <Box className="download-trend__segment" data-source="pixiv" style={{ flexGrow: item.pixiv }} />}
              </Box>
            )}
          </Box>
          <Text size="xs" c="dimmed" className="download-trend__label">{item.label}</Text>
        </Box>
      ))}
    </Box>
  );
}

/**
 * With one or two months on record there is no trend to draw, so the card
 * shows what the library is actually made of instead of a single bar sitting
 * in an otherwise empty axis.
 */
function SourceComposition({ breakdown, total }: { breakdown: { source: string; count: number }[]; total: number }) {
  const items = breakdown.filter((item) => item.count > 0);
  if (!items.length) return <Alert color="gray">まだ保存された作品がありません。</Alert>;
  return (
    <Stack gap="sm">
      <Box className="source-composition" role="img" aria-label={items.map((item) => `${item.source} ${item.count}件`).join("、")}>
        {items.map((item) => (
          <Box key={item.source} className="source-composition__segment" data-source={item.source} style={{ flexGrow: item.count }} />
        ))}
      </Box>
      <Group gap="lg">
        {items.map((item) => (
          <Group key={item.source} gap={7} wrap="nowrap">
            <Box className="source-composition__dot" data-source={item.source} />
            <Text size="sm" fw={650}>{getProvider(item.source).label}</Text>
            <Text size="sm" c="dimmed">{formatNumber(item.count)}件 · {Math.round(item.count / Math.max(1, total) * 100)}%</Text>
          </Group>
        ))}
      </Group>
    </Stack>
  );
}

function TrendLegend({ color, label }: { color: string; label: string }) {
  return <Group gap={5} wrap="nowrap"><Box w={8} h={8} bdrs="xs" style={{ background: color }} /><Text size="10px" c="dimmed">{label}</Text></Group>;
}
