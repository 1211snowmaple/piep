import { useMemo, useState } from "react";
import {
  Alert,
  Anchor,
  Avatar,
  Badge,
  Box,
  Breadcrumbs,
  Button,
  Card,
  CopyButton,
  Grid,
  Group,
  Image,
  Paper,
  Stack,
  Switch,
  Tabs,
  Text,
  Timeline,
  Title,
} from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Archive, BookOpen, ExternalLink, History, Library, RefreshCw, UserRound } from "lucide-react";
import { AppLink, useAppNavigate, useRouteParams } from "@/app/router";
import { EmptyState, ErrorState, LoadingState } from "@/components/AsyncState";
import { WorkCard } from "@/components/WorkCard";
import { externalBrand, ExternalServiceMark, ProviderMark, sourceUrl } from "@/lib/providers";
import { errorMessage, formatBytes, formatDate, formatNumber } from "@/lib/format";
import { demoFacets, searchDemoWorks } from "@/mocks/demoData";
import { exportEntityZip } from "@/services/archiveApi";
import {
  getAssetUrl,
  getLatestEntityProfileJson,
  getPerson,
  getSeries,
  isTauriRuntime,
  listEntityVersions,
  listUpdateTargets,
  refreshEntityProfile,
  searchDownloadsV2,
  upsertUpdateTarget,
} from "@/services/dbApi";
import { saveDialog } from "@/services/dialogApi";
import { openExternalUrl } from "@/services/openerApi";
import { store } from "@/store";
import type { EntityVersion, PersonEntry, SeriesEntry, UpdateTarget } from "@/types/library";

export default function EntityPage({ kind }: { kind: "person" | "series" }) {
  const { source = "", sourceKey = "" } = useRouteParams(kind === "person" ? "/people/:source/:sourceKey" : "/series/:source/:sourceKey");
  const key = decodeURIComponent(sourceKey);
  const navigate = useAppNavigate();
  const runtime = isTauriRuntime();
  const queryClient = useQueryClient();
  const [tab, setTab] = useState<string | null>("works");
  const entity = useQuery({
    queryKey: ["entity", kind, source, key],
    queryFn: async (): Promise<PersonEntry | SeriesEntry> => {
      if (runtime) return kind === "person" ? getPerson<PersonEntry>(source, key) : getSeries<SeriesEntry>(source, key);
      const facet = (kind === "person" ? demoFacets.authorEntities : demoFacets.series).find((item) => item.source === source && item.sourceKey === key) ?? (kind === "person" ? demoFacets.authorEntities[0] : demoFacets.series[0]);
      const common = { id: 1, source: facet.source, sourceKey: facet.sourceKey, coverPath: null, description: facet.description ?? null, contentHash: null, currentVersion: 2, lastCheckedAt: new Date().toISOString(), lastFetchedAt: new Date().toISOString(), createdAt: new Date().toISOString(), updatedAt: new Date().toISOString(), workCount: facet.count };
      return kind === "person" ? { ...common, displayName: facet.displayName, iconPath: null, linksJson: null } : { ...common, title: facet.displayName };
    },
  });
  const works = useQuery({
    queryKey: ["entity-works", kind, source, key],
    queryFn: () => runtime ? searchDownloadsV2({ ...(kind === "person" ? { personSource: source, personKey: key } : { seriesSource: source, seriesKey: key }), limit: 200, sortBy: "source_created_at", sortOrder: "desc" }) : Promise.resolve(searchDemoWorks("", source)),
  });
  const versions = useQuery({
    queryKey: ["entity-versions", kind, source, key],
    queryFn: () => runtime ? listEntityVersions<EntityVersion[]>(kind, source, key) : Promise.resolve<EntityVersion[]>([{ id: 1, entityType: kind, source, sourceKey: key, version: 2, contentHash: null, jsonPath: "preview.json", assetCount: 1, fileSizeBytes: 24000, createdAt: new Date().toISOString(), changeSummary: "プロフィール更新" }]),
  });
  const profileJson = useQuery({
    queryKey: ["entity-json", kind, source, key],
    queryFn: () => runtime ? getLatestEntityProfileJson<Record<string, unknown>>(kind, source, key) : Promise.resolve(entity.data ?? {}),
    enabled: Boolean(entity.data),
  });
  const target = useQuery({
    queryKey: ["update-target", kind, source, key],
    queryFn: async () => (runtime ? await listUpdateTargets<UpdateTarget>(kind === "person" ? "author" : "series", false) : []).find((item) => item.source === source && item.sourceKey === key) ?? null,
  });
  const displayName = useMemo(() => entity.data ? (kind === "person" ? (entity.data as PersonEntry).displayName : (entity.data as SeriesEntry).title) : "", [entity.data, kind]);
  const refreshMutation = useMutation({
    mutationFn: async () => {
      if (!runtime) return {};
      return refreshEntityProfile({ entityType: kind, source, sourceKey: key, force: true, refreshToken: await store.get<string>("pixiv_refresh_token"), cookie: await store.get<string>("fanbox_session_id"), userAgent: await store.get<string>("fanbox_user_agent") || "Mozilla/5.0" });
    },
    onSuccess: () => { notifications.show({ color: "green", message: "プロフィールを更新しました" }); queryClient.invalidateQueries({ queryKey: ["entity", kind, source, key] }); queryClient.invalidateQueries({ queryKey: ["entity-versions", kind, source, key] }); },
    onError: (error) => notifications.show({ color: "red", title: "更新できません", message: errorMessage(error) }),
  });
  const targetMutation = useMutation({
    mutationFn: async (enabled: boolean) => {
      if (!runtime) return;
      await upsertUpdateTarget({ targetType: kind === "person" ? "author" : "series", source, sourceKey: key, displayName, enabled, metadataJson: null });
    },
    onSuccess: () => { queryClient.invalidateQueries({ queryKey: ["update-target", kind, source, key] }); notifications.show({ color: "green", message: "更新監視を変更しました" }); },
    onError: (error) => notifications.show({ color: "red", title: "更新監視を変更できません", message: errorMessage(error) }),
  });

  if (entity.isLoading) return <div className="page"><LoadingState /></div>;
  if (entity.error || !entity.data) return <div className="page"><ErrorState error={entity.error ?? "情報がありません"} retry={() => entity.refetch()} /></div>;
  const entry = entity.data;
  const profileData = (profileJson.data ?? {}) as Record<string, any>;
  const profileStats = profileData.stats as Record<string, number> | undefined;
  const coverPath = entry.coverPath;
  const avatarPath = kind === "person" ? (entry as PersonEntry).iconPath : coverPath;
  const sourceProfileUrl = sourceUrl(source, key, kind);
  const profileLinks = kind === "person" ? [...new Set([...parseLinks((entry as PersonEntry).linksJson), ...(sourceProfileUrl ? [sourceProfileUrl] : [])])] : [];
  const workItems = works.data?.items ?? [];
  const openSource = async () => {
    const url = sourceProfileUrl;
    if (!url) return;
    if (runtime) await openExternalUrl(url); else window.open(url, "_blank", "noopener,noreferrer");
  };
  const exportZip = async () => {
    if (!runtime) return notifications.show({ color: "blue", message: "書き出しはデスクトップアプリで利用できます" });
    const path = await saveDialog({ title: "アーカイブを書き出す", defaultPath: `${displayName.replace(/[\\/:*?"<>|]/g, "_")}.zip`, filters: [{ name: "ZIP archive", extensions: ["zip"] }] });
    if (!path) return;
    try { await exportEntityZip(kind, source, key, path); notifications.show({ color: "green", title: "書き出しました", message: path }); }
    catch (error) { notifications.show({ color: "red", title: "書き出しに失敗しました", message: errorMessage(error) }); }
  };

  return (
    <div className="page page--contained entity-page">
      <Breadcrumbs mb="md"><Anchor component={AppLink} to={`/library?tab=${kind === "person" ? "people" : "series"}`} size="sm">{kind === "person" ? "作者・クリエイター" : "シリーズ"}</Anchor><Text size="sm" c="dimmed">{displayName}</Text></Breadcrumbs>
      <Card className="entity-hero" data-kind={kind} padding={0}>
        {kind === "person" && coverPath && <Box className="entity-hero__banner"><Image src={getAssetUrl(coverPath)} alt={`${displayName}のヘッダー画像`} /></Box>}
        <Box className="entity-hero__body">
          <Group justify="space-between" align="flex-start" wrap="nowrap" className="entity-hero__primary">
            <Group align="flex-end" wrap="nowrap" miw={0}>
              {kind === "person"
                ? <Avatar className="entity-hero__avatar" src={getAssetUrl(avatarPath)} size={112} radius="xl" color="piep"><UserRound size={42} /></Avatar>
                : <Box className="entity-hero__series-cover">{avatarPath ? <Image src={getAssetUrl(avatarPath)} alt={`${displayName}の表紙`} fit="contain" /> : <Library size={42} />}</Box>}
              <Stack gap={7} mb={5} miw={0}><ProviderMark provider={source} /><Title order={1} className="line-clamp-2">{displayName}</Title><Group gap="xs"><Badge variant="light" color="gray">{formatNumber(entry.workCount)}作品</Badge><Text size="xs" c="dimmed">更新 {formatDate(entry.updatedAt)}</Text></Group></Stack>
            </Group>
            <Group gap="xs" className="entity-hero__actions"><Button variant="default" leftSection={<ExternalLink size={15} />} onClick={openSource}>保存元</Button><Button variant="default" leftSection={<Archive size={15} />} onClick={exportZip}>アーカイブ</Button><Button leftSection={<RefreshCw size={15} />} loading={refreshMutation.isPending} onClick={() => refreshMutation.mutate()}>情報を更新</Button></Group>
          </Group>
          <Stack gap="md" mt="lg">
            {entry.description && <Text maw={860} c="dimmed" style={{ whiteSpace: "pre-wrap" }}>{entry.description}</Text>}
            {profileLinks.length > 0 && <Group gap="xs" className="profile-links">{profileLinks.map((url) => { const brand = externalBrand(url); return <Button key={url} className="profile-link" style={{ "--profile-link-color": brand.color }} variant="default" onClick={() => runtime ? openExternalUrl(url) : window.open(url, "_blank", "noopener,noreferrer")}><ExternalServiceMark url={url} /><Text component="span" size="xs" c="dimmed" className="profile-link__value">{profileLinkValue(url)}</Text></Button>; })}</Group>}
          </Stack>
        </Box>
      </Card>

      <Grid gap="lg" mt="lg">
        <Grid.Col span={{ base: 12, lg: 9 }}>
          <Tabs value={tab} onChange={setTab}>
            <Tabs.List><Tabs.Tab value="works">作品</Tabs.Tab><Tabs.Tab value="history">プロフィール履歴</Tabs.Tab><Tabs.Tab value="json">JSON</Tabs.Tab></Tabs.List>
            <Tabs.Panel value="works" pt="lg">
              {works.isLoading ? <LoadingState /> : works.error ? <ErrorState error={works.error} retry={() => works.refetch()} /> : workItems.length ? <div className="library-grid">{workItems.map((work) => <WorkCard key={work.id} work={work} />)}</div> : <EmptyState icon={BookOpen} title="保存作品がありません" description="このページを保存ワークスペースで開き、作品を取り込んでください。" action={<Button onClick={() => navigate(`/save/${source}`)}>保存ワークスペースを開く</Button>} />}
            </Tabs.Panel>
            <Tabs.Panel value="history" pt="lg"><Paper p="lg" withBorder>{versions.isLoading ? <LoadingState /> : <Timeline active={versions.data?.length ?? 0}>{versions.data?.map((version) => <Timeline.Item key={version.id} bullet={<History size={13} />} title={`バージョン ${version.version}`}><Text size="sm" c="dimmed">{version.changeSummary || "プロフィールを保存"}</Text><Text size="xs" c="dimmed" mt={4}>{formatDate(version.createdAt, true)} · {formatBytes(version.fileSizeBytes)}</Text></Timeline.Item>)}</Timeline>}</Paper></Tabs.Panel>
            <Tabs.Panel value="json" pt="lg">{profileJson.isLoading ? <LoadingState /> : profileJson.error ? <ErrorState error={profileJson.error} retry={() => profileJson.refetch()} /> : <Stack gap="sm"><Group justify="space-between"><Text size="sm" c="dimmed">取得したプロフィール情報を折り返して表示します。</Text><CopyButton value={JSON.stringify(profileJson.data ?? {}, null, 2)}>{({ copied, copy }) => <Button size="xs" variant="light" onClick={copy}>{copied ? "コピー済み" : "コピー"}</Button>}</CopyButton></Group><Paper className="json-view" withBorder><pre>{JSON.stringify(profileJson.data ?? {}, null, 2)}</pre></Paper></Stack>}</Tabs.Panel>
          </Tabs>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 3 }}>
          <Stack gap="lg">
            <Card p="lg"><Group justify="space-between"><Box><Text fw={700}>新着を監視</Text><Text size="xs" c="dimmed" mt={4}>{kind === "person" ? "新しい作品を検出" : "シリーズの続編を検出"}</Text></Box><Switch checked={target.data?.enabled ?? false} onChange={(event) => targetMutation.mutate(event.currentTarget.checked)} aria-label="新着の更新監視" /></Group></Card>
            <Card p="lg"><Text fw={700} mb="md">プロフィール情報</Text><Stack gap="sm">{profileData.account && <Meta label="アカウント" value={`@${profileData.account}`} />}{profileStats && <><Meta label="小説" value={`${formatNumber(profileStats.totalNovels ?? 0)}作品`} /><Meta label="小説シリーズ" value={`${formatNumber(profileStats.totalNovelSeries ?? 0)}件`} /></>}{typeof profileData.sampleNovelCount === "number" && <Meta label="取得済み構成作品" value={`${formatNumber(profileData.sampleNovelCount)}件`} />}<Meta label="現在のバージョン" value={`v${entry.currentVersion}`} /><Meta label="最終取得" value={formatDate(entry.lastFetchedAt, true)} /><Meta label="最終確認" value={formatDate(entry.lastCheckedAt, true)} /></Stack></Card>
            {!runtime && <Alert color="blue">プレビューではデモプロフィールを表示しています。</Alert>}
          </Stack>
        </Grid.Col>
      </Grid>
    </div>
  );
}

function Meta({ label, value }: { label: string; value: string }) {
  return <Group justify="space-between" gap="md"><Text size="xs" c="dimmed">{label}</Text><Text size="xs" fw={650} ta="right">{value}</Text></Group>;
}

function parseLinks(raw: string | null): string[] {
  try { return (JSON.parse(raw ?? "[]") as unknown[]).filter((value): value is string => typeof value === "string" && /^https?:\/\//.test(value)); }
  catch { return []; }
}

function profileLinkValue(url: string): string {
  try {
    const parsed = new URL(url);
    const path = decodeURIComponent(parsed.pathname).replace(/^\//, "").replace(/\/$/, "");
    if (/^(x\.com|twitter\.com)$/i.test(parsed.hostname.replace(/^www\./, "")) && path) return `@${path.split("/")[0]}`;
    return path ? path.replace(/^@/, "@") : parsed.hostname.replace(/^www\./, "");
  } catch { return url; }
}
