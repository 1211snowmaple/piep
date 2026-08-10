import { useMemo } from "react";
import {
  ActionIcon,
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
  Popover,
  ScrollArea,
  Select,
  SimpleGrid,
  TextInput,
  Timeline,
  Title,
} from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Icons, IconSize } from "@/lib/icons";
import { AppLink, useAppNavigate, useAppSearchParams, useRouteParams } from "@/app/router";
import { EmptyState, ErrorState, LoadingState } from "@/components/AsyncState";
import { EntityCard } from "@/components/EntityCard";
import { ListPager, PagingModeToggle, usePageSize, usePagingMode } from "@/components/ListPager";
import { VirtualizedWorkList } from "@/features/library/VirtualizedWorkList";
import { externalBrand, ExternalServiceMark, ProviderMark, sourceUrl } from "@/lib/providers";
import { errorMessage, formatBytes, formatDate, formatNumber } from "@/lib/format";
import { demoFacets, searchDemoWorks } from "@/mocks/demoData";
import { exportEntityZip } from "@/services/archiveApi";
import {
  getAssetUrl,
  getLatestEntityProfileJson,
  getPerson,
  getSeries,
  getUpdateTarget,
  isTauriRuntime,
  listEntityVersions,
  refreshEntityProfile,
  searchDownloadsV2,
  upsertUpdateTarget,
} from "@/services/dbApi";
import { listEntitySeries, listEntityTags } from "@/services/shelfApi";
import { saveDialog } from "@/services/dialogApi";
import { openExternalUrl } from "@/services/openerApi";
import { store } from "@/store";
import type { EntityFacet, EntityVersion, FacetCount, LibrarySortBy, PersonEntry, SeriesEntry, UpdateTarget } from "@/types/library";

/**
 * Every option says both what it sorts by and which way round.
 *
 * "タイトル" alone does not tell you whether あ or ん comes first, and "文字数"
 * does not say whether the long ones are at the top. Naming the direction in
 * the option removes the guess.
 */
const ENTITY_SORT_OPTIONS: { value: LibrarySortBy; label: string }[] = [
  { value: "source_created_at", label: "公開が新しい順" },
  { value: "downloaded_at", label: "保存が新しい順" },
  { value: "title", label: "タイトル昇順（あ→ん）" },
  { value: "text_length", label: "文字数が多い順" },
  { value: "series_order", label: "シリーズの話数順" },
];

/** Chips for the tags this author actually uses, with the rest behind a popover. */
const VISIBLE_TAG_CHIPS = 10;

export default function EntityPage({ kind }: { kind: "person" | "series" }) {
  const { source = "", sourceKey = "" } = useRouteParams(kind === "person" ? "/people/:source/:sourceKey" : "/series/:source/:sourceKey");
  // Route params are decoded by the router. Decoding a second time crashes on
  // a valid literal '%' and corrupts keys containing encoded delimiters.
  const key = sourceKey;
  const navigate = useAppNavigate();
  const runtime = isTauriRuntime();
  const queryClient = useQueryClient();
  const [urlParams, setUrlParams] = useAppSearchParams();
  const tab = parseEntityTab(urlParams.get("tab"), kind);
  const setTab = (value: string | null) => {
    const next = new URLSearchParams(urlParams);
    if (value && value !== "works") next.set("tab", value); else next.delete("tab");
    setUrlParams(next, { replace: true });
  };
  // Narrowing lives in the URL so the back button steps back through it and a
  // filtered view of an author can be linked to.
  const workQuery = (urlParams.get("q") ?? "").slice(0, 200);
  const activeTags = urlParams.getAll("tag").slice(0, 20);
  const sortBy = parseEntitySort(urlParams.get("sort"));
  const patchUrl = (patch: { q?: string; tags?: string[]; sort?: LibrarySortBy }) => {
    const next = new URLSearchParams(urlParams);
    if (patch.q !== undefined) { if (patch.q) next.set("q", patch.q); else next.delete("q"); }
    if (patch.tags !== undefined) {
      next.delete("tag");
      patch.tags.forEach((tag) => next.append("tag", tag));
    }
    if (patch.sort !== undefined) { if (patch.sort !== "source_created_at") next.set("sort", patch.sort); else next.delete("sort"); }
    // Page 7 of one filter is not page 7 of another.
    next.delete("page");
    setUrlParams(next, { replace: true });
  };
  const [pagingMode] = usePagingMode();
  const [pageSize] = usePageSize();
  const pageParam = Math.max(1, Number.parseInt(urlParams.get("page") ?? "1", 10) || 1);
  const setPage = (page: number) => {
    const next = new URLSearchParams(urlParams);
    if (page > 1) next.set("page", String(page)); else next.delete("page");
    setUrlParams(next, { replace: false });
  };
  const toggleTag = (tag: string) => patchUrl({
    tags: activeTags.includes(tag) ? activeTags.filter((item) => item !== tag) : [...activeTags, tag],
  });
  const entity = useQuery({
    queryKey: ["entity", kind, source, key],
    queryFn: async (): Promise<PersonEntry | SeriesEntry> => {
      if (runtime) return kind === "person" ? getPerson<PersonEntry>(source, key) : getSeries<SeriesEntry>(source, key);
      const facet = (kind === "person" ? demoFacets.authorEntities : demoFacets.series).find((item) => item.source === source && item.sourceKey === key) ?? (kind === "person" ? demoFacets.authorEntities[0] : demoFacets.series[0]);
      const common = { id: 1, source: facet.source, sourceKey: facet.sourceKey, coverPath: null, description: facet.description ?? null, contentHash: null, currentVersion: 2, lastCheckedAt: new Date().toISOString(), lastFetchedAt: new Date().toISOString(), createdAt: new Date().toISOString(), updatedAt: new Date().toISOString(), workCount: facet.count };
      return kind === "person" ? { ...common, displayName: facet.displayName, iconPath: null, linksJson: null } : { ...common, title: facet.displayName };
    },
  });
  // Relevance is walked with a score cursor and has no nth page, so numbers are
  // only offered once an ordering has been chosen.
  const searchingByRelevance = Boolean(workQuery) && sortBy === "source_created_at";
  const numberedPages = pagingMode === "pages" && !searchingByRelevance;
  // Prolific authors have more works than any single request should return, so
  // this pages through them instead of silently stopping at a fixed slab.
  const works = useInfiniteQuery({
    queryKey: ["entity-works", kind, source, key, workQuery, activeTags, sortBy, pageSize, numberedPages ? pageParam : 0],
    queryFn: ({ pageParam: cursor }) => runtime
      ? searchDownloadsV2({
        ...(kind === "person" ? { personSource: source, personKey: key } : { seriesSource: source, seriesKey: key }),
        text: workQuery || null,
        tagsInclude: activeTags.length ? activeTags : null,
        limit: pageSize,
        cursor,
        offset: numberedPages ? (pageParam - 1) * pageSize : null,
        // A text search inside an author ranks by relevance unless a column was
        // asked for, matching how the library itself behaves.
        sortBy: workQuery && sortBy === "source_created_at" ? "relevance" : sortBy,
        sortOrder: sortBy === "title" ? "asc" : "desc",
        projection: "libraryGallery",
      })
      : Promise.resolve(searchDemoWorks(workQuery, source)),
    initialPageParam: null as string | null,
    getNextPageParam: (lastPage, allPages) => {
      const cursor = lastPage.items.length ? lastPage.nextCursor : null;
      return cursor && !allPages.slice(0, -1).some((page) => page.nextCursor === cursor) ? cursor : undefined;
    },
    enabled: tab === "works",
  });
  // What this author writes, and who they write it with: the two things a flat
  // list of works cannot tell you.
  const authorSeries = useQuery({
    queryKey: ["entity-series", source, key],
    queryFn: () => runtime ? listEntitySeries(source, key) : Promise.resolve([] as EntityFacet[]),
    enabled: kind === "person",
    staleTime: 5 * 60_000,
  });
  const entityTags = useQuery({
    queryKey: ["entity-tags", kind, source, key],
    queryFn: () => runtime ? listEntityTags(kind, source, key) : Promise.resolve([] as FacetCount[]),
    staleTime: 5 * 60_000,
  });
  const versions = useQuery({
    queryKey: ["entity-versions", kind, source, key],
    queryFn: () => runtime ? listEntityVersions<EntityVersion[]>(kind, source, key) : Promise.resolve<EntityVersion[]>([{ id: 1, entityType: kind, source, sourceKey: key, version: 2, contentHash: null, jsonPath: "preview.json", assetCount: 1, fileSizeBytes: 24000, createdAt: new Date().toISOString(), changeSummary: "プロフィール更新" }]),
    enabled: tab === "history",
  });
  const profileJson = useQuery({
    queryKey: ["entity-json", kind, source, key],
    queryFn: () => runtime ? getLatestEntityProfileJson<Record<string, unknown>>(kind, source, key) : Promise.resolve(entity.data ?? {}),
    enabled: Boolean(entity.data),
  });
  const target = useQuery({
    queryKey: ["update-target", kind, source, key],
    queryFn: () => runtime ? getUpdateTarget<UpdateTarget>(kind === "person" ? "author" : "series", source, key) : Promise.resolve(null),
  });
  const displayName = useMemo(() => entity.data ? (kind === "person" ? (entity.data as PersonEntry).displayName : (entity.data as SeriesEntry).title) : "", [entity.data, kind]);
  const refreshMutation = useMutation({
    mutationFn: async () => {
      if (!runtime) return {};
      return refreshEntityProfile({ entityType: kind, source, sourceKey: key, force: true, refreshToken: await store.get<string>("pixiv_refresh_token"), cookie: await store.get<string>("fanbox_session_id"), userAgent: await store.get<string>("fanbox_user_agent") || "Mozilla/5.0" });
    },
    onSuccess: () => { notifications.show({ color: "green", message: "プロフィールを更新しました" }); queryClient.invalidateQueries({ queryKey: ["entity", kind, source, key] }); queryClient.invalidateQueries({ queryKey: ["entity-versions", kind, source, key] }); queryClient.invalidateQueries({ queryKey: ["entity-json", kind, source, key] }); },
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
  const workItems = useMemo(() => {
    const seen = new Set<number>();
    return works.data?.pages.flatMap((page) => page.items.filter((work) => {
      if (seen.has(work.id)) return false;
      seen.add(work.id);
      return true;
    })) ?? [];
  }, [works.data?.pages]);

  if (entity.isLoading) return <div className="page"><LoadingState /></div>;
  if (entity.error || !entity.data) return <div className="page"><ErrorState error={entity.error ?? "情報がありません"} retry={() => entity.refetch()} /></div>;
  const entry = entity.data;
  const profileData = (profileJson.data ?? {}) as Record<string, any>;
  const profileStats = profileData.stats as Record<string, number> | undefined;
  const coverPath = entry.coverPath;
  const avatarPath = kind === "person" ? (entry as PersonEntry).iconPath : coverPath;
  const sourceProfileUrl = sourceUrl(source, key, kind);
  const profileLinks = kind === "person" ? profileLinkList(parseLinks((entry as PersonEntry).linksJson), sourceProfileUrl) : [];
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
                ? <Avatar className="entity-hero__avatar" src={getAssetUrl(avatarPath)} size={112} radius="xl" color="piep"><Icons.person size={IconSize.avatar} /></Avatar>
                : <Box className="entity-hero__series-cover">{avatarPath ? <Image src={getAssetUrl(avatarPath)} alt={`${displayName}の表紙`} fit="contain" /> : <Icons.series size={IconSize.avatar} />}</Box>}
              <Stack gap={7} mb={5} miw={0} align="flex-start"><ProviderMark provider={source} /><Title order={1} className="line-clamp-2">{displayName}</Title><Group gap="xs"><Badge variant="light" color="gray">{formatNumber(entry.workCount)}作品</Badge><Text size="xs" c="dimmed">更新 {formatDate(entry.updatedAt)}</Text></Group></Stack>
            </Group>
            <Group gap="xs" className="entity-hero__actions"><Button variant="default" leftSection={<Icons.externalLink size={IconSize.menu} />} onClick={openSource}>元ページを開く</Button><Button variant="default" leftSection={<Icons.archive size={IconSize.menu} />} onClick={exportZip}>アーカイブ</Button><Button leftSection={<Icons.watch size={IconSize.menu} />} loading={refreshMutation.isPending} onClick={() => refreshMutation.mutate()}>情報を更新</Button></Group>
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
            <Tabs.List>
              <Tabs.Tab value="works" leftSection={<Icons.read size={IconSize.inline} />}>作品</Tabs.Tab>
              {kind === "person" && <Tabs.Tab value="series" leftSection={<Icons.series size={IconSize.inline} />} rightSection={authorSeries.data?.length ? <Badge size="xs" variant="light">{formatNumber(authorSeries.data.length)}</Badge> : undefined}>シリーズ</Tabs.Tab>}
              <Tabs.Tab value="history">プロフィール履歴</Tabs.Tab>
              <Tabs.Tab value="json">JSON</Tabs.Tab>
            </Tabs.List>
            <Tabs.Panel value="works" pt="lg">
              <Stack gap="sm">
                <Group gap="sm" wrap="nowrap" align="center">
                  <TextInput
                    flex={1}
                    size="sm"
                    value={workQuery}
                    onChange={(event) => patchUrl({ q: event.currentTarget.value })}
                    leftSection={<Icons.search size={IconSize.menu} />}
                    rightSection={workQuery ? <ActionIcon variant="subtle" color="gray" size="sm" aria-label="検索語を消す" onClick={() => patchUrl({ q: "" })}><Icons.cancel size={IconSize.inline} /></ActionIcon> : null}
                    placeholder={`${displayName}の作品を検索`}
                    aria-label={`${displayName}の作品を検索`}
                    maxLength={200}
                  />
                  <Select
                    size="sm"
                    w={188}
                    value={sortBy}
                    onChange={(value) => patchUrl({ sort: parseEntitySort(value) })}
                    data={ENTITY_SORT_OPTIONS}
                    leftSection={<Icons.sort size={IconSize.menu} />}
                    aria-label="並び順"
                    allowDeselect={false}
                  />
                </Group>
                {(entityTags.data?.length ?? 0) > 0 && (
                  <TagFilterBar
                    tags={entityTags.data ?? []}
                    active={activeTags}
                    onToggle={toggleTag}
                    onClear={() => patchUrl({ tags: [] })}
                  />
                )}
              </Stack>
              <Box mt="md">
                {works.isLoading ? <LoadingState /> : works.error ? <ErrorState error={works.error} retry={() => works.refetch()} /> : workItems.length ? (
                  <>
                    <Group gap="xs" mb="sm">
                      <Text size="sm" c="dimmed">{formatNumber(works.data?.pages[0]?.totalEstimate ?? workItems.length)}件</Text>
                      {/* Beside the count, which is the thing it changes. */}
                      <PagingModeToggle />
                    </Group>
                    <VirtualizedWorkList items={workItems} view="gallery" />
                    <ListPager
                      hasNext={works.hasNextPage}
                      loading={works.isFetchingNextPage || works.isFetching}
                      loaded={workItems.length}
                      total={works.data?.pages[0]?.totalEstimate ?? null}
                      onLoad={() => works.fetchNextPage()}
                      pages={{
                        current: pageParam,
                        size: pageSize,
                        onGoTo: setPage,
                        unavailableReason: searchingByRelevance
                          ? "関連度順はページ番号で移動できません。並び順を選ぶとページ番号が使えます。"
                          : null,
                      }}
                    />
                  </>
                ) : workQuery || activeTags.length ? (
                  <EmptyState
                    icon={Icons.search}
                    title="条件に合う作品がありません"
                    description="検索語やタグを減らしてください。"
                    action={<Button variant="light" onClick={() => patchUrl({ q: "", tags: [] })}>絞り込みを解除</Button>}
                  />
                ) : <EmptyState icon={Icons.read} title="保存作品がありません" description="このページを保存ワークスペースで開き、作品を取り込んでください。" action={<Button onClick={() => navigate(`/save/${source}`)}>保存ワークスペースを開く</Button>} />}
              </Box>
            </Tabs.Panel>
            {kind === "person" && (
              <Tabs.Panel value="series" pt="lg">
                {authorSeries.isLoading ? <LoadingState /> : authorSeries.error ? <ErrorState error={authorSeries.error} retry={() => authorSeries.refetch()} /> : (authorSeries.data?.length ?? 0) > 0 ? (
                  <SimpleGrid cols={{ base: 1, sm: 2, xl: 3 }}>
                    {authorSeries.data?.map((item) => <EntityCard key={`${item.source}:${item.sourceKey}`} entity={item} kind="series" />)}
                  </SimpleGrid>
                ) : <EmptyState icon={Icons.series} title="シリーズはありません" description="この作者の保存済み作品は、どのシリーズにも属していません。" />}
              </Tabs.Panel>
            )}
            <Tabs.Panel value="history" pt="lg"><Paper p="lg" withBorder>{versions.isLoading ? <LoadingState /> : <Timeline active={versions.data?.length ?? 0}>{versions.data?.map((version) => <Timeline.Item key={version.id} bullet={<Icons.versionHistory size={IconSize.inline} />} title={`バージョン ${version.version}`}><Text size="sm" c="dimmed">{version.changeSummary || "プロフィールを保存"}</Text><Text size="xs" c="dimmed" mt={4}>{formatDate(version.createdAt, true)} · {formatBytes(version.fileSizeBytes)}</Text></Timeline.Item>)}</Timeline>}</Paper></Tabs.Panel>
            <Tabs.Panel value="json" pt="lg">{profileJson.isLoading ? <LoadingState /> : profileJson.error ? <ErrorState error={profileJson.error} retry={() => profileJson.refetch()} /> : <Stack gap="sm"><Group justify="space-between"><Text size="sm" c="dimmed">取得したプロフィール情報を折り返して表示します。</Text><CopyButton value={JSON.stringify(profileJson.data ?? {}, null, 2)}>{({ copied, copy }) => <Button size="xs" variant="light" onClick={copy}>{copied ? "コピー済み" : "コピー"}</Button>}</CopyButton></Group><Paper className="json-view" withBorder><pre>{JSON.stringify(profileJson.data ?? {}, null, 2)}</pre></Paper></Stack>}</Tabs.Panel>
          </Tabs>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 3 }}>
          <Stack gap="lg">
            <Card p="lg"><Group justify="space-between"><Box><Text fw={700}>新着を監視</Text><Text size="xs" c="dimmed" mt={4}>{kind === "person" ? "新しい作品を検出" : "シリーズの続編を検出"}</Text></Box><Switch checked={targetMutation.isPending ? targetMutation.variables : target.data?.enabled ?? false} disabled={targetMutation.isPending || target.isLoading} onChange={(event) => targetMutation.mutate(event.currentTarget.checked)} aria-label="新着の更新監視" /></Group></Card>
            <Card p="lg"><Text fw={700} mb="md">プロフィール情報</Text><Stack gap="sm">{profileData.account && <Meta label="アカウント" value={`@${profileData.account}`} />}{profileStats && <><Meta label="小説" value={`${formatNumber(profileStats.totalNovels ?? 0)}作品`} /><Meta label="小説シリーズ" value={`${formatNumber(profileStats.totalNovelSeries ?? 0)}件`} /></>}{typeof profileData.sampleNovelCount === "number" && <Meta label="取得済み構成作品" value={`${formatNumber(profileData.sampleNovelCount)}件`} />}<Meta label="現在のバージョン" value={`v${entry.currentVersion}`} /><Meta label="最終取得" value={formatDate(entry.lastFetchedAt, true)} /><Meta label="最終確認" value={formatDate(entry.lastCheckedAt, true)} /></Stack></Card>
            {!runtime && <Alert color="blue">プレビューではデモプロフィールを表示しています。</Alert>}
          </Stack>
        </Grid.Col>
      </Grid>
    </div>
  );
}

function parseEntityTab(value: string | null, kind: "person" | "series"): "works" | "series" | "history" | "json" {
  if (value === "history" || value === "json") return value;
  if (value === "series" && kind === "person") return "series";
  return "works";
}

function parseEntitySort(value: unknown): LibrarySortBy {
  const allowed = new Set(ENTITY_SORT_OPTIONS.map((option) => option.value));
  return typeof value === "string" && allowed.has(value as LibrarySortBy)
    ? value as LibrarySortBy
    : "source_created_at";
}

/**
 * The tags this author uses, as filters rather than decoration.
 *
 * A prolific author's page is unusable as one flat list; their own tags are the
 * dimension that actually divides their work, so they narrow it in place
 * instead of sending the reader off to the library with a query.
 */
function TagFilterBar({ tags, active, onToggle, onClear }: {
  tags: FacetCount[];
  active: string[];
  onToggle: (tag: string) => void;
  onClear: () => void;
}) {
  // A selected tag stays visible even when it is not among the most used, or
  // the control that switched it on would vanish under the reader's cursor.
  const visible = tags.slice(0, VISIBLE_TAG_CHIPS);
  const shown = [...visible, ...tags.filter((tag) => active.includes(tag.name) && !visible.includes(tag))];
  const rest = tags.filter((tag) => !shown.includes(tag));
  return (
    <Group gap={6}>
      <Icons.tag size={IconSize.inline} aria-hidden style={{ opacity: 0.6 }} />
      {shown.map((tag) => (
        <Badge
          key={tag.name}
          component="button"
          type="button"
          variant={active.includes(tag.name) ? "filled" : "light"}
          color={active.includes(tag.name) ? "piep" : "gray"}
          style={{ cursor: "pointer", textTransform: "none" }}
          rightSection={<Text component="span" size="9px" opacity={0.75}>{formatNumber(tag.count)}</Text>}
          onClick={() => onToggle(tag.name)}
          aria-pressed={active.includes(tag.name)}
        >
          {tag.name}
        </Badge>
      ))}
      {rest.length > 0 && (
        <Popover width={300} position="bottom-start" withArrow shadow="md">
          <Popover.Target>
            <Badge variant="default" style={{ cursor: "pointer", textTransform: "none" }}>他{formatNumber(rest.length)}件</Badge>
          </Popover.Target>
          <Popover.Dropdown>
            <ScrollArea.Autosize mah={260}>
              <Group gap={6}>
                {rest.map((tag) => (
                  <Badge
                    key={tag.name}
                    component="button"
                    type="button"
                    variant="light"
                    color="gray"
                    style={{ cursor: "pointer", textTransform: "none" }}
                    rightSection={<Text component="span" size="9px" opacity={0.75}>{formatNumber(tag.count)}</Text>}
                    onClick={() => onToggle(tag.name)}
                  >
                    {tag.name}
                  </Badge>
                ))}
              </Group>
            </ScrollArea.Autosize>
          </Popover.Dropdown>
        </Popover>
      )}
      {active.length > 0 && <Button size="compact-xs" variant="subtle" color="gray" onClick={onClear}>タグを解除</Button>}
    </Group>
  );
}

function Meta({ label, value }: { label: string; value: string }) {
  return <Group justify="space-between" gap="md"><Text size="xs" c="dimmed">{label}</Text><Text size="xs" fw={650} ta="right">{value}</Text></Group>;
}

/**
 * Trims the prose a profile link was extracted from.
 *
 * Profiles write links inline - `twitter【https://twitter.com/name】※進捗報告`
 * - and the extractor keeps everything up to the next space, so the bracket and
 * the sentence after it end up inside the URL. The same account then appears
 * twice, once clean and once with a tail, because the two strings differ.
 */
export function cleanProfileLink(raw: string): string | null {
  const match = /^https?:\/\/\S+/.exec(raw.trim());
  if (!match) return null;
  // Cut at the first character that cannot be part of a URL. Full-width
  // brackets and CJK text are prose that ran into it.
  let url = match[0].split(/[】【〕〔）（＞＜「」『』、。※\s]/)[0];
  // A closing bracket or sentence punctuation at the very end is never part of
  // the address either.
  url = url.replace(/[)\]}>,.;:!?'"｝］〉》]+$/, "");
  try {
    const parsed = new URL(url);
    if (!/^https?:$/.test(parsed.protocol)) return null;
    return parsed.toString();
  } catch {
    return null;
  }
}

/** Two links are the same account however they were written down. */
function profileLinkIdentity(url: string): string {
  try {
    const parsed = new URL(url);
    const host = parsed.hostname.replace(/^www\./, "").toLowerCase();
    const path = parsed.pathname.replace(/\/+$/, "").toLowerCase();
    return `${host}${path}`;
  } catch {
    return url.toLowerCase();
  }
}

/** Distinct profile links, cleaned of the prose they were written in. */
export function profileLinkList(rawLinks: string[], extra: string | null): string[] {
  const seen = new Map<string, string>();
  for (const candidate of [...rawLinks, ...(extra ? [extra] : [])]) {
    const url = cleanProfileLink(candidate);
    if (!url) continue;
    const identity = profileLinkIdentity(url);
    // Keep the first, shortest form: a later copy is the one with a tail.
    const existing = seen.get(identity);
    if (!existing || url.length < existing.length) seen.set(identity, url);
  }
  return [...seen.values()];
}

function parseLinks(raw: string | null): string[] {
  try { return (JSON.parse(raw ?? "[]") as unknown[]).filter((value): value is string => typeof value === "string"); }
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
