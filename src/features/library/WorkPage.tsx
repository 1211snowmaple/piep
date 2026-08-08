import { useEffect, useMemo, useState } from "react";
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
  Menu,
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
import {
  Archive,
  BookOpen,
  Calendar,
  ChevronLeft,
  ChevronRight,
  Download,
  Edit3,
  Ellipsis,
  ExternalLink,
  File,
  FileImage,
  FolderOpen,
  Heart,
  History,
  Images,
  Library,
  RefreshCw,
  Trash2,
  UserRound,
} from "lucide-react";
import { AppLink, useAppNavigate, useAppSearchParams, useRouteParams } from "@/app/router";
import { useWorkspace } from "@/app/WorkspaceContext";
import { ErrorState, LoadingState } from "@/components/AsyncState";
import { WorkCover } from "@/components/WorkCover";
import { extractSavedSourceTarget } from "@/features/browser/downloadCandidates";
import { ProviderMark, sourceUrl } from "@/lib/providers";
import { contentTypeLabel, errorMessage, formatBytes, formatDate, formatNumber } from "@/lib/format";
import { prepareDocumentHtml, splitDocumentPages } from "@/lib/content";
import { exportSingle } from "@/services/archiveApi";
import { openSingleDialog } from "@/services/dialogApi";
import {
  deleteDownload,
  getAssetUrl,
  getDownloadBySource,
  getReaderDocument,
  isTauriRuntime,
  openLocalAsset,
  readFileContent,
  setFavorite,
  setWatchUpdates,
} from "@/services/dbApi";
import { openExternalUrl } from "@/services/openerApi";
import { getDemoReader } from "@/mocks/demoData";

export default function WorkPage() {
  const navigate = useAppNavigate();
  const { workId } = useRouteParams("/works/:workId");
  const id = Number(workId);
  const runtime = isTauriRuntime();
  const queryClient = useQueryClient();
  const { addToEpubQueue, removeFromEpubQueue, isQueuedForEpub } = useWorkspace();
  // Driven by the URL so the card's version chip can deep-link into the
  // history, and so going back returns to the tab you were on.
  const [searchParams, setSearchParams] = useAppSearchParams();
  const tab = searchParams.get("tab") ?? "overview";
  const setTab = (next: string | null) => {
    const params = new URLSearchParams(searchParams);
    if (!next || next === "overview") params.delete("tab"); else params.set("tab", next);
    setSearchParams(params, { replace: true });
  };
  const [contentPage, setContentPage] = useState(1);
  const [selectedVersion, setSelectedVersion] = useState<number | null>(null);
  const documentQuery = useQuery({
    queryKey: ["reader-document", id, null],
    queryFn: () => runtime ? getReaderDocument(id) : Promise.resolve(getDemoReader(id)),
    enabled: Number.isFinite(id),
  });
  const rawJson = useQuery({
    queryKey: ["work-json", id, documentQuery.data?.download.jsonPath],
    queryFn: () => runtime && documentQuery.data ? readFileContent(documentQuery.data.download.jsonPath) : Promise.resolve(JSON.stringify(documentQuery.data?.download ?? {}, null, 2)),
    enabled: tab === "json" && Boolean(documentQuery.data),
  });
  const versionPreview = useQuery({
    queryKey: ["reader-document", id, selectedVersion],
    queryFn: () => runtime ? getReaderDocument(id, selectedVersion) : Promise.resolve(getDemoReader(id)),
    enabled: selectedVersion !== null,
  });
  const mutate = useMutation({
    mutationFn: async (input: { favorite?: boolean; watch?: boolean }) => {
      if (!runtime) return;
      if (typeof input.favorite === "boolean") await setFavorite(id, input.favorite);
      if (typeof input.watch === "boolean") await setWatchUpdates(id, input.watch);
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["reader-document", id] });
      queryClient.invalidateQueries({ queryKey: ["library"] });
    },
    onError: (error) => notifications.show({ color: "red", title: "変更できません", message: errorMessage(error) }),
  });
  const preparedHtml = useMemo(() => prepareDocumentHtml(documentQuery.data?.html ?? "", getAssetUrl), [documentQuery.data?.html]);
  const captionHtml = useMemo(() => prepareDocumentHtml(documentQuery.data?.download.excerpt ?? "", getAssetUrl), [documentQuery.data?.download.excerpt]);
  const contentPages = useMemo(() => splitDocumentPages(preparedHtml, documentQuery.data?.download.source ?? ""), [documentQuery.data?.download.source, preparedHtml]);
  useEffect(() => setContentPage(1), [preparedHtml]);

  if (documentQuery.isLoading) return <div className="page"><LoadingState label="作品を開いています" /></div>;
  if (documentQuery.error || !documentQuery.data) return <div className="page"><ErrorState error={documentQuery.error ?? "作品が見つかりません"} retry={() => documentQuery.refetch()} /></div>;
  const doc = documentQuery.data;
  const work = doc.download;
  const queued = isQueuedForEpub(work.id);

  const deleteWork = () => modals.openConfirmModal({
    title: "この作品を削除しますか？",
    children: <Text size="sm">「{work.title}」と保存済みアセットをローカルライブラリから削除します。この操作は元に戻せません。</Text>,
    labels: { confirm: "削除する", cancel: "キャンセル" }, confirmProps: { color: "red" },
    onConfirm: async () => {
      try { if (runtime) await deleteDownload(work.id); navigate("/library"); notifications.show({ color: "green", message: "作品を削除しました" }); }
      catch (error) { notifications.show({ color: "red", title: "削除できません", message: errorMessage(error) }); }
    },
  });
  const exportArchive = async () => {
    if (!runtime) return notifications.show({ color: "blue", message: "アーカイブ出力はデスクトップアプリで利用できます" });
    const dir = await openSingleDialog({ directory: true, title: "アーカイブの出力先" });
    if (!dir) return;
    try { const path = await exportSingle(work.id, dir); notifications.show({ color: "green", title: "アーカイブを書き出しました", message: path }); }
    catch (error) { notifications.show({ color: "red", title: "書き出しに失敗しました", message: errorMessage(error) }); }
  };
  const openSource = async () => {
    const url = sourceUrl(work.source, work.sourceId, work.contentType, work.personId || work.authorId);
    if (!url) return;
    if (runtime) await openExternalUrl(url); else window.open(url, "_blank", "noopener,noreferrer");
  };
  const handleRichContentClick = async (event: React.MouseEvent<HTMLElement>) => {
    const anchor = (event.target as HTMLElement).closest<HTMLAnchorElement>("a[href]");
    if (!anchor || !anchor.href.startsWith("http")) return;
    event.preventDefault();
    const target = extractSavedSourceTarget(anchor.href);
    if (runtime && target) {
      const saved = await getDownloadBySource(target.source, target.sourceId);
      if (saved) {
        navigate(`/works/${saved.id}`);
        return;
      }
    }
    if (runtime) await openExternalUrl(anchor.href); else window.open(anchor.href, "_blank", "noopener,noreferrer");
  };
  const previewAsset = (asset: (typeof doc.assets)[number]) => {
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
      <Breadcrumbs mb="md" className="work-breadcrumbs"><Anchor component={AppLink} to="/library" size="sm">ライブラリ</Anchor><Text size="sm" c="dimmed" className="line-clamp-1">{work.title}</Text></Breadcrumbs>
      <Card className="work-hero" padding={0}>
        <Grid gap={0} align="stretch">
          <Grid.Col span={{ base: 12, sm: 4, lg: 3 }}>
            <Box className="work-hero__cover-stage">
              <WorkCover work={work} variant="detail" className="work-hero__cover" />
            </Box>
          </Grid.Col>
          <Grid.Col span={{ base: 12, sm: 8, lg: 9 }}>
            <Stack p={{ base: "lg", lg: "xl" }} h="100%" justify="space-between">
              <Stack gap="md">
                <Group justify="space-between" align="flex-start" wrap="nowrap">
                  <Group gap="xs"><ProviderMark provider={work.source} /><Badge variant="light" color="gray">{contentTypeLabel(work.contentType)}</Badge>{doc.isEdited && <Badge color="violet" variant="light">ローカル編集</Badge>}</Group>
                  <Menu position="bottom-end"><Menu.Target><Tooltip label="その他"><ActionIcon variant="subtle" color="gray" aria-label="作品のその他の操作"><Ellipsis size={20} /></ActionIcon></Tooltip></Menu.Target><Menu.Dropdown><Menu.Item leftSection={<FolderOpen size={15} />} onClick={() => runtime && openLocalAsset(work.jsonPath)}>この端末の保存フォルダーを開く</Menu.Item><Menu.Item leftSection={<Archive size={15} />} onClick={exportArchive}>アーカイブを書き出す</Menu.Item><Menu.Divider /><Menu.Item color="red" leftSection={<Trash2 size={15} />} onClick={deleteWork}>作品を削除</Menu.Item></Menu.Dropdown></Menu>
                </Group>
                <Box>
                  {work.seriesTitle && <Text size="sm" c="dimmed" fw={650} mb={5}>{work.seriesTitle}</Text>}
                  <Title order={1} className="work-hero__title line-clamp-2" title={work.title}>{work.title}</Title>
                  <Group gap="md" mt="sm"><Text size="sm" c="dimmed"><Calendar size={14} /> 公開 {formatDate(work.sourceCreatedAt)}</Text><Text size="sm" c="dimmed"><Images size={14} /> {work.assetCount}アセット</Text><Text size="sm" c="dimmed">{formatNumber(work.textLength)}字</Text></Group>
                  {captionHtml && <Box className="work-caption line-clamp-3" mt="sm" maw={820} onClick={handleRichContentClick} dangerouslySetInnerHTML={{ __html: captionHtml }} />}
                </Box>
                <Group gap="lg">
                  <Button variant="subtle" color="gray" leftSection={<UserRound size={16} />} px={0} onClick={() => navigate(`/people/${encodeURIComponent(work.source)}/${encodeURIComponent(work.personId || work.authorId)}`)}>{work.personName || work.authorName}</Button>
                  {work.seriesId && <Button variant="subtle" color="gray" leftSection={<Library size={16} />} px={0} onClick={() => navigate(`/series/${encodeURIComponent(work.source)}/${encodeURIComponent(work.seriesId!)}`)}>{work.seriesTitle || "シリーズ"}</Button>}
                </Group>
                <Group gap="xs">{work.tags.map((tag) => <Badge key={tag} variant="light" color="gray" component={AppLink} to={`/library?q=${encodeURIComponent(tag)}`}>{tag}</Badge>)}</Group>
              </Stack>
              <Group justify="space-between" mt="xl">
                <Group gap="xs"><Button leftSection={<BookOpen size={16} />} onClick={() => navigate(`/reader/${work.id}`)}>読む</Button><Button variant="light" leftSection={<Edit3 size={16} />} onClick={() => navigate(`/editor/${work.id}`)}>編集</Button><Button variant="default" leftSection={queued ? <CheckIcon /> : <Download size={16} />} onClick={() => queued ? removeFromEpubQueue(work.id) : addToEpubQueue(work.id)}>{queued ? "EPUBキュー済み" : "EPUBへ"}</Button></Group>
                <Group gap="xs"><Tooltip label={work.favorite ? "お気に入りを解除" : "お気に入りに追加"}><ActionIcon size="lg" variant={work.favorite ? "filled" : "light"} color="orange" aria-label={work.favorite ? "お気に入りを解除" : "お気に入りに追加"} onClick={() => mutate.mutate({ favorite: !work.favorite })}><Heart size={18} fill={work.favorite ? "currentColor" : "none"} /></ActionIcon></Tooltip>{/* "保存元" read as either the website or the folder on disk; both are
    reachable and each now says which one it is. */}
<Button variant="subtle" color="gray" rightSection={<ExternalLink size={14} />} onClick={openSource}>元ページを開く</Button></Group>
              </Group>
            </Stack>
          </Grid.Col>
        </Grid>
      </Card>

      <Tabs value={tab} onChange={setTab} mt="lg">
        <Tabs.List>
          <Tabs.Tab value="overview">概要</Tabs.Tab>
          <Tabs.Tab value="content">本文</Tabs.Tab>
          <Tabs.Tab value="assets">アセット <Badge size="xs" variant="light" ml={4}>{doc.assets.length}</Badge></Tabs.Tab>
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
              <Card p="lg" h="100%"><Title order={3} mb="md">作品情報</Title><SimpleGrid cols={{ base: 1, sm: 2 }} spacing="lg"><Info label="作者" value={work.authorName} icon={<UserRound size={16} />} /><Info label="公開日" value={formatDate(work.sourceCreatedAt)} icon={<Calendar size={16} />} /><Info label="文字数" value={`${formatNumber(work.textLength)}字`} icon={<BookOpen size={16} />} /><Info label="ローカル容量" value={formatBytes(work.fileSizeBytes)} icon={<FolderOpen size={16} />} /><Info label="現在のバージョン" value={`v${work.currentVersion}`} icon={<History size={16} />} /><Info label="最終保存" value={formatDate(work.downloadedAt, true)} icon={<Download size={16} />} /></SimpleGrid></Card>
            </Grid.Col>
            <Grid.Col span={{ base: 12, md: 4 }}>
              <Stack gap="lg" h="100%" justify="space-between"><Card p="lg"><Group justify="space-between"><Box><Text fw={700}>更新監視</Text><Text size="xs" c="dimmed" mt={3}>保存元の変更をチェック</Text></Box><Switch checked={work.watchUpdates} onChange={(event) => mutate.mutate({ watch: event.currentTarget.checked })} aria-label="更新監視" /></Group><Button fullWidth variant="light" leftSection={<RefreshCw size={15} />} mt="md" onClick={() => navigate(`/updates?work=${work.id}`)}>今すぐ更新確認</Button></Card><Alert color="blue" title="編集について" className="work-note">ローカル編集は元データを残したまま別リビジョンとして保存されます。</Alert></Stack>
            </Grid.Col>
          </Grid>
        </Tabs.Panel>
        <Tabs.Panel value="content" pt="lg">
          <Stack gap="sm">
            <ContentPagination current={contentPage} total={contentPages.length} onChange={setContentPage} />
            <Paper className="content-preview content-preview--paged" p={{ base: "lg", sm: "xl" }} withBorder>
              <div onClick={handleRichContentClick} dangerouslySetInnerHTML={{ __html: contentPages[contentPage - 1] ?? "" }} />
            </Paper>
            <ContentPagination current={contentPage} total={contentPages.length} onChange={setContentPage} />
          </Stack>
        </Tabs.Panel>
        <Tabs.Panel value="assets" pt="lg">
          {doc.assets.length ? <SimpleGrid cols={{ base: 2, sm: 3, lg: 4, xl: 5 }} spacing="md">{doc.assets.map((asset) => {
            const url = getAssetUrl(asset.localPath);
            const image = asset.mimeType?.startsWith("image/") && url;
            const original = Boolean(asset.originalUrl && (/original/i.test(asset.originalUrl) || !/\/c\//.test(asset.originalUrl)));
            return <Card key={asset.id} p={0} className="asset-card surface--interactive" onClick={() => image && previewAsset(asset)}>
              <Box className="asset-card__preview">
                {image ? <Image src={url} alt={asset.filename} className="asset-card__image" /> : <ThemeIcon size={56} variant="light" color="gray"><File size={26} /></ThemeIcon>}
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
          })}</SimpleGrid> : <Alert color="gray" icon={<FileImage size={17} />}>この作品にアセットはありません。</Alert>}
        </Tabs.Panel>
        <Tabs.Panel value="history" pt="lg">
          <Grid gap="lg">
            <Grid.Col span={{ base: 12, lg: 5 }}><Stack gap="xs">{doc.versions.map((version) => <Paper component="button" type="button" key={version.id} p="md" withBorder className="version-row" data-active={selectedVersion === version.version || undefined} onClick={() => setSelectedVersion(version.version)}><Group wrap="nowrap" align="flex-start"><ThemeIcon variant="light" color={version.version === work.currentVersion ? "blue" : "gray"}><History size={15} /></ThemeIcon><Stack gap={3} flex={1} ta="left"><Group justify="space-between"><Text size="sm" fw={700}>バージョン {version.version}</Text>{version.version === work.currentVersion && <Badge size="xs">現在</Badge>}</Group><Text size="xs" c="dimmed">{version.changeSummary || "保存元から取得"}</Text><Text size="xs" c="dimmed">{formatDate(version.createdAt, true)} · {formatNumber(version.textLength)}字 · {formatBytes(version.fileSizeBytes)}</Text></Stack></Group></Paper>)}</Stack></Grid.Col>
            <Grid.Col span={{ base: 12, lg: 7 }}><Card p="lg" className="version-preview"><Group justify="space-between" mb="md"><Box><Text fw={700}>{selectedVersion ? `バージョン ${selectedVersion} の内容` : "履歴を選択"}</Text><Text size="xs" c="dimmed">履歴をクリックすると、その時点の本文を確認できます</Text></Box>{selectedVersion && <Button size="xs" variant="light" onClick={() => navigate(`/reader/${work.id}?version=${selectedVersion}`)}>この版を読む</Button>}</Group>{versionPreview.isLoading ? <LoadingState label="履歴を読み込んでいます" /> : selectedVersion && versionPreview.data ? <Text className="version-preview__text">{versionPreview.data.plainText.slice(0, 5000) || "本文がありません"}</Text> : <Alert color="gray">左から確認するバージョンを選択してください。</Alert>}</Card></Grid.Col>
          </Grid>
        </Tabs.Panel>
        <Tabs.Panel value="json" pt="lg">{rawJson.isLoading ? <LoadingState /> : rawJson.error ? <ErrorState error={rawJson.error} retry={() => rawJson.refetch()} /> : <Stack gap="sm"><Group justify="space-between"><Text size="sm" c="dimmed">保存元から取得したJSONを整形表示しています。長い文字列は画面内で折り返します。</Text><Group gap="xs"><CopyButton value={rawJson.data ?? "{}"}>{({ copied, copy }) => <Button size="xs" variant="light" onClick={copy}>{copied ? "コピー済み" : "JSONをコピー"}</Button>}</CopyButton><Button size="xs" variant="default" disabled={!runtime} onClick={() => openLocalAsset(work.jsonPath)}>この端末のJSONを開く</Button></Group></Group><Paper className="json-view" withBorder><pre>{prettyJson(rawJson.data ?? "{}")}</pre></Paper></Stack>}</Tabs.Panel>
      </Tabs>
    </div>
  );
}

function CheckIcon() { return <span aria-hidden>✓</span>; }

function ContentPagination({ current, total, onChange }: { current: number; total: number; onChange: (page: number) => void }) {
  if (total <= 1) return null;
  return <Group justify="center" gap="xs" className="content-pagination">
    <Button size="xs" variant="default" leftSection={<ChevronLeft size={14} />} disabled={current <= 1} onClick={() => onChange(current - 1)}>前のページ</Button>
    <Text size="sm" fw={700} ff="monospace" miw={80} ta="center">{current} / {total}</Text>
    <Button size="xs" variant="default" rightSection={<ChevronRight size={14} />} disabled={current >= total} onClick={() => onChange(current + 1)}>次のページ</Button>
  </Group>;
}

function prettyJson(raw: string): string {
  try { return JSON.stringify(JSON.parse(raw), null, 2); }
  catch { return raw; }
}

function Info({ label, value, icon }: { label: string; value: string; icon: React.ReactNode }) {
  return <Group wrap="nowrap" align="flex-start"><Avatar size="sm" variant="light" color="gray">{icon}</Avatar><Box><Text size="xs" c="dimmed">{label}</Text><Text size="sm" fw={650} mt={2}>{value}</Text></Box></Group>;
}
