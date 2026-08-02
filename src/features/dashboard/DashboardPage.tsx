import { useMemo } from "react";
import {
  Alert,
  Badge,
  Box,
  Button,
  Card,
  Grid,
  Group,
  Image,
  Progress,
  SimpleGrid,
  Stack,
  Text,
  ThemeIcon,
  Title,
} from "@mantine/core";
import { useForm, isUrl } from "@mantine/form";
import { useQuery } from "@tanstack/react-query";
import {
  ArrowRight,
  Database,
  Download,
  HardDrive,
  Heart,
  RefreshCw,
  Search,
} from "lucide-react";
import piepIcon from "@/assets/icon.svg";
import { useAppNavigate } from "@/app/router";
import { WorkCard } from "@/components/WorkCard";
import { ErrorState, LoadingState } from "@/components/AsyncState";
import { RuntimeNotice } from "@/components/RuntimeNotice";
import { demoDashboard } from "@/mocks/demoData";
import { formatBytes, formatNumber } from "@/lib/format";
import { getDashboardSummary, getSearchIndexStatus, isTauriRuntime } from "@/services/dbApi";
import { store } from "@/store";

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
    validate: { url: isUrl({ protocols: ["http", "https"] }, "pixivまたはFANBOXのURLを入力してください") },
  });
  const chartData = useMemo(() => summary.data?.monthlyDownloads.map((item) => ({
    month: new Intl.DateTimeFormat("ja-JP", { month: "short" }).format(new Date(`${item.bucket}-01T00:00:00`)),
    total: item.count,
    pixiv: item.pixivCount,
    fanbox: item.fanboxCount,
    other: Math.max(0, item.count - item.pixivCount - item.fanboxCount),
  })) ?? [], [summary.data]);

  if (summary.isLoading) return <div className="page"><LoadingState label="ホームを準備しています" /></div>;
  if (summary.error || !summary.data) return <div className="page"><ErrorState error={summary.error} retry={() => summary.refetch()} /></div>;
  const data = summary.data;
  const indexProgress = index.data?.totalDownloads ? Math.round(index.data.indexedDownloads / index.data.totalDownloads * 100) : 100;

  return (
    <div className="page page--contained dashboard-page">
      <Stack gap="md">
        <RuntimeNotice />
        <Card className="dashboard-hero dashboard-quick-save" padding="lg">
          <Group wrap="nowrap" align="flex-start" gap="lg">
            <Image src={piepIcon} alt="piep" w={52} h={52} className="dashboard-quick-save__logo" />
            <Stack gap="sm" flex={1} miw={0}>
              <Group justify="space-between" align="flex-start" wrap="nowrap"><Box><Text size="sm" fw={750} c="piep.6">Webから保存</Text><Text size="sm" c="dimmed">URLを貼るだけで作品・シリーズ・作者を判定します</Text></Box><Group gap="xs" wrap="nowrap" visibleFrom="sm"><ConnectionBadge label="pixiv" connected={auth.data?.pixiv} /><ConnectionBadge label="FANBOX" connected={auth.data?.fanbox} /><Button variant="subtle" size="compact-sm" leftSection={<Download size={14} />} onClick={() => navigate("/save/pixiv")}>保存画面</Button></Group></Group>
              <form onSubmit={quickSave.onSubmit(({ url }) => navigate(`/save/${url.includes("fanbox.cc") ? "fanbox" : "pixiv"}?url=${encodeURIComponent(url)}`))}>
                <Group align="flex-start" wrap="nowrap">
                  <Box flex={1}><input key={quickSave.key("url")} className={`quick-save-input${quickSave.errors.url ? " quick-save-input--error" : ""}`} placeholder="作品ページのURLを貼り付け" aria-label="保存するページのURL" {...quickSave.getInputProps("url")} />{quickSave.errors.url && <Text size="xs" c="red" mt={4}>{quickSave.errors.url}</Text>}</Box>
                  <Button type="submit" rightSection={<ArrowRight size={15} />}>候補を開く</Button>
                </Group>
              </form>
              {(!auth.data?.pixiv || !auth.data?.fanbox) && <Button variant="subtle" size="compact-xs" w="fit-content" onClick={() => navigate("/settings")}>未接続サービスを設定</Button>}
            </Stack>
          </Group>
        </Card>

        <SimpleGrid cols={{ base: 2, md: 4 }} spacing="sm">
          <StatCard icon={Database} label="保存作品" value={formatNumber(data.stats.totalDownloads)} hint={`pixiv ${formatNumber(data.stats.pixivCount)} · FANBOX ${formatNumber(data.stats.fanboxCount)}`} color="#0d86f4" onClick={() => navigate("/library")} />
          <StatCard icon={HardDrive} label="使用容量" value={formatBytes(data.stats.totalSizeBytes)} hint={`${formatNumber(data.stats.totalAssets)} アセット`} color="#31b497" onClick={() => navigate("/settings")} />
          <StatCard icon={Heart} label="お気に入り" value={formatNumber(data.favoriteCount)} hint="すぐ読み返せる作品" color="#ef5b78" onClick={() => navigate("/library?favorite=1")} />
          <StatCard icon={RefreshCw} label="更新監視" value={formatNumber(data.watchedCount)} hint={`${formatNumber(data.updateTargetCount)} 作者・シリーズ`} color="#86b918" onClick={() => navigate("/updates")} />
        </SimpleGrid>

        <Grid gap="lg">
          <Grid.Col span={{ base: 12, lg: 8 }}>
            <Card p="lg" h="100%">
              <Group justify="space-between" mb="md"><Box><Text fw={700}>保存の推移</Text><Group gap="md" mt={3}><TrendLegend color="#0096fa" label="pixiv" /><TrendLegend color="#d1a900" label="FANBOX" /></Group></Box><Badge variant="light">{formatNumber(chartData.reduce((sum, item) => sum + item.total, 0))}件</Badge></Group>
              <DownloadTrend data={chartData} />
            </Card>
          </Grid.Col>
          <Grid.Col span={{ base: 12, lg: 4 }}>
            <Stack gap="lg" h="100%">
              <Card p="lg">
                <Group justify="space-between" mb="sm"><Group gap="xs"><Search size={17} /><Text fw={700}>検索インデックス</Text></Group><Badge color={indexProgress === 100 ? "green" : "yellow"}>{indexProgress}%</Badge></Group>
                <Progress value={indexProgress} mb="sm" aria-label={`検索インデックス ${indexProgress}%`} />
                <Text size="xs" c="dimmed">{index.data?.isComplete ? "全文・意味検索は最新です" : `${formatNumber(index.data?.pendingDownloads)}件を処理中`}</Text>
              </Card>
              <Card p="lg" flex={1}>
                <Text fw={700} mb="sm">よく使うタグ</Text>
                <Group gap="xs">{data.topTags.slice(0, 8).map((tag) => <Badge className="dashboard-tag" key={tag.name} variant="light" color="gray" component="button" onClick={() => navigate(`/library?q=${encodeURIComponent(tag.name)}`)}><span>{tag.name}</span><span className="dashboard-tag__count">{formatNumber(tag.count)}</span></Badge>)}</Group>
              </Card>
            </Stack>
          </Grid.Col>
        </Grid>

        <Box>
          <Group justify="space-between" mb="md"><Box><Title order={2}>最近の保存</Title><Text size="sm" c="dimmed">続きから読む、整理する</Text></Box><Button variant="subtle" rightSection={<ArrowRight size={15} />} onClick={() => navigate("/library")}>すべて表示</Button></Group>
          {data.recentDownloads.length ? <div className="work-grid">{data.recentDownloads.slice(0, 4).map((work) => <WorkCard key={work.id} work={work} />)}</div> : <Alert color="gray">まだ作品がありません。</Alert>}
        </Box>
      </Stack>
    </div>
  );
}

function ConnectionBadge({ label, connected }: { label: string; connected?: boolean }) {
  return <Badge color={connected ? "green" : "gray"} variant="light" leftSection={<span className="status-dot" />}>{label} {connected ? "接続済み" : "未接続"}</Badge>;
}

function StatCard({ icon: Icon, label, value, hint, color, onClick }: { icon: typeof Database; label: string; value: string; hint: string; color: string; onClick: () => void }) {
  return (
    <Card component="button" type="button" className="stat-card" style={{ "--stat-accent": color }} onClick={onClick}>
      <Group justify="space-between" align="flex-start"><Text size="sm" c="dimmed" fw={600}>{label}</Text><ThemeIcon variant="light" color="gray"><Icon size={17} style={{ color }} /></ThemeIcon></Group>
      <Text fz="20px" fw={760} mt={2} lts="-0.035em">{value}</Text>
      <Group justify="space-between" gap="xs" wrap="nowrap"><Text size="xs" c="dimmed" mt={2} className="line-clamp-1">{hint}</Text><ArrowRight className="stat-card__arrow" size={14} /></Group>
    </Card>
  );
}

function DownloadTrend({ data }: { data: { month: string; total: number; pixiv: number; fanbox: number; other: number }[] }) {
  const maximum = Math.max(1, ...data.map((item) => item.total));
  const label = data.map((item) => `${item.month} ${item.total}件（pixiv ${item.pixiv}件、FANBOX ${item.fanbox}件）`).join("、");
  return (
    <Box className="download-trend" role="img" aria-label={`月別の保存数。${label}`}>
      {data.map((item) => (
        <Box className="download-trend__column" key={item.month}>
          <Text size="xs" fw={700} c="dimmed">{formatNumber(item.total)}</Text>
          <Box className="download-trend__track" aria-hidden>
            <Box className="download-trend__stack" style={{ height: `${Math.max(6, item.total / maximum * 100)}%` }}>
              {item.other > 0 && <Box className="download-trend__segment" data-source="other" style={{ flexGrow: item.other }} />}
              {item.fanbox > 0 && <Box className="download-trend__segment" data-source="fanbox" style={{ flexGrow: item.fanbox }} />}
              {item.pixiv > 0 && <Box className="download-trend__segment" data-source="pixiv" style={{ flexGrow: item.pixiv }} />}
            </Box>
          </Box>
          <Text size="xs" c="dimmed">{item.month}</Text>
        </Box>
      ))}
    </Box>
  );
}

function TrendLegend({ color, label }: { color: string; label: string }) {
  return <Group gap={5} wrap="nowrap"><Box w={8} h={8} bdrs="xs" style={{ background: color }} /><Text size="10px" c="dimmed">{label}</Text></Group>;
}
