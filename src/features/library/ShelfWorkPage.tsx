import { useMemo, useRef, useState } from "react";
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
  Divider,
  Grid,
  Group,
  Image,
  Paper,
  Select,
  SimpleGrid,
  Stack,
  Text,
  ThemeIcon,
  Title,
  Tooltip,
} from "@mantine/core";
import { modals } from "@mantine/modals";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { notifications } from "@mantine/notifications";
import { MotionTabs as Tabs } from "@/components/MotionTabs";
import { ActionBar } from "@/components/ActionBar";
import { AppLink, useAppNavigate, useAppSearchParams, useReturnTo } from "@/app/router";
import { ErrorState, LoadingState } from "@/components/AsyncState";
import { ExpandableText } from "@/components/ExpandableText";
import { WorkCover } from "@/components/WorkCover";
import { BoundedJsonView } from "@/components/BoundedJsonView";
import { RevisionDiff } from "@/components/RevisionDiff";
import { Note } from "@/components/Note";
import { PdfOriginalViewer, type PdfOriginalTarget } from "@/components/PdfOriginalViewer";
import { Icons, IconSize } from "@/lib/icons";
import { ProviderMark, sourceUrl } from "@/lib/providers";
import { contentTypeLabel, errorMessage, formatBytes, formatDate, formatNumber } from "@/lib/format";
import { prepareDocumentHtml } from "@/lib/content";
import { useContentLinkNavigation, useLibraryLinkMarks } from "@/lib/contentLinks";
import { isTauriRuntime } from "@/services/dbApi";
import { openExternalUrl } from "@/services/openerApi";
import {
  isShelfPdf,
  libraryV2Content,
  libraryV2EditHistory,
  libraryV2EditPointer,
  libraryV2Media,
  libraryV2Original,
  libraryV2VersionText,
  libraryV2SetUserState,
  libraryV2Work,
  shelfBodyUrl,
  shelfFileUrl,
  staleEditBaseOf,
  type ShelfEditChange,
  type ShelfEditorRevision,
  type ShelfMedia,
  type ShelfVersion,
} from "@/services/libraryV2Api";
import { invalidateShelf, shelfEntry, useShelfWritable } from "@/features/library/shelfV2";
import { ContentPagination, Info, parseWorkTab } from "@/features/library/workPageParts";
import { ActionRow } from "@/components/ActionRow";
import { DANGER_CONFIRM, DangerButton } from "@/components/DangerButton";
import { EditStatusStrip } from "@/features/library/EditStatusStrip";
import type { WorkEditRevision } from "@/types/library";
import { runSingleCheck } from "@/features/updates/startSingleCheck";

const READ_ONLY = "新しい棚では変えられません";

function versionDate(version: ShelfVersion): string {
  return formatDate(new Date(version.saved_at).toISOString(), true);
}

function revisionDate(revision: ShelfEditorRevision): string {
  return formatDate(new Date(revision.createdAt).toISOString(), true);
}

/** 本文の上の知らせ（旧棚と共用）が読む形へ。 */
function stripRevision(revision: ShelfEditorRevision, status: string): WorkEditRevision {
  const at = new Date(revision.createdAt).toISOString();
  return { id: 0, downloadId: 0, baseVersion: revision.baseVersion, status, title: revision.title, contentHash: null, createdAt: at, updatedAt: at };
}

/**
 * 新しい棚（試験中）の作品の画面。正本に切り替えた棚では、お気に入りと編集（編集画面へ進む・取り込んだ
 * 本文に戻す・以前の編集版をもう一度反映する・一覧から外す）を書く。
 *
 * 旧棚の作品の画面と同じ並び（頭・概要・本文・画像と添付・履歴・保存データ）にして、
 * 切り替えても見る場所が変わらないようにする。本文は新しい棚の「本文の中の指定 → 画像」の
 * 対応で画像を引き、旧棚のファイル名には頼らない。
 */
export function ShelfWorkPage({ workId }: { workId: string }) {
  const navigate = useAppNavigate();
  const returnTo = useReturnTo();
  const openContentLink = useContentLinkNavigation();
  const runtime = isTauriRuntime();
  const [searchParams, setSearchParams] = useAppSearchParams();
  const tab = parseWorkTab(searchParams.get("tab"));
  const setTab = (next: string | null) => {
    const params = new URLSearchParams(searchParams);
    const normalized = parseWorkTab(next);
    if (normalized === "overview") params.delete("tab"); else params.set("tab", normalized);
    setSearchParams(params, { replace: true });
  };
  const [contentPage, setContentPage] = useState(1);
  // 履歴から「この版の本文を見る」で選んだ版。null は今読む本文。
  const [contentSnapshot, setContentSnapshot] = useState<string | null>(null);
  const [assetSnapshot, setAssetSnapshot] = useState<string | null>(null);
  const [selectedSnapshot, setSelectedSnapshot] = useState<string | null>(null);
  const [visibleAssetCount, setVisibleAssetCount] = useState(80);
  const [pdfOriginal, setPdfOriginal] = useState<PdfOriginalTarget | null>(null);
  const contentFrameRef = useRef<HTMLDivElement>(null);
  const contentBodyRef = useRef<HTMLDivElement>(null);

  const workQuery = useQuery({
    queryKey: ["library-v2-work", workId],
    queryFn: () => libraryV2Work(workId),
    enabled: runtime,
  });
  const work = workQuery.data;
  const queryClient = useQueryClient();
  // 正本に切り替えた棚だけ書ける。切り替える前は旧棚から移した写しなので押せない。
  const writable = useShelfWritable(runtime);
  const favoriteLabel = !writable ? `お気に入り（${READ_ONLY}）` : work?.favorite ? "お気に入りを解除" : "お気に入りに追加";
  const editLabel = writable ? "編集" : `編集（${READ_ONLY}）`;
  const checkLabel = writable ? "更新を確認" : `更新を確認（${READ_ONLY}）`;
  const [checking, setChecking] = useState(false);
  const checkWork = async () => {
    if (!writable || checking || !work) return;
    setChecking(true);
    try {
      await runSingleCheck({ kind: "work", workId, label: work.title }, () => navigate("/updates"));
    } finally { setChecking(false); }
  };
  // 下書き・反映中の版・以前の編集版。
  const historyQuery = useQuery({
    queryKey: ["library-v2-edit-history", workId],
    queryFn: () => libraryV2EditHistory(workId),
    enabled: runtime,
  });
  const editHistory = historyQuery.data;
  const archived = editHistory?.archived ?? [];
  // 編集の指し先を変える（版そのものは残る）。今の世代は押したときに引き直す。
  const [changing, setChanging] = useState<string | null>(null);
  const changeEdit = async (key: string, change: ShelfEditChange, done: string, failed: string) => {
    setChanging(key);
    try {
      const { generation } = await libraryV2EditHistory(workId);
      await libraryV2EditPointer(workId, generation, change);
      await invalidateShelf(queryClient);
      notifications.show({ color: "green", message: done });
    } catch (error) {
      notifications.show({ color: "red", title: failed, message: errorMessage(error) });
    } finally {
      setChanging(null);
    }
  };
  const revertEdit = () => changeEdit("revert", { kind: "unpublish" }, "取り込んだ本文に戻しました", "本文を戻せません");
  const confirmRestore = (revision: ShelfEditorRevision) => modals.openConfirmModal({
    title: "この編集版をもう一度反映しますか？",
    children: <Text size="sm">読書画面とEPUBは、{revisionDate(revision)}の編集版を使うようになります。{editHistory?.active ? "いま反映している編集版は、ここに残ります。" : ""}</Text>,
    labels: { confirm: "反映する", cancel: "やめる" },
    onConfirm: () => { void changeEdit(`restore-${revision.id}`, { kind: "restore", edit: revision.id }, "編集版を反映しました", "編集版を反映できません"); },
  });
  // 一覧から外す。封をした版は棚に残るが、ここからはもう反映できない。
  const confirmForget = (revision: ShelfEditorRevision | null) => modals.openConfirmModal({
    title: revision ? "この編集版を一覧から外しますか？" : `以前の編集版を${formatNumber(archived.length)}件すべて一覧から外しますか？`,
    children: <Text size="sm">{revision ? `${revisionDate(revision)}の編集版を一覧から外します。` : ""}外した編集版は、ここからはもう反映できません。取り込んだ本文と、いま反映している編集版・下書きは残ります。</Text>,
    labels: { confirm: "外す", cancel: "やめる" },
    confirmProps: DANGER_CONFIRM,
    onConfirm: () => {
      void (revision
        ? changeEdit(`forget-${revision.id}`, { kind: "forget", edit: revision.id }, "編集版を一覧から外しました", "編集版を外せません")
        : changeEdit("forget-all", { kind: "forget_all" }, "以前の編集版を一覧から外しました", "編集版を外せません"));
    },
  });
  const toggleFavorite = async () => {
    if (!work) return;
    try {
      await libraryV2SetUserState(workId, { favorite: !work.favorite });
      await invalidateShelf(queryClient);
    } catch (error) {
      notifications.show({ color: "red", title: "お気に入りを変更できません", message: errorMessage(error) });
    }
  };
  const versions = useMemo(() => [...(work?.versions ?? [])].sort((left, right) => right.version - left.version), [work?.versions]);
  const contentQuery = useQuery({
    queryKey: ["library-v2-content", workId, contentSnapshot, contentPage - 1],
    queryFn: () => libraryV2Content(workId, contentSnapshot, contentPage - 1),
    enabled: runtime && tab === "content",
    placeholderData: (previous) => previous,
  });
  const mediaQuery = useQuery({
    queryKey: ["library-v2-media", workId, assetSnapshot],
    queryFn: () => libraryV2Media(workId, assetSnapshot),
    enabled: runtime && tab === "assets",
  });
  const originalQuery = useQuery({
    queryKey: ["library-v2-original", workId],
    queryFn: () => libraryV2Original(workId, null),
    enabled: runtime && tab === "json",
  });
  // 履歴は、選んだ版と一つ前の版を比べる。何も選んでいなければ現在の版。
  const selected = versions.find((version) => version.snapshot === selectedSnapshot) ?? versions[0] ?? null;
  const previous = selected ? versions.find((version) => version.version < selected.version) ?? null : null;
  const selectedText = useQuery({
    queryKey: ["library-v2-version-text", workId, selected?.snapshot],
    queryFn: () => libraryV2VersionText(workId, selected!.snapshot),
    enabled: runtime && tab === "history" && Boolean(selected),
  });
  const previousText = useQuery({
    queryKey: ["library-v2-version-text", workId, previous?.snapshot],
    queryFn: () => libraryV2VersionText(workId, previous!.snapshot),
    enabled: runtime && tab === "history" && Boolean(previous),
  });

  const contentReady = !contentQuery.isPlaceholderData && contentQuery.data?.page === contentPage - 1;
  const preparedHtml = useMemo(
    () => tab === "content" && contentReady ? prepareDocumentHtml(contentQuery.data?.html ?? "", shelfBodyUrl) : "",
    [contentQuery.data?.html, contentReady, tab],
  );
  const preparedMarkup = useMemo(() => ({ __html: preparedHtml }), [preparedHtml]);
  useLibraryLinkMarks(contentBodyRef, preparedHtml);
  const captionHtml = useMemo(() => prepareDocumentHtml(work?.caption ?? "", shelfBodyUrl, { inlineLinks: true }), [work?.caption]);

  const goToContentPage = (page: number) => {
    setContentPage(page);
    if (contentFrameRef.current) contentFrameRef.current.scrollTop = 0;
  };
  const showVersionContent = (snapshot: string | null) => {
    setContentSnapshot(snapshot);
    goToContentPage(1);
    setTab("content");
  };
  const handleContentClick = (event: React.MouseEvent<HTMLElement>) => {
    // 取り込んだ本文の頭の「原本を見る」。印はファイルの識別子を持つ。
    const original = (event.target as HTMLElement).closest<HTMLElement>("button.attachment-original");
    if (original) {
      event.preventDefault();
      const file = original.dataset.localPath;
      if (file) setPdfOriginal({ workId, localPath: file, fileName: original.dataset.fileName || "添付ファイル" });
      return;
    }
    void openContentLink(event);
  };

  if (!runtime) return <div className="page"><Alert color="gray">新しい棚の作品は、デスクトップアプリで開けます。</Alert></div>;
  if (workQuery.isLoading) return <div className="page"><LoadingState label="作品を開いています" /></div>;
  if (workQuery.error || !work) return <div className="page"><ErrorState error={workQuery.error ?? "作品が見つかりません"} retry={() => workQuery.refetch()} /></div>;

  const entry = shelfEntry(work);
  const edited = work.edit_id !== null;
  // 編集のあとに取り込んだ改稿は、編集版を読んでいるあいだ見えない。
  const staleBase = staleEditBaseOf(work);
  const current = versions.find((version) => version.snapshot === work.snapshot_id) ?? versions[0] ?? null;
  const url = sourceUrl(work.provider, work.external_key, work.content_type, work.authorKey);
  const openSource = async () => {
    if (url) await openExternalUrl(url);
  };
  const media = mediaQuery.data ?? [];
  const visibleMedia = media.slice(0, visibleAssetCount);
  const previewImage = (item: ShelfMedia) => {
    const src = shelfFileUrl(item.file);
    if (!src) return;
    const name = item.name ?? item.role;
    modals.open({
      title: name,
      size: "min(92vw, 1100px)",
      yOffset: 24,
      radius: "lg",
      classNames: { content: "asset-preview-modal" },
      children: <div className="asset-preview"><div className="asset-preview__canvas"><Image src={src} alt={name} fit="contain" /></div><Group className="asset-preview__footer" justify="space-between"><Text size="sm" c="dimmed">{formatBytes(item.length)}{item.mime_type ? ` · ${item.mime_type.replace("image/", "").toUpperCase()}` : ""}</Text></Group></div>,
    });
  };

  return (
    <div className="page page--contained work-page">
      <Breadcrumbs mb="md" className="work-breadcrumbs"><Anchor component="button" type="button" size="sm" onClick={() => returnTo("/library")}>ライブラリ</Anchor><Text size="sm" c="dimmed" className="line-clamp-1">{work.title}</Text></Breadcrumbs>
      <Card className="work-hero" padding={0}>
        <Grid gap={0} align="stretch">
          <Grid.Col span={{ base: 12, sm: 4, lg: 3 }}>
            <Box className="work-hero__cover-stage">
              <WorkCover work={entry} variant="detail" className="work-hero__cover" />
            </Box>
          </Grid.Col>
          <Grid.Col span={{ base: 12, sm: 8, lg: 9 }}>
            <Stack p={{ base: "lg", lg: "xl" }} h="100%" gap={0}>
              <Stack gap="md" mb="md">
                <Box>
                  <div className="work-hero__toprow">
                    <Group gap="xs" wrap="wrap" className="work-hero__marks"><ProviderMark provider={work.provider} /><Badge variant="light" color="gray">{contentTypeLabel(work.content_type)}</Badge>{edited && <Badge color="gray" variant="light">ローカル編集</Badge>}<Badge color="yellow" variant="light">新しい棚（試験中）</Badge></Group>
                    {url && <Box className="work-hero__utility">
                      <ActionBar
                        label="作品の外へ"
                        items={[
                          { key: "in-app", label: "アプリ内で開く", icon: Icons.inAppBrowser, onClick: () => navigate(`/save/${work.provider}?url=${encodeURIComponent(url)}`) },
                          { key: "browser", label: "ブラウザで開く", icon: Icons.externalLink, onClick: openSource },
                        ]}
                      />
                    </Box>}
                  </div>
                  {work.series_title && (work.seriesKey
                    ? <Anchor component="button" type="button" size="sm" fw={650} c="dimmed" mb={5} display="block" ta="left" className="work-hero__series-link" onClick={() => navigate(`/series/${encodeURIComponent(work.provider)}/${encodeURIComponent(work.seriesKey!)}`)}>{work.series_title}</Anchor>
                    : <Text size="sm" c="dimmed" fw={650} mb={5}>{work.series_title}</Text>)}
                  <Title order={1} className="work-hero__title">{work.title}</Title>
                  {captionHtml && <ExpandableText className="work-caption" mt="sm" maw={820} lines={4} label="キャプション" html={captionHtml} onClick={openContentLink} />}
                </Box>
                <Group gap="lg">
                  <Button
                    variant="subtle"
                    color="gray"
                    px={0}
                    className="work-hero__author-link button-plain"
                    leftSection={<Avatar size={22} radius="xl" color="gray"><Icons.person size={IconSize.inline} /></Avatar>}
                    disabled={!work.authorKey}
                    onClick={() => work.authorKey && navigate(`/people/${encodeURIComponent(work.provider)}/${encodeURIComponent(work.authorKey)}`)}
                  >
                    {work.author_name}
                  </Button>
                </Group>
                <Group gap={8} className="work-hero__tags">{work.tags.map((tag) => <Badge key={tag} size="md" variant="light" color="gray" component={AppLink} className="work-hero__tag" to={`/library?q=${encodeURIComponent(`tag:${tag}`)}`}>#{tag}</Badge>)}</Group>
              </Stack>
              <div className="work-hero__actionbar">
                <Group gap="md" style={{ rowGap: 2 }} className="work-hero__facts">
                  <Text size="sm" c="dimmed"><Icons.publishedDate size={IconSize.menu} />公開 {formatDate(entry.sourceCreatedAt)}</Text>
                  <Text size="sm" c="dimmed"><Icons.textLength size={IconSize.menu} />{edited && "取得元 "}{formatNumber(work.text_length)}字</Text>
                  {work.media_count > 0 && <Text size="sm" c="dimmed"><Icons.assets size={IconSize.menu} />画像・添付 {formatNumber(work.media_count)}件</Text>}
                </Group>
                <Group gap="md" wrap="wrap" justify="flex-end" className="work-hero__actions">
                  <Group gap={6} wrap="nowrap" role="group" aria-label="中身へ進む">
                    <Button className="work-hero__action work-hero__read" leftSection={<Icons.read size={IconSize.action} />} onClick={() => navigate(`/reader/${workId}`)}>読む</Button>
                    <Tooltip label={editLabel}><ActionIcon className="work-hero__action" variant="default" size={38} aria-label={editLabel} disabled={!writable} onClick={() => navigate(`/editor/${workId}`)}><Icons.edit size={IconSize.action} /></ActionIcon></Tooltip>
                  </Group>
                  <Group gap={6} wrap="nowrap" role="group" aria-label="取得元を確認">
                    <Tooltip label={checkLabel}><ActionIcon className="work-hero__action" size={38} variant="default" aria-label={checkLabel} disabled={!writable} loading={checking} onClick={() => { void checkWork(); }}><Icons.watch size={IconSize.action} /></ActionIcon></Tooltip>
                  </Group>
                  <Group gap={6} wrap="nowrap" role="group" aria-label="手元の印">
                    <Tooltip label={favoriteLabel}><ActionIcon className="work-hero__action" size={38} variant="default" style={work.favorite ? { "--ai-color": "var(--flag-favorite)", "--ai-bg": "var(--flag-favorite-bg)" } : undefined} aria-label={favoriteLabel} aria-pressed={work.favorite} disabled={!writable} onClick={toggleFavorite}><Icons.favorite size={IconSize.action} fill={work.favorite ? "currentColor" : "none"} /></ActionIcon></Tooltip>
                  </Group>
                </Group>
              </div>
            </Stack>
          </Grid.Col>
        </Grid>
      </Card>

      <Tabs value={tab} onChange={setTab} mt="lg" keepMounted={false}>
        <Tabs.List>
          <Tabs.Tab value="overview">概要</Tabs.Tab>
          <Tabs.Tab value="content">本文</Tabs.Tab>
          <Tabs.Tab value="assets">画像・添付 <Badge size="xs" variant="light" ml={4}>{work.media_count}</Badge></Tabs.Tab>
          <Tabs.Tab value="history">履歴</Tabs.Tab>
          <Tabs.Tab value="json">保存データ</Tabs.Tab>
        </Tabs.List>
        <Tabs.Panel value="overview" pt="lg">
          <Grid gap="lg" align="stretch">
            <Grid.Col span={{ base: 12, md: 8 }}>
              <Card p="lg" h="100%"><Title order={3} mb="md">作品情報</Title><Text size="xs" c="dimmed" fw={700} mb="sm">取得元</Text><SimpleGrid cols={{ base: 1, sm: 2 }} spacing="lg"><Info label="作者" value={work.author_name} icon={<Icons.person size={IconSize.action} />} /><Info label="公開日" value={formatDate(entry.sourceCreatedAt, true)} icon={<Icons.publishedDate size={IconSize.action} />} />{entry.sourceUpdatedAt && entry.sourceUpdatedAt !== entry.sourceCreatedAt && <Info label="最終更新" value={formatDate(entry.sourceUpdatedAt, true)} icon={<Icons.updates size={IconSize.action} />} />}</SimpleGrid><Divider my="lg" /><Text size="xs" c="dimmed" fw={700} mb="sm">手元</Text><SimpleGrid cols={{ base: 1, sm: 2 }} spacing="lg"><Info label={edited ? "取得元の文字数" : "文字数"} value={`${formatNumber(work.text_length)}字`} icon={<Icons.read size={IconSize.action} />} /><Info label="容量" value={formatBytes(work.logical_size)} icon={<Icons.openFolder size={IconSize.action} />} /><Info label="バージョン" value={`v${current?.version ?? work.version_number}`} icon={<Icons.versionHistory size={IconSize.action} />} /><Info label="保存日" value={formatDate(entry.downloadedAt, true)} icon={<Icons.epubAdd size={IconSize.action} />} /></SimpleGrid></Card>
            </Grid.Col>
            <Grid.Col span={{ base: 12, md: 4 }}>
              <Stack gap="lg">
                {writable && edited ? <Card p="lg"><Group justify="space-between" wrap="nowrap" mb="xs"><Box><Text fw={700}>ローカル編集</Text><Text size="xs" c="dimmed" mt={3}>いま読める本文は編集版です</Text></Box><Badge color="gray" variant="light">反映中</Badge></Group><Text size="xs" c="dimmed" mb="sm">取り込んだ本文はそのまま残っています。戻しても、編集した版は履歴から選び直せます。</Text>{editHistory?.draft && <Text size="xs" c="yellow.8" mb="sm">このほかに、反映していない下書きがあります（{revisionDate(editHistory.draft)}保存）。</Text>}<ActionRow><Button variant="default" size="xs" leftSection={<Icons.undo size={IconSize.menu} />} loading={changing === "revert"} onClick={() => modals.openConfirmModal({ title: "取り込んだ本文に戻しますか？", children: <Text size="sm">リーダーとEPUBは取り込んだままの本文を使うようになります。編集した版は履歴に残ります。</Text>, labels: { confirm: "元に戻す", cancel: "キャンセル" }, onConfirm: () => { void revertEdit(); } })}>取り込んだ本文に戻す</Button></ActionRow></Card>
                  : writable && editHistory?.draft ? <Card p="lg"><Group justify="space-between" wrap="nowrap" mb="xs"><Box><Text fw={700}>ローカル編集</Text><Text size="xs" c="dimmed" mt={3}>いま読める本文は取り込んだままです</Text></Box><Badge color="yellow" variant="light">下書きのみ</Badge></Group><Text size="xs" c="dimmed" mb="sm">{revisionDate(editHistory.draft)}に保存した下書きは、編集画面で「反映」を押すまで読書画面とEPUBに使われません。</Text><ActionRow><Button variant="default" size="xs" leftSection={<Icons.edit size={IconSize.menu} />} onClick={() => navigate(`/editor/${workId}`)}>編集画面で開く</Button></ActionRow></Card>
                  : null}
                <Note title="新しい棚（試験中）">{writable
                  ? `正本に切り替えた棚です。お気に入りと編集はここで変えられます。${archived.length ? "以前に反映した編集版は、履歴タブからもう一度反映できます。" : ""}`
                  : "旧棚から移行した時点の写しを、読むだけで開いています。お気に入り・更新監視・編集・削除・コレクションは、旧棚の作品の画面で変えてください。"}</Note>
              </Stack>
            </Grid.Col>
          </Grid>
        </Tabs.Panel>
        <Tabs.Panel value="content" pt="lg">
          {contentQuery.isLoading ? <LoadingState label="本文を読み込んでいます" /> : contentQuery.error ? <ErrorState error={contentQuery.error} retry={() => contentQuery.refetch()} /> : !contentReady ? <LoadingState label="本文を読み込んでいます" /> : <Stack gap="sm">
            {contentSnapshot !== null && <Group justify="space-between"><Text size="xs" c="dimmed">保存済み v{versions.find((version) => version.snapshot === contentSnapshot)?.version ?? "?"}</Text><Button variant="subtle" size="xs" onClick={() => showVersionContent(null)}>現在の本文へ</Button></Group>}
            {contentSnapshot === null && writable && editHistory?.draft && <EditStatusStrip activeRevision={null} draftRevision={stripRevision(editHistory.draft, "draft")} onEdit={() => navigate(`/editor/${workId}`)} />}
            {contentSnapshot === null && edited && <Text size="xs" c="dimmed">{work.editBaseVersion === null ? "編集版の本文です。" : `v${work.editBaseVersion} をもとに編集した本文です。`}{staleBase !== null && `取り込み済みの v${work.version_number} は表示されていません。`}</Text>}
            {/* 取得元の応答に本文が無い作品（公開範囲の外の投稿など）。空の枠だけを出さない。 */}
            {contentQuery.data?.html.trim() ? <Paper className="content-frame" withBorder ref={contentFrameRef}>
              <div ref={contentBodyRef} className="content-preview content-preview--paged" onClick={handleContentClick} dangerouslySetInnerHTML={preparedMarkup} />
            </Paper> : <Alert color="gray">この版には本文がありません。</Alert>}
            <ContentPagination current={contentPage} total={contentQuery.data?.pageCount ?? 1} onChange={goToContentPage} />
          </Stack>}
        </Tabs.Panel>
        <Tabs.Panel value="assets" pt="lg">
          <Group justify="space-between" mb="md"><Text size="xs" c="dimmed">選んだ保存版の画像・添付を表示します。</Text><Select size="xs" w={160} aria-label="画像・添付の保存版" value={assetSnapshot ?? "current"} onChange={(value) => { setAssetSnapshot(value && value !== "current" ? value : null); setVisibleAssetCount(80); }} allowDeselect={false} data={[{ value: "current", label: `現在（v${current?.version ?? work.version_number}）` }, ...versions.filter((version) => version.snapshot !== current?.snapshot).map((version) => ({ value: version.snapshot, label: `保存済み v${version.version}` }))]} /></Group>
          {mediaQuery.isLoading ? <LoadingState label="画像・添付を読み込んでいます" /> : mediaQuery.error ? <ErrorState error={mediaQuery.error} retry={() => mediaQuery.refetch()} /> : media.length ? <Stack gap="lg"><SimpleGrid cols={{ base: 2, sm: 3, lg: 4, xl: 5 }} spacing="md">{visibleMedia.map((item, index) => {
            const src = item.kind === "media" ? shelfFileUrl(item.file) : null;
            const name = item.name ?? item.role;
            // PDF は外のアプリへ渡さない方針なので、旧棚と同じくアプリの中で紙面を見せる。
            const pdf = isShelfPdf(item) ? item.file : null;
            const open = src ? () => previewImage(item) : pdf ? () => setPdfOriginal({ workId, localPath: pdf, fileName: name }) : undefined;
            return <Card key={`${item.kind}-${index}`} p={0} className={open ? "asset-card surface--interactive" : "asset-card"} onClick={open}>
              <Box className="asset-card__preview">
                {src ? <Image src={src} alt={name} className="asset-card__image" loading="lazy" decoding="async" /> : <ThemeIcon size={56} variant="light" color="gray"><Icons.file size={IconSize.feature} /></ThemeIcon>}
                {pdf && <Badge className="asset-card__badge" size="xs" color="gray" variant="filled">原本を見る</Badge>}
                {!item.file && <Badge className="asset-card__badge" size="xs" color="gray" variant="filled">未保存</Badge>}
              </Box>
              <Stack gap={2} className="asset-card__meta">
                <Text size="xs" fw={650} className="asset-card__name" title={name}>{name}</Text>
                <Group justify="space-between" gap="xs" wrap="nowrap">
                  <Text size="10px" c="dimmed">{formatBytes(item.length)}</Text>
                  <Text size="10px" c="dimmed" tt="uppercase">{(item.mime_type || item.kind).split("/").pop()}</Text>
                </Group>
              </Stack>
            </Card>;
          })}</SimpleGrid>{visibleMedia.length < media.length && <Group justify="center"><Button variant="default" onClick={() => setVisibleAssetCount((count) => Math.min(media.length, count + 80))}>さらに表示（残り{formatNumber(media.length - visibleMedia.length)}件）</Button></Group>}</Stack> : <Alert color="gray" icon={<Icons.imageFile size={IconSize.action} />}>この版に画像・添付はありません。</Alert>}
        </Tabs.Panel>
        <Tabs.Panel value="history" pt="lg">
          {edited && <Alert color="blue" mb="md">編集しても取得元の版は増えません。編集版は{work.editBaseVersion === null ? "取得元の版" : ` v${work.editBaseVersion} `}の上に重ねて保存してあり、いま読む本文に使います。</Alert>}
          <div className="history-layout">
            <nav className="history-layout__versions" aria-label="保存した版">
              {editHistory?.active && <Paper p="xs" withBorder className="version-row version-row--static">
                <Group justify="space-between" wrap="nowrap" gap={6}><Text size="sm" fw={700}>編集版</Text><Badge size="xs" color="blue">現在</Badge></Group>
                <Text size="xs" c="dimmed">v{editHistory.active.baseVersion} をもとに</Text>
                <Text size="xs" c="dimmed">{revisionDate(editHistory.active)}</Text>
              </Paper>}
              {editHistory?.draft && <Paper p="xs" withBorder className="version-row version-row--static">
                <Group justify="space-between" wrap="nowrap" gap={6}><Text size="sm" fw={700}>下書き</Text><Badge size="xs" color="yellow" variant="light">未反映</Badge></Group>
                <Text size="xs" c="dimmed">{revisionDate(editHistory.draft)}</Text>
                {writable && <Button mt={6} size="compact-xs" variant="default" fullWidth onClick={() => navigate(`/editor/${workId}`)}>編集画面で開く</Button>}
              </Paper>}
              {writable && archived.length >= 2 && <Group justify="space-between" wrap="nowrap" gap={6} px={4}>
                <Text size="xs" c="dimmed">以前の編集版 {formatNumber(archived.length)}件</Text>
                <DangerButton size="compact-xs" loading={changing === "forget-all"} onClick={() => confirmForget(null)}>すべて外す</DangerButton>
              </Group>}
              {archived.map((revision) => <Paper key={`edit-${revision.id}`} p="xs" withBorder className="version-row version-row--static">
                <Group justify="space-between" wrap="nowrap" gap={6}><Text size="sm" fw={700}>以前の編集版</Text><Badge size="xs" color="gray" variant="light">未使用</Badge></Group>
                <Text size="xs" c="dimmed">v{revision.baseVersion} をもとに</Text>
                <Text size="xs" c="dimmed">{revisionDate(revision)}</Text>
                {writable && <>
                  <Button mt={6} size="compact-xs" variant="default" fullWidth loading={changing === `restore-${revision.id}`} onClick={() => confirmRestore(revision)}>もう一度反映</Button>
                  <DangerButton mt={4} size="compact-xs" fullWidth loading={changing === `forget-${revision.id}`} onClick={() => confirmForget(revision)}>一覧から外す</DangerButton>
                </>}
              </Paper>)}
              {versions.map((version) => <Paper component="button" type="button" key={version.snapshot} p="xs" withBorder className="version-row" data-active={selected?.snapshot === version.snapshot || undefined} onClick={() => setSelectedSnapshot(version.snapshot)}>
                <Group justify="space-between" wrap="nowrap" gap={6}><Text size="sm" fw={700}>v{version.version}</Text>{version.snapshot === current?.snapshot && <Badge size="xs">{edited ? "取得元の最新版" : "現在"}</Badge>}</Group>
                <Text size="xs" c="dimmed">{versionDate(version)}</Text>
                {version.text_length !== null && <Text size="xs" c="dimmed">{formatNumber(version.text_length)}字</Text>}
              </Paper>)}
            </nav>
            <Card p="lg" className="version-preview">
              {!selected
                ? <Alert color="gray">保存した版がありません。</Alert>
                : selectedText.isLoading || (previous && previousText.isLoading)
                  ? <LoadingState label="変更内容を読み込んでいます" />
                  : selectedText.error || previousText.error
                    ? <ErrorState error={selectedText.error ?? previousText.error} retry={() => { selectedText.refetch(); previousText.refetch(); }} />
                    : <RevisionDiff
                        beforeVersion={previous?.version ?? null}
                        afterVersion={selected.version}
                        beforeText={previous ? previousText.data ?? "" : null}
                        afterText={selectedText.data ?? ""}
                        actions={<Button size="xs" variant="light" onClick={() => showVersionContent(selected.snapshot === current?.snapshot && !edited ? null : selected.snapshot)}>この版の本文を見る</Button>}
                      />}
            </Card>
          </div>
        </Tabs.Panel>
        <Tabs.Panel value="json" pt="lg">{originalQuery.isLoading ? <LoadingState /> : originalQuery.error ? <ErrorState error={originalQuery.error} retry={() => originalQuery.refetch()} /> : originalQuery.data ? <BoundedJsonView jsonText={originalQuery.data} description="現在の版を作ったときに取得元から受け取ったデータを表示します（JSON形式）" /> : <Alert color="gray">この版には取得元から受け取ったデータが残っていません。</Alert>}</Tabs.Panel>
      </Tabs>
      <PdfOriginalViewer target={pdfOriginal} onClose={() => setPdfOriginal(null)} />
    </div>
  );
}
