import { useEffect, useMemo, useRef, useState } from "react";
import {
  ActionIcon,
  Badge,
  Box,
  Button,
  Divider,
  Drawer,
  Group,
  NumberInput,
  Pagination,
  Paper,
  Popover,
  Progress,
  Radio,
  ScrollArea,
  SegmentedControl,
  Select,
  Slider,
  Stack,
  Text,
  Title,
  Tooltip,
} from "@mantine/core";
import { useDisclosure, useLocalStorage } from "@mantine/hooks";
import { notifications } from "@mantine/notifications";
import { ArrowLeft, ArrowUp, Bookmark as BookmarkIcon, BookmarkPlus, BookOpen, Edit3, Expand, ExternalLink, FolderOpen, Shrink, Settings2, Type, X } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { useBookmarks, type Bookmark } from "@/features/reader/bookmarks";
import { useAppNavigate, useAppSearchParams, useRouteParams } from "@/app/router";
import { ErrorState, LoadingState } from "@/components/AsyncState";
import { ProviderMark, sourceUrl } from "@/lib/providers";
import { formatDate, formatNumber } from "@/lib/format";
import { prepareDocumentHtml, splitDocumentPages } from "@/lib/content";
import { extractSavedSourceTarget } from "@/features/browser/downloadCandidates";
import { getAssetUrl, getDownloadBySource, getReaderDocument, isTauriRuntime, openLocalAsset } from "@/services/dbApi";
import { openExternalUrl } from "@/services/openerApi";
import { getDemoReader } from "@/mocks/demoData";

function BookmarkControls({ bookmarks, onAdd, onOpen, onRemove }: {
  bookmarks: Bookmark[];
  onAdd: () => void;
  onOpen: (bookmark: Bookmark) => void;
  onRemove: (id: string) => void;
}) {
  const [opened, setOpened] = useState(false);
  return (
    <Group gap={2} wrap="nowrap">
      <Tooltip label="現在の位置にしおりを挟む">
        <ActionIcon variant="subtle" color="gray" aria-label="現在の位置にしおりを挟む" onClick={onAdd}><BookmarkPlus size={18} /></ActionIcon>
      </Tooltip>
      <Popover opened={opened} onChange={setOpened} position="bottom-end" withArrow shadow="md" width={280}>
        <Popover.Target>
          <Tooltip label="しおり一覧">
            {/* A plain icon button like its neighbour, with the count drawn by
                us: Mantine's Indicator both clipped the number and shifted the
                glyph out of line with the rest of the bar. */}
            <ActionIcon className="reader-bookmark" variant="subtle" color="gray" aria-label={`しおり一覧（${bookmarks.length}件）`} onClick={() => setOpened((value) => !value)}>
              <BookmarkIcon size={18} />
              {bookmarks.length > 0 && <span className="reader-bookmark__count">{bookmarks.length}</span>}
            </ActionIcon>
          </Tooltip>
        </Popover.Target>
        <Popover.Dropdown>
          <Text size="xs" fw={700} mb="xs">しおり</Text>
          {bookmarks.length ? (
            <Stack gap={4}>
              {bookmarks.map((bookmark) => (
                <Group key={bookmark.id} gap={4} wrap="nowrap">
                  <Button variant="subtle" color="gray" size="compact-xs" justify="flex-start" flex={1} onClick={() => { setOpened(false); onOpen(bookmark); }}>
                    {bookmark.label}
                  </Button>
                  <ActionIcon variant="subtle" color="red" size="sm" aria-label={`${bookmark.label}のしおりを削除`} onClick={() => onRemove(bookmark.id)}><X size={14} /></ActionIcon>
                </Group>
              ))}
            </Stack>
          ) : <Text size="xs" c="dimmed">まだしおりはありません。左のボタンで現在地を記録できます。</Text>}
        </Popover.Dropdown>
      </Popover>
    </Group>
  );
}

interface ReaderSettings {
  fontSize: number;
  lineHeight: number;
  maxWidth: number;
  font: "serif" | "sans";
  theme: "paper" | "white" | "night";
}

const defaults: ReaderSettings = { fontSize: 18, lineHeight: 1.72, maxWidth: 680, font: "serif", theme: "white" };

export default function ReaderPage() {
  const navigate = useAppNavigate();
  const { workId } = useRouteParams("/reader/:workId");
  const [searchParams] = useAppSearchParams();
  const id = Number(workId);
  const runtime = isTauriRuntime();
  const requestedVersion = Number(searchParams.get("version"));
  const [version, setVersion] = useState<number | null>(Number.isFinite(requestedVersion) && requestedVersion > 0 ? requestedVersion : null);
  const [settings, setSettings] = useLocalStorage<ReaderSettings>({ key: "piep.reader-settings.v4", defaultValue: defaults });
  const [settingsOpened, settingsDrawer] = useDisclosure(false);
  const [progress, setProgress] = useState(0);
  const [sourcePage, setSourcePage] = useState(1);
  const [jumpPage, setJumpPage] = useState<number | string>(1);
  const [jumpOpened, setJumpOpened] = useState(false);
  const [fullscreen, setFullscreen] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);
  const restoredKeyRef = useRef<string | null>(null);
  const { bookmarks, add: addBookmark, remove: removeBookmark } = useBookmarks(id);
  const query = useQuery({
    queryKey: ["reader-document", id, version],
    queryFn: () => runtime ? getReaderDocument(id, version) : Promise.resolve(getDemoReader(id)),
    enabled: Number.isFinite(id),
  });
  const preparedHtml = useMemo(() => prepareDocumentHtml(query.data?.html ?? "", getAssetUrl), [query.data?.html]);
  const sourcePages = useMemo(() => splitDocumentPages(preparedHtml, query.data?.download.source ?? ""), [preparedHtml, query.data?.download.source]);
  const hasSourcePages = query.data?.download.source === "pixiv" && sourcePages.length > 1;
  const positionKey = `piep.reader-position.${id}.${version ?? "current"}`;

  useEffect(() => {
    const update = () => setFullscreen(Boolean(document.fullscreenElement));
    document.addEventListener("fullscreenchange", update);
    update();
    return () => document.removeEventListener("fullscreenchange", update);
  }, []);
  useEffect(() => { setJumpPage(sourcePage); }, [sourcePage]);

  // Restoring is declared before persisting on purpose. Effects run in
  // declaration order, so persisting first would write the not-yet-restored
  // scrollTop of 0 over the stored position and lose the reader's place.
  useEffect(() => {
    const viewport = scrollRef.current;
    if (!viewport || !query.data) return;
    if (restoredKeyRef.current === positionKey) return;
    let savedPage = 1;
    let savedTop = 0;
    try {
      const saved = JSON.parse(sessionStorage.getItem(positionKey) ?? "{}");
      savedPage = Math.min(Math.max(1, Number(saved.page) || 1), Math.max(1, sourcePages.length));
      savedTop = Math.max(0, Number(saved.top) || 0);
    } catch { /* Legacy or invalid session value: start at the beginning. */ }
    setSourcePage(savedPage);
    // The document is still laying out on the first frame, so assigning the
    // saved offset there just clamps it to 0. Retry until it sticks, and only
    // then record that this document has been restored - marking it up front
    // meant a cancelled first attempt was never retried.
    let cancelled = false;
    let frame = 0;
    const deadline = performance.now() + 2000;
    const attempt = () => {
      if (cancelled) return;
      viewport.scrollTop = savedTop;
      if (Math.abs(viewport.scrollTop - savedTop) < 2 || performance.now() >= deadline) {
        restoredKeyRef.current = positionKey;
        return;
      }
      frame = requestAnimationFrame(attempt);
    };
    frame = requestAnimationFrame(attempt);
    return () => { cancelled = true; cancelAnimationFrame(frame); };
  }, [positionKey, query.data, sourcePages.length]);
  useEffect(() => {
    const viewport = scrollRef.current;
    if (!viewport) return;
    const update = (event?: Event) => {
      const total = viewport.scrollHeight - viewport.clientHeight;
      const next = total <= 0 ? 100 : Math.max(0, Math.min(100, viewport.scrollTop / total * 100));
      setProgress(hasSourcePages ? ((sourcePage - 1) + next / 100) / sourcePages.length * 100 : next);
      // Only a genuine scroll writes the position. The priming call below runs
      // before the saved offset has been applied, so persisting there would
      // overwrite the stored place with 0 each time the reader opens.
      if (event) sessionStorage.setItem(positionKey, JSON.stringify({ page: sourcePage, top: viewport.scrollTop }));
    };
    viewport.addEventListener("scroll", update, { passive: true });
    update();
    return () => viewport.removeEventListener("scroll", update);
  }, [hasSourcePages, positionKey, query.data, sourcePage, sourcePages.length]);

  if (query.isLoading) return <div className="page"><LoadingState label="リーダーを準備しています" /></div>;
  if (query.error || !query.data) return <div className="page"><ErrorState error={query.error ?? "作品が見つかりません"} retry={() => query.refetch()} /></div>;
  const doc = query.data;
  const work = doc.download;
  const currentVersion = version ?? work.currentVersion;
  const openSource = async () => {
    const url = sourceUrl(work.source, work.sourceId, work.contentType, work.personId || work.authorId);
    if (!url) return;
    if (runtime) await openExternalUrl(url); else window.open(url, "_blank", "noopener,noreferrer");
  };
  const handleArticleClick = async (event: React.MouseEvent<HTMLElement>) => {
    const anchor = (event.target as HTMLElement).closest<HTMLAnchorElement>("a[href]");
    if (!anchor) return;
    const jumpPage = Number(anchor.dataset.page);
    if (anchor.classList.contains("jump-link") && Number.isFinite(jumpPage)) {
      event.preventDefault();
      setSourcePage(Math.min(Math.max(1, jumpPage), sourcePages.length));
      scrollRef.current?.scrollTo({ top: 0, behavior: "smooth" });
      return;
    }
    const href = anchor.href;
    if (!href.startsWith("http")) return;
    event.preventDefault();
    const target = extractSavedSourceTarget(href);
    if (runtime && target) {
      const saved = await getDownloadBySource(target.source, target.sourceId);
      if (saved) {
        navigate(`/reader/${saved.id}`);
        return;
      }
    }
    if (runtime) await openExternalUrl(href); else window.open(href, "_blank", "noopener,noreferrer");
  };
  const goToSourcePage = (page: number) => {
    const next = Math.min(Math.max(1, page), sourcePages.length);
    if (next === sourcePage) return;
    setSourcePage(next);
    scrollRef.current?.scrollTo({ top: 0, behavior: "smooth" });
  };
  const addBookmarkHere = () => {
    const viewport = scrollRef.current;
    const top = Math.round(viewport?.scrollTop ?? 0);
    // Measured here rather than read from the progress state, which only
    // catches up on the next scroll event.
    const scrollable = (viewport?.scrollHeight ?? 0) - (viewport?.clientHeight ?? 0);
    const percent = scrollable > 0 ? Math.round(top / scrollable * 100) : 0;
    const label = `${hasSourcePages ? `${sourcePage}ページ · ` : ""}${percent}%`;
    addBookmark({ page: sourcePage, top, label });
    notifications.show({ color: "teal", message: `しおりを挟みました（${label}）` });
  };
  const jumpToBookmark = (bookmark: { page: number; top: number }) => {
    setSourcePage(Math.min(Math.max(1, bookmark.page), Math.max(1, sourcePages.length)));
    // The page swap re-renders the article, so the offset is applied after it.
    requestAnimationFrame(() => scrollRef.current?.scrollTo({ top: bookmark.top, behavior: "smooth" }));
  };

  return (
    <div className={`reader-page reader-theme--${settings.theme}`}>
      <header className="reader-toolbar">
        <Group h="100%" px="md" justify="space-between" wrap="nowrap">
          <Group wrap="nowrap" miw={0}>
            <Tooltip label="作品詳細へ戻る"><ActionIcon variant="subtle" color="gray" aria-label="作品詳細へ戻る" onClick={() => navigate(`/works/${work.id}`)}><ArrowLeft size={19} /></ActionIcon></Tooltip>
            <Divider orientation="vertical" h={24} />
            {/* Title only: the provider and author already head the document
                itself, and repeating them here just crowded the bar. */}
            <Text size="sm" fw={700} className="line-clamp-1">{work.title}</Text>
          </Group>
          <Group gap="xs" wrap="nowrap">
            {doc.isEdited && <Badge color="violet" variant="light" visibleFrom="sm">ローカル編集</Badge>}
            <BookmarkControls
              bookmarks={bookmarks}
              onAdd={addBookmarkHere}
              onOpen={jumpToBookmark}
              onRemove={removeBookmark}
            />
            <Select
              size="xs"
              value={String(currentVersion)}
              onChange={(value) => setVersion(value ? Number(value) : null)}
              data={doc.versions.map((item) => ({ value: String(item.version), label: `v${item.version}` }))}
              w={84}
              aria-label="表示バージョン"
            />
            <Tooltip label="表示設定"><ActionIcon variant="subtle" color="gray" aria-label="リーダーの表示設定" onClick={settingsDrawer.open}><Settings2 size={18} /></ActionIcon></Tooltip>
            <Tooltip label={fullscreen ? "全画面を終了" : "全画面で読む"}><ActionIcon variant="subtle" color="gray" aria-label={fullscreen ? "全画面を終了" : "全画面表示"} onClick={() => fullscreen ? document.exitFullscreen() : document.documentElement.requestFullscreen()}>{fullscreen ? <Shrink size={18} /> : <Expand size={18} />}</ActionIcon></Tooltip>
            <Button variant="subtle" color="gray" size="xs" leftSection={<Edit3 size={14} />} onClick={() => navigate(`/editor/${work.id}`)}>編集</Button>
          </Group>
        </Group>
        <Progress value={progress} h={2} radius={0} aria-label={`読書進捗 ${Math.round(progress)}%`} />
      </header>

      <ScrollArea className="reader-scroll" viewportRef={scrollRef} type="auto" scrollbarSize={8}>
        <Box className="reader-stage">
          <Box
            className="reader-paper"
            style={{
              "--reader-font-size": `${settings.fontSize}px`,
              "--reader-line-height": settings.lineHeight,
              "--reader-max-width": `${settings.maxWidth}px`,
              "--reader-font-family": settings.font === "serif" ? '"Noto Serif JP", "Yu Mincho", serif' : '"Noto Sans JP", "Yu Gothic UI", sans-serif',
            }}
          >
            {sourcePage === 1 && <Box className="reader-title-page">
              <ProviderMark provider={work.source} />
              <Title order={1} className="reader-title-page__title">{work.title}</Title>
              <Text c="dimmed" className="reader-title-page__author">{work.authorName}</Text>
              <Text size="xs" c="dimmed" className="reader-title-page__meta">{formatNumber(work.textLength)}字 · {formatDate(work.sourceCreatedAt)}</Text>
              {/* Two destinations that were both called "保存元": one opens the
                  page on the web, the other the folder on this machine. */}
              <Group gap={4} mt="lg" justify="flex-end" wrap="wrap">
                <Button variant="subtle" color="gray" size="compact-xs" rightSection={<ExternalLink size={12} />} onClick={openSource}>元ページ</Button>
                <Button variant="subtle" color="gray" size="compact-xs" rightSection={<FolderOpen size={12} />} disabled={!runtime} onClick={() => runtime && openLocalAsset(work.jsonPath)}>保存フォルダー</Button>
              </Group>
            </Box>}
            {sourcePage === 1 && <Divider my="xl" />}
            <article className="reader-content" onClick={handleArticleClick} dangerouslySetInnerHTML={{ __html: sourcePages[sourcePage - 1] ?? "" }} />
            {sourcePage === sourcePages.length && <Box className="reader-finish"><BookOpen size={28} /><Text fw={700}>読了</Text><Button variant="light" onClick={() => navigate(`/works/${work.id}`)}>作品詳細へ戻る</Button></Box>}
          </Box>
        </Box>
      </ScrollArea>
      {hasSourcePages && <Paper className="reader-page-controls" shadow="lg" withBorder radius="md" p={5} aria-label="pixiv原稿のページ操作">
        <Group gap={5} wrap="nowrap">
          <Pagination total={sourcePages.length} value={sourcePage} onChange={goToSourcePage} siblings={1} boundaries={1} withEdges size="sm" radius="sm" />
          <Divider orientation="vertical" h={24} />
          <Popover opened={jumpOpened} onChange={setJumpOpened} position="top" withArrow shadow="md">
            <Popover.Target><Button variant="subtle" color="gray" size="compact-sm" onClick={() => setJumpOpened((value) => !value)} aria-label="ページ番号を指定">{sourcePage} / {sourcePages.length}</Button></Popover.Target>
            <Popover.Dropdown>
              <Stack gap="xs" w={180}>
                <Text size="xs" fw={700}>移動先のページ</Text>
                <NumberInput min={1} max={sourcePages.length} value={jumpPage} onChange={setJumpPage} clampBehavior="strict" aria-label="移動先ページ" />
                <Button size="xs" onClick={() => { goToSourcePage(Number(jumpPage) || 1); setJumpOpened(false); }}>移動</Button>
              </Stack>
            </Popover.Dropdown>
          </Popover>
        </Group>
      </Paper>}
      {progress > 8 && <Tooltip label="先頭へ戻る"><ActionIcon className="reader-to-top" size="lg" radius="xl" variant="filled" aria-label="本文の先頭へ戻る" onClick={() => scrollRef.current?.scrollTo({ top: 0, behavior: "smooth" })}><ArrowUp size={18} /></ActionIcon></Tooltip>}

      <Drawer opened={settingsOpened} onClose={settingsDrawer.close} position="right" title="読書設定" size={360}>
        <Stack gap="xl">
          <Box><Group justify="space-between" mb="sm"><Text size="sm" fw={700}>文字サイズ</Text><Text size="xs" c="dimmed">{settings.fontSize}px</Text></Group><Slider min={14} max={28} step={1} value={settings.fontSize} onChange={(fontSize) => setSettings({ ...settings, fontSize })} marks={[{ value: 14, label: "小" }, { value: 21, label: "標準" }, { value: 28, label: "大" }]} /></Box>
          <Box><Group justify="space-between" mb="sm"><Text size="sm" fw={700}>行間</Text><Text size="xs" c="dimmed">{settings.lineHeight.toFixed(2)}</Text></Group><Slider min={1.4} max={2.4} step={0.05} value={settings.lineHeight} onChange={(lineHeight) => setSettings({ ...settings, lineHeight })} /></Box>
          <Box><Group justify="space-between" mb="sm"><Text size="sm" fw={700}>本文幅</Text><Text size="xs" c="dimmed">{settings.maxWidth}px</Text></Group><Slider min={520} max={900} step={20} value={settings.maxWidth} onChange={(maxWidth) => setSettings({ ...settings, maxWidth })} /></Box>
          <Box><Text size="sm" fw={700} mb="sm">書体</Text><SegmentedControl fullWidth value={settings.font} onChange={(font) => setSettings({ ...settings, font: font as ReaderSettings["font"] })} data={[{ value: "serif", label: "明朝" }, { value: "sans", label: "ゴシック" }]} /></Box>
          <Box><Text size="sm" fw={700} mb="sm">背景</Text><Radio.Group value={settings.theme} onChange={(theme) => setSettings({ ...settings, theme: theme as ReaderSettings["theme"] })}><Stack gap="xs"><Radio value="paper" label="紙の色" /><Radio value="white" label="白" /><Radio value="night" label="ナイト" /></Stack></Radio.Group></Box>
          <Button variant="default" leftSection={<Type size={15} />} onClick={() => setSettings(defaults)}>初期設定に戻す</Button>
        </Stack>
      </Drawer>
    </div>
  );
}
