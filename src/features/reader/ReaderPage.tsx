import { useEffect, useMemo, useRef, useState } from "react";
import {
  ActionIcon,
  Alert,
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
  TextInput,
} from "@mantine/core";
import { useDebouncedValue, useDisclosure, useLocalStorage } from "@mantine/hooks";
import { notifications } from "@mantine/notifications";
import { Icons, IconSize } from "@/lib/icons";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useBookmarks, type Bookmark } from "@/features/reader/bookmarks";
import { readReadingPosition, writeReadingPosition } from "@/features/library/readingShelf";
import { useAppNavigate, useAppSearchParams, useReturnTo, useRouteParams } from "@/app/router";
import { ErrorState, LoadingState } from "@/components/AsyncState";
import { ProviderMark, sourceUrl } from "@/lib/providers";
import { formatDate, formatNumber } from "@/lib/format";
import { prepareDocumentHtml } from "@/lib/content";
import { useContentLinkNavigation } from "@/lib/contentLinks";
import { getWorkCollection, listCollectionsForWork } from "@/services/collectionApi";
import { getAssetUrl, getReaderContentPage, getReaderMetadata, isTauriRuntime, openLocalAsset, searchDownloadsV2, searchReaderContent } from "@/services/dbApi";
import { openExternalUrl } from "@/services/openerApi";
import { getDemoReader } from "@/mocks/demoData";
import { RecapPanel } from "@/features/assist/RecapPanel";
import type { WorkCollection } from "@/types/collections";
import type { SearchV2Params, SearchV2Result } from "@/types/library";

interface ReadingContextRow {
  key: string;
  label: string;
  detail: string;
  collectionId?: string;
  /** Ordered collections and official series have a real reading order, so the
   *  neighbours can be called 前 / 次. An unordered collection only has a
   *  display arrangement: calling its neighbours 続き would invent a sequence
   *  the reader never asked for. */
  sequential: boolean;
  previous: { id: number; title: string } | null;
  next: { id: number; title: string } | null;
  /** いま読んでいる作品。「前回のあらすじ」をここにぶら下げる。 */
  currentId: number;
}

function adjacentWorks(items: { id: number; title: string }[], currentId: number) {
  const index = items.findIndex((item) => item.id === currentId);
  return {
    position: index >= 0 ? index + 1 : null,
    total: items.length,
    previous: index > 0 ? items[index - 1] : null,
    next: index >= 0 && index + 1 < items.length ? items[index + 1] : null,
  };
}

interface ReaderSequenceResult extends SearchV2Result {
  truncated: boolean;
}

/**
 * Walks an ordered search until the current work and its following neighbour
 * are both known. Most series finish in one request; long-running series keep
 * following the opaque cursor instead of pretending item 200 is the end.
 */
async function loadReaderSequence(params: SearchV2Params, currentId: number): Promise<ReaderSequenceResult> {
  const items: SearchV2Result["items"] = [];
  const seenCursors = new Set<string>();
  let cursor: string | null = null;
  let first: SearchV2Result | null = null;
  const maxPages = 25;

  for (let page = 0; page < maxPages; page += 1) {
    const result = await searchDownloadsV2({ ...params, limit: 200, cursor });
    first ??= result;
    const knownIds = new Set(items.map((item) => item.id));
    items.push(...result.items.filter((item) => !knownIds.has(item.id)));
    const position = items.findIndex((item) => item.id === currentId);
    if (position >= 0 && (position < items.length - 1 || !result.nextCursor)) {
      return { ...first, items, nextCursor: result.nextCursor, truncated: false };
    }
    if (!result.nextCursor || seenCursors.has(result.nextCursor)) {
      return { ...first, items, nextCursor: null, truncated: false };
    }
    seenCursors.add(result.nextCursor);
    cursor = result.nextCursor;
  }

  if (!first) throw new Error("作品の並びを読み込めませんでした");
  return { ...first, items, nextCursor: cursor, truncated: true };
}

/** One continuation route offered at the end of a work.
 *
 *  An ordered collection or an official series really does have a next
 *  instalment, so it gets 前の作品 / 次の作品. Anything without a reading order
 *  — an unordered collection, an author's posting history — offers the same
 *  neighbours but never calls them the continuation, because presenting a
 *  themed set as a serial is exactly the misreading the arrangement invites. */
function ReadingContextCard({ context, onOpen }: {
  context: ReadingContextRow;
  onOpen: (path: string) => void;
}) {
  const href = (id: number) => `/reader/${id}${context.collectionId ? `?collection=${encodeURIComponent(context.collectionId)}` : ""}`;
  const previousLabel = context.sequential ? "前の作品はありません" : "ほかの作品はありません";
  const nextLabel = context.sequential ? "次の作品はありません" : "ほかの作品はありません";
  return (
    <Paper withBorder p="sm" radius="md">
      <Stack gap="xs">
        <Group justify="space-between">
          <Box>
            <Text size="sm" fw={700}>{context.label}</Text>
            <Text size="xs" c="dimmed">{context.detail}</Text>
          </Box>
          {context.collectionId && <Button size="compact-xs" variant="subtle" onClick={() => onOpen(`/collections/${context.collectionId}`)}>一覧</Button>}
        </Group>
        {!context.sequential && <Text size="xs" c="dimmed">読む順は決まっていません。同じまとまりの作品です。</Text>}
        {/* 読む順があって、前の話があるときだけ。順序なしのまとまりで
            「前回のあらすじ」を出すと、無い連続性をあるように見せてしまう。 */}
        {context.sequential && context.previous && (
          <RecapPanel currentId={context.currentId} previous={context.previous} />
        )}
        <Group grow>
          {context.previous
            ? <Button variant="default" leftSection={context.sequential ? <Icons.previous size={IconSize.menu} /> : undefined} onClick={() => onOpen(href(context.previous!.id))}>{context.previous.title}</Button>
            : <Button variant="default" disabled>{previousLabel}</Button>}
          {context.next
            ? <Button variant={context.sequential ? "filled" : "default"} rightSection={context.sequential ? <Icons.next size={IconSize.menu} /> : undefined} onClick={() => onOpen(href(context.next!.id))}>{context.next.title}</Button>
            : <Button variant="default" disabled>{nextLabel}</Button>}
        </Group>
      </Stack>
    </Paper>
  );
}

function BookmarkControls({ bookmarks, addDisabled, onAdd, onOpen, onRemove }: {
  bookmarks: Bookmark[];
  addDisabled?: boolean;
  onAdd: () => void;
  onOpen: (bookmark: Bookmark) => void;
  onRemove: (id: string) => void;
}) {
  const [opened, setOpened] = useState(false);
  return (
    <Group gap={2} wrap="nowrap">
      <Tooltip label="現在の位置にしおりを挟む">
        <ActionIcon variant="subtle" color="gray" aria-label="現在の位置にしおりを挟む" disabled={addDisabled} onClick={onAdd}><Icons.saveSearch size={IconSize.nav} /></ActionIcon>
      </Tooltip>
      <Popover opened={opened} onChange={setOpened} position="bottom-end" withArrow shadow="md" width={280}>
        <Popover.Target>
          <Tooltip label="しおり一覧">
            {/* A plain icon button like its neighbour, with the count drawn by
                us: Mantine's Indicator both clipped the number and shifted the
                glyph out of line with the rest of the bar. */}
            <ActionIcon className="reader-bookmark" variant="subtle" color="gray" aria-label={`しおり一覧（${bookmarks.length}件）`} onClick={() => setOpened((value) => !value)}>
              <Icons.savedSearch size={IconSize.nav} />
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
                  <ActionIcon variant="subtle" color="red" size="sm" aria-label={`${bookmark.label}のしおりを削除`} onClick={() => onRemove(bookmark.id)}><Icons.cancel size={IconSize.menu} /></ActionIcon>
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
  // A saved work reached from inside the text opens in the reader: somebody
  // following "前編はこちら" mid-chapter wants to keep reading.
  const openContentLink = useContentLinkNavigation({ workRoute: (id) => `/reader/${id}` });
  // The detail screen is normally the entry behind this one, so "戻る" goes
  // back to it - keeping its tab and scroll position - instead of pushing a
  // fresh copy and leaving the header pointing back into the reader.
  const returnTo = useReturnTo();
  const { workId } = useRouteParams("/reader/:workId");
  const [searchParams] = useAppSearchParams();
  const id = Number(workId);
  const runtime = isTauriRuntime();
  const queryClient = useQueryClient();
  const requestedVersion = Number(searchParams.get("version"));
  const requestedCollectionId = searchParams.get("collection")?.trim() || null;
  const normalizedRequestedVersion = Number.isFinite(requestedVersion) && requestedVersion > 0 ? requestedVersion : null;
  const [version, setVersion] = useState<number | null>(normalizedRequestedVersion);
  const [settings, setSettings] = useLocalStorage<ReaderSettings>({ key: "piep.reader-settings.v4", defaultValue: defaults });
  const [settingsOpened, settingsDrawer] = useDisclosure(false);
  const [searchOpened, searchDrawer] = useDisclosure(false);
  const [readerSearch, setReaderSearch] = useState("");
  const [debouncedReaderSearch] = useDebouncedValue(readerSearch, 180);
  const [progress, setProgress] = useState(0);
  const [sourcePage, setSourcePage] = useState(1);
  const [jumpPage, setJumpPage] = useState<number | string>(1);
  const [jumpOpened, setJumpOpened] = useState(false);
  const [fullscreen, setFullscreen] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);
  const restoredKeyRef = useRef<string | null>(null);
  const pendingBookmarkRef = useRef<{ page: number; top: number } | null>(null);
  const { bookmarks, add: addBookmark, remove: removeBookmark } = useBookmarks(id);
  const metadataQuery = useQuery({
    queryKey: ["reader-metadata", id],
    queryFn: () => runtime ? getReaderMetadata(id) : Promise.resolve((() => { const demo = getDemoReader(id); return { download: demo.download, versions: demo.versions, assetCount: demo.assets.length, isEdited: demo.isEdited, activeEditRevision: demo.activeEditRevision }; })()),
    enabled: Number.isFinite(id),
  });
  const contentQuery = useQuery({
    queryKey: ["reader-content-page", id, version, sourcePage - 1],
    queryFn: () => runtime ? getReaderContentPage(id, version, sourcePage - 1) : Promise.resolve((() => { const demo = getDemoReader(id); return { page: 0, pageCount: 1, html: demo.html, plainText: demo.plainText, totalPlainTextChars: demo.plainText.length }; })()),
    enabled: Number.isFinite(id),
    placeholderData: (previous) => previous,
  });
  const readerSearchQuery = useQuery({
    queryKey: ["reader-content-search", id, version, debouncedReaderSearch],
    queryFn: () => runtime ? searchReaderContent(id, debouncedReaderSearch, version) : Promise.resolve([]),
    enabled: searchOpened && debouncedReaderSearch.trim().length > 0,
  });
  const explicitCollectionQuery = useQuery({
    // 取得関数はコレクション詳細と同じ getWorkCollection なので、キーも同じに
    // する。別名で持っていたため、束から作品を外した直後にリーダーへ戻ると、
    // 前後移動が外したはずの作品を指したままだった。
    queryKey: ["work-collection", requestedCollectionId],
    queryFn: () => getWorkCollection(requestedCollectionId!),
    enabled: runtime && Boolean(requestedCollectionId),
  });
  const memberCollectionsQuery = useQuery({
    queryKey: ["reader-member-collections", metadataQuery.data?.download.source, metadataQuery.data?.download.sourceId],
    queryFn: async () => {
      const summaries = await listCollectionsForWork(metadataQuery.data!.download.source, metadataQuery.data!.download.sourceId);
      const collections: WorkCollection[] = [];
      // Avoid one unbounded Promise.all while still loading every membership;
      // otherwise the 21st collection silently disappeared from reading context.
      for (let index = 0; index < summaries.length; index += 8) {
        collections.push(...await Promise.all(summaries.slice(index, index + 8).map((summary) => getWorkCollection(summary.id))));
      }
      return collections;
    },
    enabled: runtime && Boolean(metadataQuery.data),
    staleTime: 30_000,
  });
  const officialSeriesQuery = useQuery({
    // `id` belongs in the key: loadReaderSequence stops walking once this work
    // and its neighbour are known, so the cached list is an answer to "where is
    // this work", not "what is in this series". Without it, opening episode 1
    // of a long series cached the first 200 items and episode 250 read that
    // cache, found itself missing, and dropped the continuation row entirely.
    queryKey: ["reader-official-series", metadataQuery.data?.download.source, metadataQuery.data?.download.seriesId, id],
    queryFn: () => loadReaderSequence({ seriesSource: metadataQuery.data!.download.source, seriesKey: metadataQuery.data!.download.seriesId!, sortBy: "series_order", sortOrder: "asc", projection: "bulk" }, id),
    enabled: runtime && Boolean(metadataQuery.data?.download.seriesId),
    staleTime: 60_000,
  });
  const authorWorksQuery = useQuery({
    queryKey: ["reader-author-sequence", metadataQuery.data?.download.source, metadataQuery.data?.download.personId ?? metadataQuery.data?.download.authorId, id],
    queryFn: () => loadReaderSequence({ personSource: metadataQuery.data!.download.source, personKey: metadataQuery.data!.download.personId || metadataQuery.data!.download.authorId, sortBy: "source_created_at", sortOrder: "asc", projection: "bulk" }, id),
    enabled: runtime && Boolean(metadataQuery.data?.download.personId || metadataQuery.data?.download.authorId),
    staleTime: 60_000,
  });
  const readingContexts = useMemo<ReadingContextRow[]>(() => {
    const work = metadataQuery.data?.download;
    if (!work) return [];
    const rows: ReadingContextRow[] = [];
    const seenCollections = new Set<string>();
    // 読み始めた文脈を先に積む。以降は重複を弾くので、同じコレクションが
    // 二重に並ぶことなく、開始文脈が常に先頭に来る。
    const addCollection = (collection: WorkCollection | undefined) => {
      if (!collection || seenCollections.has(collection.id)) return;
      const missing = collection.memberCount - collection.availableCount;
      const items = collection.members.flatMap((member) => member.downloadId ? [{ id: member.downloadId, title: member.title }] : []);
      const adjacent = adjacentWorks(items, work.id);
      if (!adjacent.previous && !adjacent.next) return;
      seenCollections.add(collection.id);
      const sequential = collection.collectionKind === "ordered";
      const progress = sequential && adjacent.position
        ? `${adjacent.position} / ${adjacent.total}`
        : `${adjacent.total}作品`;
      rows.push({
        key: `collection:${collection.id}`,
        // 公式シリーズの行と同じく、常に何のまとまりかを名乗る。作品から直接
        // 開いた場合に名前だけが並ぶと、シリーズとの区別がつかなくなる。
        label: `コレクション「${collection.name}」`,
        // 順序なしは「並び」と呼ばない。表示上の並びがあるだけで、読む順ではない。
        detail: [progress, sequential ? "読む順あり" : "順序なし", missing > 0 ? `未保存${missing}作品は除外` : null]
          .filter(Boolean)
          .join(" · "),
        collectionId: collection.id,
        sequential,
        currentId: work.id,
        ...adjacent,
      });
    };
    addCollection(explicitCollectionQuery.data);
    for (const collection of memberCollectionsQuery.data ?? []) addCollection(collection);
    if (officialSeriesQuery.data) {
      const items = officialSeriesQuery.data.items.map((item) => ({ id: item.id, title: item.title }));
      const adjacent = adjacentWorks(items, work.id);
      const total = officialSeriesQuery.data.totalEstimate ?? adjacent.total;
      if (adjacent.previous || adjacent.next) rows.push({ key: `series:${work.source}:${work.seriesId}`, label: work.seriesTitle ? `公式シリーズ「${work.seriesTitle}」` : "公式シリーズ", detail: `${adjacent.position ?? "-"} / ${total} · 公式順`, sequential: true, currentId: work.id, ...adjacent });
    }
    if (authorWorksQuery.data) {
      const items = authorWorksQuery.data.items.map((item) => ({ id: item.id, title: item.title }));
      const adjacent = adjacentWorks(items, work.id);
      const total = authorWorksQuery.data.totalEstimate ?? adjacent.total;
      // 作者の投稿順は「物語の次」ではないため、続きとしては案内しない。
      if (adjacent.previous || adjacent.next) rows.push({ key: `author:${work.source}:${work.authorId}`, label: `${work.authorName}の作品`, detail: `${total}作品の公開順`, sequential: false, currentId: work.id, ...adjacent });
    }
    return rows;
  }, [authorWorksQuery.data, explicitCollectionQuery.data, memberCollectionsQuery.data, metadataQuery.data?.download, officialSeriesQuery.data, requestedCollectionId]);
  // Only while reading: an address you cannot follow is noise here, whereas the
  // detail screen deliberately shows the work as its author wrote it.
  const contentReady = !contentQuery.isPlaceholderData && contentQuery.data?.page === sourcePage - 1;
  const currentContent = contentReady ? contentQuery.data : null;
  const preparedHtml = useMemo(() => prepareDocumentHtml(currentContent?.html ?? "", getAssetUrl, { linkifyBareUrls: true }), [currentContent?.html]);
  const pageCount = contentQuery.data?.pageCount ?? 1;
  const hasSourcePages = pageCount > 1;
  const positionKey = `piep.reader-position.${id}.${version ?? "current"}`;
  const sequenceContextLimited = Boolean(officialSeriesQuery.data?.truncated || authorWorksQuery.data?.truncated);

  useEffect(() => { setVersion(normalizedRequestedVersion); }, [normalizedRequestedVersion]);
  useEffect(() => {
    setSourcePage(1);
  }, [id, version]);
  useEffect(() => {
    const openSearch = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f") {
        event.preventDefault();
        searchDrawer.open();
      }
    };
    window.addEventListener("keydown", openSearch);
    return () => window.removeEventListener("keydown", openSearch);
  }, [searchDrawer]);
  useEffect(() => {
    if (contentQuery.data?.page !== sourcePage - 1) return;
    queryClient.removeQueries({
      queryKey: ["reader-content-page", id, version],
      type: "inactive",
      predicate: (query) => query.queryKey[3] !== sourcePage - 1,
    });
  }, [contentQuery.data?.page, id, queryClient, sourcePage, version]);

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
    if (!viewport || !metadataQuery.data || !contentQuery.data) return;
    if (restoredKeyRef.current === positionKey) return;
    const saved = readReadingPosition(id, version);
    const savedPage = Math.min(Math.max(1, saved?.page ?? 1), Math.max(1, pageCount));
    const savedTop = Math.max(0, saved?.top ?? 0);
    if (sourcePage !== savedPage) {
      setSourcePage(savedPage);
      return;
    }
    // `placeholderData` still describes the page we just left. Applying its
    // offset would mark restoration complete before the requested page exists.
    if (!contentReady || contentQuery.data.page !== savedPage - 1) return;
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
  }, [contentReady, contentQuery.data, id, metadataQuery.data, pageCount, positionKey, sourcePage, version]);
  useEffect(() => {
    const pending = pendingBookmarkRef.current;
    const viewport = scrollRef.current;
    if (!pending || !viewport || !contentReady || contentQuery.data?.page !== pending.page - 1) return;
    let cancelled = false;
    let frame = 0;
    const deadline = performance.now() + 2_000;
    const apply = () => {
      if (cancelled) return;
      viewport.scrollTop = pending.top;
      if (Math.abs(viewport.scrollTop - pending.top) < 2 || performance.now() >= deadline) {
        pendingBookmarkRef.current = null;
        return;
      }
      frame = requestAnimationFrame(apply);
    };
    frame = requestAnimationFrame(apply);
    return () => { cancelled = true; cancelAnimationFrame(frame); };
  }, [contentQuery.data?.page, contentReady, sourcePage]);
  useEffect(() => {
    const viewport = scrollRef.current;
    if (!viewport || !contentReady) return;
    // 読んでいる位置の保存は、スクロールのたびに `getItem` + `parse` +
    // `stringify` + `setItem` を同期で走らせていた。指を滑らせている間は
    // 1秒に数十回になり、長い本ではその分だけ引っかかる。**進み具合の表示は
    // 毎回更新し、書き込みだけを間引く。** 離れるときに必ず書き切るので、
    // 最後に読んでいた場所は失われない。
    let pendingTop: number | null = null;
    let saveTimer: number | null = null;
    const flush = () => {
      saveTimer = null;
      if (pendingTop === null) return;
      writeReadingPosition(id, version, { page: sourcePage, top: pendingTop });
      pendingTop = null;
    };
    const update = (event?: Event) => {
      const total = viewport.scrollHeight - viewport.clientHeight;
      const next = total <= 0 ? 100 : Math.max(0, Math.min(100, viewport.scrollTop / total * 100));
      setProgress(hasSourcePages ? ((sourcePage - 1) + next / 100) / pageCount * 100 : next);
      // Only a genuine scroll writes the position. The priming call below runs
      // before the saved offset has been applied, so persisting there would
      // overwrite the stored place with 0 each time the reader opens.
      if (!event || restoredKeyRef.current !== positionKey) return;
      pendingTop = viewport.scrollTop;
      if (saveTimer === null) saveTimer = window.setTimeout(flush, 400);
    };
    viewport.addEventListener("scroll", update, { passive: true });
    update();
    return () => {
      viewport.removeEventListener("scroll", update);
      if (saveTimer !== null) window.clearTimeout(saveTimer);
      flush();
    };
  }, [contentReady, hasSourcePages, id, positionKey, sourcePage, pageCount, version]);

  if (metadataQuery.isLoading || contentQuery.isLoading) return <div className="page"><LoadingState label="リーダーを準備しています" /></div>;
  if (metadataQuery.error || contentQuery.error || !metadataQuery.data || !contentQuery.data) return <div className="page"><ErrorState error={metadataQuery.error ?? contentQuery.error ?? "作品が見つかりません"} retry={() => { metadataQuery.refetch(); contentQuery.refetch(); }} /></div>;
  const doc = metadataQuery.data;
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
      setSourcePage(Math.min(Math.max(1, jumpPage), pageCount));
      scrollRef.current?.scrollTo({ top: 0, behavior: "smooth" });
      return;
    }
    // Everything else - the series, the earlier part, the author's other
    // account - is resolved against the library first and only then handed to a
    // browser.
    await openContentLink(event);
  };
  const goToSourcePage = (page: number) => {
    const next = Math.min(Math.max(1, page), pageCount);
    if (next === sourcePage) return;
    setSourcePage(next);
    scrollRef.current?.scrollTo({ top: 0, behavior: "smooth" });
  };
  const addBookmarkHere = () => {
    if (!contentReady) return;
    const viewport = scrollRef.current;
    const top = Math.round(viewport?.scrollTop ?? 0);
    // Measured here rather than read from the progress state, which only
    // catches up on the next scroll event.
    const scrollable = (viewport?.scrollHeight ?? 0) - (viewport?.clientHeight ?? 0);
    const percent = scrollable > 0 ? Math.round(top / scrollable * 100) : 0;
    const label = `${hasSourcePages ? `${sourcePage}ページ · ` : ""}${percent}%`;
    addBookmark({ page: sourcePage, top, label });
    notifications.show({ color: "piep", message: `しおりを挟みました（${label}）` });
  };
  const jumpToBookmark = (bookmark: { page: number; top: number }) => {
    const page = Math.min(Math.max(1, bookmark.page), Math.max(1, pageCount));
    if (page === sourcePage && contentReady) {
      scrollRef.current?.scrollTo({ top: bookmark.top, behavior: "smooth" });
      return;
    }
    // Keep the target until the requested page has actually replaced the
    // placeholder. A single RAF runs long before an async page fetch completes.
    pendingBookmarkRef.current = { page, top: bookmark.top };
    setSourcePage(page);
  };

  return (
    <div className={`reader-page reader-theme--${settings.theme}`}>
      <header className="reader-toolbar">
        <Group h="100%" px="md" justify="space-between" wrap="nowrap">
          <Group wrap="nowrap" miw={0}>
            <Tooltip label="作品詳細へ戻る"><ActionIcon variant="subtle" color="gray" aria-label="作品詳細へ戻る" onClick={() => returnTo(`/works/${work.id}`)}><Icons.back size={IconSize.nav} /></ActionIcon></Tooltip>
            <Divider orientation="vertical" h={24} />
            {/* Title only: the provider and author already head the document
                itself, and repeating them here just crowded the bar. */}
            <Text size="sm" fw={700} className="line-clamp-1">{work.title}</Text>
          </Group>
          <Group gap="xs" wrap="nowrap">
            {doc.isEdited && <Badge color="gray" variant="light" visibleFrom="sm">ローカル編集</Badge>}
            <BookmarkControls
              bookmarks={bookmarks}
              addDisabled={!contentReady}
              onAdd={addBookmarkHere}
              onOpen={jumpToBookmark}
              onRemove={removeBookmark}
            />
            <Tooltip label="本文内を検索 (Ctrl+F)"><ActionIcon variant="subtle" color="gray" aria-label="本文内を検索" onClick={searchDrawer.open}><Icons.search size={IconSize.nav} /></ActionIcon></Tooltip>
            <Select
              size="xs"
              value={String(currentVersion)}
              onChange={(value) => setVersion(value ? Number(value) : null)}
              data={doc.versions.map((item) => ({ value: String(item.version), label: `v${item.version}` }))}
              w={84}
              aria-label="表示バージョン"
            />
            <Tooltip label="表示設定"><ActionIcon variant="subtle" color="gray" aria-label="リーダーの表示設定" onClick={settingsDrawer.open}><Icons.readerSettings size={IconSize.nav} /></ActionIcon></Tooltip>
            <Tooltip label={fullscreen ? "全画面を終了" : "全画面で読む"}><ActionIcon variant="subtle" color="gray" aria-label={fullscreen ? "全画面を終了" : "全画面表示"} onClick={() => fullscreen ? document.exitFullscreen() : document.documentElement.requestFullscreen()}>{fullscreen ? <Icons.fullscreenExit size={IconSize.nav} /> : <Icons.fullscreenEnter size={IconSize.nav} />}</ActionIcon></Tooltip>
            <Button variant="subtle" color="gray" size="xs" leftSection={<Icons.edit size={IconSize.menu} />} onClick={() => navigate(`/editor/${work.id}`)}>編集</Button>
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
            {!contentReady ? <LoadingState label="読書位置のページを読み込んでいます" /> : <>
              {sourcePage === 1 && <Box className="reader-title-page">
              {/* Source on the left, the two places this work lives on the
                  right - both were once called "保存元" - all on one line above
                  the title. */}
              <Group justify="space-between" align="center" gap="sm" wrap="nowrap" className="reader-title-page__head">
                <ProviderMark provider={work.source} />
                <Group gap={2} wrap="nowrap">
                  <Button variant="subtle" color="gray" size="compact-sm" rightSection={<Icons.externalLink size={IconSize.inline} />} onClick={openSource}>元ページをブラウザで</Button>
                  <Button variant="subtle" color="gray" size="compact-sm" rightSection={<Icons.openFolder size={IconSize.inline} />} disabled={!runtime} onClick={() => runtime && openLocalAsset(work.jsonPath)}>保存フォルダー</Button>
                </Group>
              </Group>
              <Title order={1} className="reader-title-page__title">{work.title}</Title>
              <Text c="dimmed" className="reader-title-page__author">{work.authorName}</Text>
              <Text size="sm" c="dimmed" className="reader-title-page__meta">{formatDate(work.sourceCreatedAt)} · {formatNumber(work.textLength)}字</Text>
              </Box>}
              {sourcePage === 1 && <Divider my="xl" />}
              <article className="reader-content" onClick={handleArticleClick} dangerouslySetInnerHTML={{ __html: preparedHtml }} />
              {sourcePage === pageCount && <Box className="reader-finish"><Icons.read size={IconSize.hero} /><Text fw={700}>読了</Text>{sequenceContextLimited && <Alert color="yellow" mt="md" maw={620}>作品数が非常に多いため、前後の作品を特定できない並びがあります。作品詳細からシリーズまたは作者一覧を開いてください。</Alert>}{readingContexts.length > 0 && <Stack gap="sm" w="100%" maw={620} mt="md">{readingContexts.map((context) => <ReadingContextCard key={context.key} context={context} onOpen={navigate} />)}</Stack>}<Button variant="light" mt="md" onClick={() => returnTo(`/works/${work.id}`)}>作品詳細へ戻る</Button></Box>}
            </>}
          </Box>
        </Box>
      </ScrollArea>
      {hasSourcePages && <Paper className="reader-page-controls" shadow="lg" withBorder radius="md" p={5} aria-label="pixiv原稿のページ操作">
        <Group gap={5} wrap="nowrap">
          <Pagination total={pageCount} value={sourcePage} onChange={goToSourcePage} siblings={1} boundaries={1} withEdges size="sm" radius="sm" />
          <Divider orientation="vertical" h={24} />
          <Popover opened={jumpOpened} onChange={setJumpOpened} position="top" withArrow shadow="md">
            <Popover.Target><Button variant="subtle" color="gray" size="compact-sm" onClick={() => setJumpOpened((value) => !value)} aria-label="ページ番号を指定">{sourcePage} / {pageCount}</Button></Popover.Target>
            <Popover.Dropdown>
              <Stack gap="xs" w={180}>
                <Text size="xs" fw={700}>移動先のページ</Text>
                <NumberInput min={1} max={pageCount} value={jumpPage} onChange={setJumpPage} clampBehavior="strict" aria-label="移動先ページ" />
                <Button size="xs" onClick={() => { goToSourcePage(Number(jumpPage) || 1); setJumpOpened(false); }}>移動</Button>
              </Stack>
            </Popover.Dropdown>
          </Popover>
        </Group>
      </Paper>}
      {progress > 8 && <Tooltip label="先頭へ戻る"><ActionIcon className="reader-to-top" size="lg" radius="xl" variant="filled" aria-label="本文の先頭へ戻る" onClick={() => scrollRef.current?.scrollTo({ top: 0, behavior: "smooth" })}><Icons.up size={IconSize.nav} /></ActionIcon></Tooltip>}

      <Drawer opened={settingsOpened} onClose={settingsDrawer.close} position="right" title="読書設定" size={360}>
        <Stack gap="xl">
          <Box><Group justify="space-between" mb="sm"><Text size="sm" fw={700}>文字サイズ</Text><Text size="xs" c="dimmed">{settings.fontSize}px</Text></Group><Slider aria-label="文字サイズ" min={14} max={28} step={1} value={settings.fontSize} onChange={(fontSize) => setSettings({ ...settings, fontSize })} marks={[{ value: 14, label: "小" }, { value: 21, label: "標準" }, { value: 28, label: "大" }]} /></Box>
          <Box><Group justify="space-between" mb="sm"><Text size="sm" fw={700}>行間</Text><Text size="xs" c="dimmed">{settings.lineHeight.toFixed(2)}</Text></Group><Slider aria-label="行間" min={1.4} max={2.4} step={0.05} value={settings.lineHeight} onChange={(lineHeight) => setSettings({ ...settings, lineHeight })} /></Box>
          <Box><Group justify="space-between" mb="sm"><Text size="sm" fw={700}>本文幅</Text><Text size="xs" c="dimmed">{settings.maxWidth}px</Text></Group><Slider aria-label="本文幅" min={520} max={900} step={20} value={settings.maxWidth} onChange={(maxWidth) => setSettings({ ...settings, maxWidth })} /></Box>
          <Box><Text size="sm" fw={700} mb="sm">書体</Text><SegmentedControl aria-label="書体" fullWidth value={settings.font} onChange={(font) => setSettings({ ...settings, font: font as ReaderSettings["font"] })} data={[{ value: "serif", label: "明朝" }, { value: "sans", label: "ゴシック" }]} /></Box>
          <Box><Text size="sm" fw={700} mb="sm">背景</Text><Radio.Group value={settings.theme} onChange={(theme) => setSettings({ ...settings, theme: theme as ReaderSettings["theme"] })}><Stack gap="xs"><Radio value="paper" label="紙の色" /><Radio value="white" label="白" /><Radio value="night" label="ナイト" /></Stack></Radio.Group></Box>
          <Button variant="default" leftSection={<Icons.typography size={IconSize.menu} />} onClick={() => setSettings(defaults)}>初期設定に戻す</Button>
        </Stack>
      </Drawer>
      <Drawer opened={searchOpened} onClose={searchDrawer.close} position="left" title="本文内を検索" size={380}>
        <Stack gap="md">
          <TextInput value={readerSearch} onChange={(event) => setReaderSearch(event.currentTarget.value)} leftSection={<Icons.search size={IconSize.action} />} placeholder="検索語を入力" autoFocus aria-label="本文内の検索語" />
          {!readerSearch.trim() ? <Text size="sm" c="dimmed">全ページを対象に端末内で検索します。</Text> : readerSearchQuery.isLoading ? <LoadingState label="本文を検索しています" /> : readerSearchQuery.error ? <ErrorState error={readerSearchQuery.error} retry={() => readerSearchQuery.refetch()} /> : readerSearchQuery.data?.length ? <Stack gap="xs">{readerSearchQuery.data.map((hit) => <Button key={`${hit.page}-${hit.snippet}`} variant="default" h="auto" py="sm" px="md" justify="flex-start" onClick={() => { goToSourcePage(hit.page); searchDrawer.close(); }}><Stack gap={3} align="flex-start"><Text size="xs" fw={700}>{hit.page}ページ · {hit.count}件</Text><Text size="xs" c="dimmed" ta="left" lineClamp={3}>{hit.snippet}</Text></Stack></Button>)}</Stack> : <Alert color="gray">一致する箇所はありません。</Alert>}
        </Stack>
      </Drawer>
    </div>
  );
}
