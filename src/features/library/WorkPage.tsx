import { useEffect, useMemo, useRef, useState } from "react";
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
  Divider,
  Grid,
  Group,
  Image,
  Menu,
  Pagination,
  Paper,
  SimpleGrid,
  Stack,
  Switch,
  Tabs,
  Text,
  ThemeIcon,
  Title,
  Tooltip,
} from "@mantine/core";
import { modals } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Icons, IconSize } from "@/lib/icons";
import { Note } from "@/components/Note";
import { runSingleCheck } from "@/features/updates/startSingleCheck";
import { AppLink, useAppNavigate, useAppSearchParams, useReturnTo, useRouteParams } from "@/app/router";
import { useWorkspace } from "@/app/WorkspaceContext";
import { ErrorState, LoadingState } from "@/components/AsyncState";
import { ExpandableText } from "@/components/ExpandableText";
import { WorkCover } from "@/components/WorkCover";
import { ProviderMark, sourceUrl } from "@/lib/providers";
import { contentTypeLabel, errorMessage, formatBytes, formatDate, formatNumber } from "@/lib/format";
import { prepareDocumentHtml } from "@/lib/content";
import { useContentLinkNavigation } from "@/lib/contentLinks";
import { exportSingle } from "@/services/archiveApi";
import { openSingleDialog } from "@/services/dialogApi";
import {
  deleteDownload,
  getAssetUrl,
  getAssets,
  getReaderContentPage,
  getReaderMetadata,
  isTauriRuntime,
  openLocalAsset,
  readFileContent,
  setFavorite,
  setWatchUpdates,
} from "@/services/dbApi";
import { openExternalUrl, revealPathInFileManager } from "@/services/openerApi";
import { getDemoReader } from "@/mocks/demoData";
import { deleteThenCleanup } from "@/features/library/deletedWorkCleanup";
import type { ReaderMetadata } from "@/types/library";

type WorkTab = "overview" | "content" | "assets" | "history" | "json";

function parseWorkTab(value: string | null): WorkTab {
  return value === "content" || value === "assets" || value === "history" || value === "json" ? value : "overview";
}

export default function WorkPage() {
  const navigate = useAppNavigate();
  const returnTo = useReturnTo();
  // Links inside a caption or the body often name something already in the
  // library; those open the screen for it rather than a browser.
  const openContentLink = useContentLinkNavigation();
  const { workId } = useRouteParams("/works/:workId");
  const id = Number(workId);
  const validId = Number.isSafeInteger(id) && id > 0;
  const runtime = isTauriRuntime();
  const queryClient = useQueryClient();
  const { addToEpubQueue, removeFromEpubQueue, isQueuedForEpub } = useWorkspace();
  // Driven by the URL so the card's version chip can deep-link into the
  // history, and so going back returns to the tab you were on.
  const [searchParams, setSearchParams] = useAppSearchParams();
  const tab = parseWorkTab(searchParams.get("tab"));
  const setTab = (next: string | null) => {
    const params = new URLSearchParams(searchParams);
    const normalized = parseWorkTab(next);
    if (normalized === "overview") params.delete("tab"); else params.set("tab", normalized);
    setSearchParams(params, { replace: true });
  };
  // Switching tabs does not move the page.
  const [contentPage, setContentPage] = useState(1);
  // The body scrolls inside its own frame, so turning a page returns that frame
  // to its top and leaves the screen around it exactly where it was - the same
  // thing the reader does with its own pages.
  const contentFrameRef = useRef<HTMLDivElement>(null);
  const goToContentPage = (page: number) => {
    setContentPage(page);
    contentFrameRef.current?.scrollTo({ top: 0, behavior: "smooth" });
  };
  const [selectedVersion, setSelectedVersion] = useState<number | null>(null);
  const [visibleAssetCount, setVisibleAssetCount] = useState(80);
  useEffect(() => setVisibleAssetCount(80), [id]);
  const documentQuery = useQuery({
    queryKey: ["reader-metadata", id],
    queryFn: () => runtime ? getReaderMetadata(id) : Promise.resolve((() => { const demo = getDemoReader(id); return { download: demo.download, versions: demo.versions, assetCount: demo.assets.length, isEdited: demo.isEdited, activeEditRevision: demo.activeEditRevision }; })()),
    enabled: validId,
  });
  const assetsQuery = useQuery({
    queryKey: ["work-assets", id],
    queryFn: () => runtime ? getAssets(id) : Promise.resolve(getDemoReader(id).assets),
    enabled: validId && tab === "assets",
  });
  const contentQuery = useQuery({
    queryKey: ["reader-content-page", id, null, contentPage - 1],
    queryFn: () => runtime ? getReaderContentPage(id, null, contentPage - 1) : Promise.resolve((() => { const demo = getDemoReader(id); return { page: 0, pageCount: 1, html: demo.html, plainText: demo.plainText, totalPlainTextChars: demo.plainText.length }; })()),
    enabled: validId && tab === "content",
    placeholderData: (previous) => previous,
  });
  useEffect(() => {
    if (contentQuery.data?.page !== contentPage - 1) return;
    queryClient.removeQueries({
      queryKey: ["reader-content-page", id, null],
      type: "inactive",
      predicate: (query) => query.queryKey[3] !== contentPage - 1,
    });
  }, [contentPage, contentQuery.data?.page, id, queryClient]);
  const rawJson = useQuery({
    queryKey: ["work-json", id, documentQuery.data?.download.jsonPath],
    queryFn: () => runtime && documentQuery.data ? readFileContent(documentQuery.data.download.jsonPath) : Promise.resolve(JSON.stringify(documentQuery.data?.download ?? {}, null, 2)),
    enabled: tab === "json" && Boolean(documentQuery.data),
  });
  const versionPreview = useQuery({
    queryKey: ["reader-content-page", id, selectedVersion, 0],
    queryFn: () => runtime ? getReaderContentPage(id, selectedVersion, 0) : Promise.resolve((() => { const demo = getDemoReader(id); return { page: 0, pageCount: 1, html: demo.html, plainText: demo.plainText, totalPlainTextChars: demo.plainText.length }; })()),
    enabled: validId && tab === "history" && selectedVersion !== null,
  });
  const mutate = useMutation({
    mutationFn: async (input: { favorite?: boolean; watch?: boolean }) => {
      if (!runtime) return;
      if (typeof input.favorite === "boolean") await setFavorite(id, input.favorite);
      if (typeof input.watch === "boolean") await setWatchUpdates(id, input.watch);
    },
    onMutate: (input) => {
      const key = ["reader-metadata", id] as const;
      const previous = queryClient.getQueryData<ReaderMetadata>(key);
      if (previous) queryClient.setQueryData<ReaderMetadata>(key, {
        ...previous,
        download: {
          ...previous.download,
          ...(typeof input.favorite === "boolean" ? { favorite: input.favorite } : {}),
          ...(typeof input.watch === "boolean" ? { watchUpdates: input.watch } : {}),
        },
      });
      return { key, previous };
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["library"], refetchType: "none" });
      queryClient.invalidateQueries({ queryKey: ["dashboard"] });
    },
    onError: (error, _input, context) => {
      if (context?.previous) queryClient.setQueryData(context.key, context.previous);
      notifications.show({ color: "red", title: "変更できません", message: errorMessage(error) });
    },
  });
  // Sanitising and page-splitting a very large novel builds multiple DOM trees.
  // Defer that work until the user actually opens the content tab.
  const contentReady = !contentQuery.isPlaceholderData && contentQuery.data?.page === contentPage - 1;
  const preparedHtml = useMemo(() => tab === "content" && contentReady ? prepareDocumentHtml(contentQuery.data?.html ?? "", getAssetUrl) : "", [contentQuery.data?.html, contentReady, tab]);
  // Captions are mostly links - the series, the earlier part, the author's
  // other accounts - so they are set as text rather than as a stack of cards
  // taller than the caption itself.
  const captionHtml = useMemo(() => prepareDocumentHtml(documentQuery.data?.download.excerpt ?? "", getAssetUrl, { inlineLinks: true }), [documentQuery.data?.download.excerpt]);
  useEffect(() => { if (tab !== "content") setContentPage(1); }, [id, tab]);

  if (documentQuery.isLoading) return <div className="page"><LoadingState label="作品を開いています" /></div>;
  if (documentQuery.error || !documentQuery.data) return <div className="page"><ErrorState error={documentQuery.error ?? "作品が見つかりません"} retry={() => documentQuery.refetch()} /></div>;
  const doc = documentQuery.data;
  const work = doc.download;
  const assets = assetsQuery.data ?? [];
  const visibleAssets = assets.slice(0, visibleAssetCount);
  const queued = isQueuedForEpub(work.id);

  const deleteWork = () => modals.openConfirmModal({
    title: "この作品を削除しますか？",
    children: <Text size="sm">「{work.title}」と保存済みアセットをローカルライブラリから削除します。この操作は元に戻せません。</Text>,
    labels: { confirm: "削除する", cancel: "キャンセル" }, confirmProps: { color: "red" },
    onConfirm: async () => {
      try {
        await deleteThenCleanup(
          () => runtime ? deleteDownload(work.id) : Promise.resolve(),
          { queryClient, ids: [work.id], removeFromEpubQueue },
        );
        navigate("/library");
        notifications.show({ color: "green", message: "作品を削除しました" });
      }
      catch (error) { notifications.show({ color: "red", title: "削除できません", message: errorMessage(error) }); }
    },
  });
  const exportArchive = async () => {
    if (!runtime) return notifications.show({ color: "piep", message: "アーカイブ出力はデスクトップアプリで利用できます" });
    const dir = await openSingleDialog({ directory: true, title: "アーカイブの出力先" });
    if (!dir) return;
    try { const path = await exportSingle(work.id, dir); notifications.show({ color: "green", title: "アーカイブを書き出しました", message: path }); }
    catch (error) { notifications.show({ color: "red", title: "書き出しに失敗しました", message: errorMessage(error) }); }
  };
  const openSavedFolder = async () => {
    if (!runtime) return notifications.show({ color: "piep", message: "保存フォルダーはデスクトップアプリで開けます" });
    try { await revealPathInFileManager(work.jsonPath); }
    catch (error) { notifications.show({ color: "red", title: "保存フォルダーを開けません", message: errorMessage(error) }); }
  };
  const openSource = async () => {
    const url = sourceUrl(work.source, work.sourceId, work.contentType, work.personId || work.authorId);
    if (!url) return;
    if (runtime) await openExternalUrl(url); else window.open(url, "_blank", "noopener,noreferrer");
  };
  // 内蔵ブラウザで開けば、その場から保存にも進める。外部ブラウザとの違いを
  // 名前で示すため、メニューは2項目に分けている。
  const openSourceInApp = () => {
    const url = sourceUrl(work.source, work.sourceId, work.contentType, work.personId || work.authorId);
    if (url) navigate(`/save/${work.source}?url=${encodeURIComponent(url)}`);
  };
  const previewAsset = (asset: (typeof assets)[number]) => {
    const url = getAssetUrl(asset.localPath);
    if (!url || !asset.mimeType?.startsWith("image/")) return;
    modals.open({
      title: asset.filename,
      size: "min(92vw, 1100px)",
      // The scroll area squared off the right edge of the dialog; keeping the
      // radius on the body and clipping it keeps all four corners round.
      radius: "lg",
      classNames: { content: "asset-preview-modal" },
      children: <Stack><Image src={url} alt={asset.filename} mah="76vh" fit="contain" /><Group justify="space-between"><Text size="xs" c="dimmed">{formatBytes(asset.fileSizeBytes)} · {asset.mimeType}</Text><Button size="xs" variant="light" onClick={() => runtime && openLocalAsset(asset.localPath)}>この端末のJSONを開く</Button></Group></Stack>,
    });
  };

  return (
    <div className="page page--contained work-page">
      {/* Goes back to the library rather than opening a fresh one, so the search,
          the filters and the row this work was opened from are still there. */}
      <Breadcrumbs mb="md" className="work-breadcrumbs"><Anchor component="button" type="button" size="sm" onClick={() => returnTo("/library")}>ライブラリ</Anchor><Text size="sm" c="dimmed" className="line-clamp-1">{work.title}</Text></Breadcrumbs>
      <Card className="work-hero" padding={0}>
        <Grid gap={0} align="stretch">
          <Grid.Col span={{ base: 12, sm: 4, lg: 3 }}>
            <Box className="work-hero__cover-stage">
              <WorkCover work={work} variant="detail" className="work-hero__cover" />
            </Box>
          </Grid.Col>
          <Grid.Col span={{ base: 12, sm: 8, lg: 9 }}>
            <Stack p={{ base: "lg", lg: "xl" }} h="100%" gap={0}>
              <Stack gap="md">
                <Group justify="space-between" align="flex-start" wrap="nowrap">
                  <Group gap="xs"><ProviderMark provider={work.source} /><Badge variant="light" color="gray">{contentTypeLabel(work.contentType)}</Badge>{doc.isEdited && <Badge color="gray" variant="light">ローカル編集</Badge>}</Group>
                  <Menu position="bottom-end"><Menu.Target><Tooltip label="その他"><ActionIcon variant="subtle" color="gray" size="lg" aria-label="作品のその他の操作"><Icons.more size={IconSize.action} /></ActionIcon></Tooltip></Menu.Target><Menu.Dropdown><Menu.Item leftSection={<Icons.inAppBrowser size={IconSize.menu} />} onClick={openSourceInApp}>元ページをアプリ内で開く</Menu.Item><Menu.Item leftSection={<Icons.externalLink size={IconSize.menu} />} onClick={openSource}>元ページをブラウザで開く</Menu.Item><Menu.Divider /><Menu.Item color="red" leftSection={<Icons.delete size={IconSize.menu} />} onClick={deleteWork}>作品を削除</Menu.Item></Menu.Dropdown></Menu>
                </Group>
                <Box>
                  {/* The series line is itself the link, so it is not repeated
                      as a separate button beside the author below. */}
                  {work.seriesTitle && (work.seriesId
                    ? <Anchor component="button" type="button" size="sm" fw={650} c="dimmed" mb={5} display="block" ta="left" className="work-hero__series-link" onClick={() => navigate(`/series/${encodeURIComponent(work.source)}/${encodeURIComponent(work.seriesId!)}`)}>{work.seriesTitle}</Anchor>
                    : <Text size="sm" c="dimmed" fw={650} mb={5}>{work.seriesTitle}</Text>)}
                  <Title order={1} className="work-hero__title line-clamp-2" title={work.title}>{work.title}</Title>
                  {/* Captions were cut off at three lines with no way to read
                      the rest anywhere in the app; the ones people write run
                      from a sentence to a page of notes, so they fold. */}
                  {captionHtml && <ExpandableText className="work-caption" mt="sm" maw={820} lines={4} label="キャプション" html={captionHtml} onClick={openContentLink} />}
                </Box>
                <Group gap="lg">
                  {/* The creator's own picture rather than a generic silhouette. */}
                  <Button
                    variant="subtle"
                    color="gray"
                    px={0}
                    className="work-hero__author-link"
                    leftSection={<Avatar src={getAssetUrl(work.personIconPath)} size={22} radius="xl" color="gray"><Icons.person size={IconSize.inline} /></Avatar>}
                    onClick={() => navigate(`/people/${encodeURIComponent(work.source)}/${encodeURIComponent(work.personId || work.authorId)}`)}
                  >
                    {work.personName || work.authorName}
                  </Button>
                </Group>
                <Group gap={8} className="work-hero__tags">{work.tags.map((tag) => <Badge key={tag} size="md" variant="light" color="gray" component={AppLink} className="work-hero__tag" to={`/library?q=${encodeURIComponent(`tag:${tag}`)}`}>#{tag}</Badge>)}</Group>
              </Stack>
              <Group gap="lg" mt="md" className="work-hero__facts">
                <Text size="sm" c="dimmed"><Icons.publishedDate size={IconSize.menu} />公開 {formatDate(work.sourceCreatedAt)}</Text>
                <Text size="sm" c="dimmed"><Icons.textLength size={IconSize.menu} />{formatNumber(work.textLength)}字</Text>
                {work.assetCount > 0 && <Text size="sm" c="dimmed"><Icons.assets size={IconSize.menu} />{work.assetCount}アセット</Text>}
              </Group>
              <div className="work-hero__actionbar">
                <Group gap={8} wrap="nowrap" className="work-hero__actions">
                  <Button className="work-hero__action" leftSection={<Icons.read size={IconSize.action} />} onClick={() => navigate(`/reader/${work.id}`)}>読む</Button>
                  <Button className="work-hero__action" variant="default" leftSection={<Icons.edit size={IconSize.action} />} onClick={() => navigate(`/editor/${work.id}`)}>編集</Button>
                  {/* One glyph in both states, as on the cards: the label already says which way the press goes. */}
                  <Button className="work-hero__action" variant={queued ? "filled" : "default"} color="leaf" leftSection={<Icons.epubAdd size={IconSize.action} />} onClick={() => queued ? removeFromEpubQueue(work.id) : addToEpubQueue(work.id)}>{queued ? "EPUB追加済み" : "EPUB"}</Button>
                </Group>
                <Group gap={6} wrap="nowrap" className="work-hero__secondary-actions">
                  <Button className="work-hero__action" variant="default" leftSection={<Icons.openFolder size={IconSize.action} />} onClick={openSavedFolder}>保存フォルダー</Button>
                  <Button className="work-hero__action" variant="default" leftSection={<Icons.archive size={IconSize.action} />} onClick={exportArchive}>アーカイブ</Button>
                  <Tooltip label={work.favorite ? "お気に入りを解除" : "お気に入りに追加"}><ActionIcon size="lg" variant="default" color={work.favorite ? "red" : "gray"} style={{ "--ai-color": work.favorite ? "var(--flag-favorite)" : "var(--flag-off)", "--ai-bg": work.favorite ? "var(--flag-favorite-bg)" : undefined }} aria-label={work.favorite ? "お気に入りを解除" : "お気に入りに追加"} aria-pressed={work.favorite} disabled={mutate.isPending} onClick={() => mutate.mutate({ favorite: !work.favorite })}><Icons.favorite size={IconSize.nav} fill={work.favorite ? "currentColor" : "none"} /></ActionIcon></Tooltip>
                </Group>
              </div>
            </Stack>
          </Grid.Col>
        </Grid>
      </Card>

      <Tabs value={tab} onChange={setTab} mt="lg">
        <Tabs.List>
          <Tabs.Tab value="overview">概要</Tabs.Tab>
          <Tabs.Tab value="content">本文</Tabs.Tab>
          <Tabs.Tab value="assets">アセット <Badge size="xs" variant="light" ml={4}>{doc.assetCount}</Badge></Tabs.Tab>
          <Tabs.Tab value="history">履歴</Tabs.Tab>
          <Tabs.Tab value="json">JSON</Tabs.Tab>
        </Tabs.List>
        <Tabs.Panel value="overview" pt="lg">
          {/* Two columns from the medium breakpoint - the previous large one
              left this stacked at ordinary window sizes - and both stretch so
              the section reads as a single block rather than three cards of
              unrelated heights. */}
          <Grid gap="lg" align="stretch">
            <Grid.Col span={{ base: 12, md: 8 }}>
              <Card p="lg" h="100%"><Title order={3} mb="md">作品情報</Title><SimpleGrid cols={{ base: 1, sm: 2 }} spacing="lg"><Info label="作者" value={work.authorName} icon={<Icons.person size={IconSize.action} />} /><Info label="公開日" value={formatDate(work.sourceCreatedAt)} icon={<Icons.publishedDate size={IconSize.action} />} /><Info label="文字数" value={`${formatNumber(work.textLength)}字`} icon={<Icons.read size={IconSize.action} />} /><Info label="ローカル容量" value={formatBytes(work.fileSizeBytes)} icon={<Icons.openFolder size={IconSize.action} />} /><Info label="現在のバージョン" value={`v${work.currentVersion}`} icon={<Icons.versionHistory size={IconSize.action} />} /><Info label="最終保存" value={formatDate(work.downloadedAt, true)} icon={<Icons.epubAdd size={IconSize.action} />} /></SimpleGrid></Card>
            </Grid.Col>
            <Grid.Col span={{ base: 12, md: 4 }}>
              <Stack gap="lg" h="100%" justify="space-between"><Card p="lg"><Group justify="space-between"><Box><Text fw={700}>更新監視</Text><Text size="xs" c="dimmed" mt={3}>保存元の変更をチェック</Text></Box><Switch checked={work.watchUpdates} disabled={mutate.isPending} onChange={(event) => mutate.mutate({ watch: event.currentTarget.checked })} aria-label="更新監視" /></Group><Button fullWidth variant="light" leftSection={<Icons.watch size={IconSize.menu} />} mt="md" onClick={() => runSingleCheck({ kind: "work", workId: work.id, label: work.title }, () => navigate("/updates"))}>今すぐ更新確認</Button></Card><Note title="編集について">ローカル編集は元データを残したまま別リビジョンとして保存されます。</Note></Stack>
            </Grid.Col>
          </Grid>
        </Tabs.Panel>
        <Tabs.Panel value="content" pt="lg">
          {contentQuery.isLoading ? <LoadingState label="本文を読み込んでいます" /> : contentQuery.error ? <ErrorState error={contentQuery.error} retry={() => contentQuery.refetch()} /> : !contentReady ? <LoadingState label="本文を読み込んでいます" /> : <Stack gap="sm">
            {/* A whole novel laid out down the page pushed the tabs, the pager
                and everything else off the top of the window. It reads inside a
                frame of its own instead, the way the JSON already does, so the
                screen around it stays put while you read. */}
            <Paper className="content-frame" withBorder ref={contentFrameRef}>
              <div className="content-preview content-preview--paged" onClick={openContentLink} dangerouslySetInnerHTML={{ __html: preparedHtml }} />
            </Paper>
            <ContentPagination current={contentPage} total={contentQuery.data?.pageCount ?? 1} onChange={goToContentPage} />
          </Stack>}
        </Tabs.Panel>
        <Tabs.Panel value="assets" pt="lg">
          {assetsQuery.isLoading ? <LoadingState label="アセットを読み込んでいます" /> : assetsQuery.error ? <ErrorState error={assetsQuery.error} retry={() => assetsQuery.refetch()} /> : assets.length ? <Stack gap="lg"><SimpleGrid cols={{ base: 2, sm: 3, lg: 4, xl: 5 }} spacing="md">{visibleAssets.map((asset) => {
            const url = getAssetUrl(asset.localPath);
            const image = asset.mimeType?.startsWith("image/") && url;
            const original = Boolean(asset.originalUrl && (/original/i.test(asset.originalUrl) || !/\/c\//.test(asset.originalUrl)));
            return <Card key={asset.id} p={0} className="asset-card surface--interactive" onClick={() => image && previewAsset(asset)}>
              <Box className="asset-card__preview">
                {image ? <Image src={url} alt={asset.filename} className="asset-card__image" loading="lazy" decoding="async" /> : <ThemeIcon size={56} variant="light" color="gray"><Icons.file size={IconSize.feature} /></ThemeIcon>}
                {original && <Badge className="asset-card__badge" size="xs" color="green" variant="filled">原寸</Badge>}
              </Box>
              {/* Filename first and on its own line: it is what identifies the
                  file, and crushing it beside a badge only truncated it. */}
              <Stack gap={2} className="asset-card__meta">
                <Text size="xs" fw={650} className="asset-card__name" title={asset.filename}>{asset.filename}</Text>
                <Group justify="space-between" gap="xs" wrap="nowrap">
                  <Text size="10px" c="dimmed">{formatBytes(asset.fileSizeBytes)}</Text>
                  <Text size="10px" c="dimmed" tt="uppercase">{(asset.mimeType || asset.assetType).split("/").pop()}</Text>
                </Group>
              </Stack>
            </Card>;
          })}</SimpleGrid>{visibleAssets.length < assets.length && <Group justify="center"><Button variant="default" onClick={() => setVisibleAssetCount((count) => Math.min(assets.length, count + 80))}>さらに表示（残り{formatNumber(assets.length - visibleAssets.length)}件）</Button></Group>}</Stack> : <Alert color="gray" icon={<Icons.imageFile size={IconSize.action} />}>この作品にアセットはありません。</Alert>}
        </Tabs.Panel>
        <Tabs.Panel value="history" pt="lg">
          <Grid gap="lg">
            <Grid.Col span={{ base: 12, lg: 5 }}><Stack gap="xs">{doc.versions.map((version) => <Paper component="button" type="button" key={version.id} p="md" withBorder className="version-row" data-active={selectedVersion === version.version || undefined} onClick={() => setSelectedVersion(version.version)}><Group wrap="nowrap" align="flex-start"><ThemeIcon variant="light" color={version.version === work.currentVersion ? "piep" : "gray"}><Icons.versionHistory size={IconSize.menu} /></ThemeIcon><Stack gap={3} flex={1} ta="left"><Group justify="space-between"><Text size="sm" fw={700}>バージョン {version.version}</Text>{version.version === work.currentVersion && <Badge size="xs">現在</Badge>}</Group><Text size="xs" c="dimmed">{version.changeSummary || "保存元から取得"}</Text><Text size="xs" c="dimmed">{formatDate(version.createdAt, true)} · {formatNumber(version.textLength)}字 · {formatBytes(version.fileSizeBytes)}</Text></Stack></Group></Paper>)}</Stack></Grid.Col>
            <Grid.Col span={{ base: 12, lg: 7 }}><Card p="lg" className="version-preview"><Group justify="space-between" mb="md"><Box><Text fw={700}>{selectedVersion ? `バージョン ${selectedVersion} の内容` : "履歴を選択"}</Text><Text size="xs" c="dimmed">履歴をクリックすると、その時点の本文を確認できます</Text></Box>{selectedVersion && <Button size="xs" variant="light" onClick={() => navigate(`/reader/${work.id}?version=${selectedVersion}`)}>この版を読む</Button>}</Group>{versionPreview.isLoading ? <LoadingState label="履歴を読み込んでいます" /> : selectedVersion && versionPreview.data ? <Text className="version-preview__text">{versionPreview.data.plainText.slice(0, 5000) || "本文がありません"}</Text> : <Alert color="gray">左から確認するバージョンを選択してください。</Alert>}</Card></Grid.Col>
          </Grid>
        </Tabs.Panel>
        <Tabs.Panel value="json" pt="lg">{rawJson.isLoading ? <LoadingState /> : rawJson.error ? <ErrorState error={rawJson.error} retry={() => rawJson.refetch()} /> : <Stack gap="sm"><Group justify="space-between"><Text size="sm" c="dimmed">保存元から取得したJSONを整形表示しています。長い文字列は画面内で折り返します。</Text><Group gap="xs"><CopyButton value={rawJson.data ?? "{}"}>{({ copied, copy }) => <Button size="xs" variant="light" onClick={copy}>{copied ? "コピー済み" : "JSONをコピー"}</Button>}</CopyButton><Button size="xs" variant="default" disabled={!runtime} onClick={() => openLocalAsset(work.jsonPath)}>この端末のJSONを開く</Button></Group></Group><Paper className="json-view" withBorder><pre>{prettyJson(rawJson.data ?? "{}")}</pre></Paper></Stack>}</Tabs.Panel>
      </Tabs>
    </div>
  );
}

/**
 * The same controls the reader moves between source pages with.
 *
 * It is the same act on the same document, so it should not be a different
 * shape here - two wide buttons that could only step one page at a time, in a
 * novel that has forty of them.
 */
function ContentPagination({ current, total, onChange }: { current: number; total: number; onChange: (page: number) => void }) {
  if (total <= 1) return null;
  return <Group justify="center" gap={6} wrap="nowrap" className="content-pagination">
    <Pagination total={total} value={current} onChange={onChange} siblings={1} boundaries={1} withEdges size="sm" radius="sm" />
    <Divider orientation="vertical" h={24} />
    <Text size="sm" fw={650} ff="monospace" c="dimmed">{current} / {total}</Text>
  </Group>;
}

function prettyJson(raw: string): string {
  try { return JSON.stringify(JSON.parse(raw), null, 2); }
  catch { return raw; }
}

function Info({ label, value, icon }: { label: string; value: string; icon: React.ReactNode }) {
  return <Group wrap="nowrap" align="flex-start"><Avatar size="sm" variant="light" color="gray">{icon}</Avatar><Box><Text size="xs" c="dimmed">{label}</Text><Text size="sm" fw={650} mt={2}>{value}</Text></Box></Group>;
}
