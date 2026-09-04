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
  Modal,
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
import { currentAnchor, flowLength, flowOffset, scrollElementIntoView, scrollToAnchor, scrollToStart } from "@/features/reader/readerAnchor";
import { HIT_ATTRIBUTE, highlightMatches } from "@/features/reader/readerSearch";
import { readReadingPosition, writeReadingPosition } from "@/features/library/readingShelf";
import { useAppNavigate, useAppSearchParams, useReturnTo, useRouteParams } from "@/app/router";
import { ErrorState, LoadingState } from "@/components/AsyncState";
import { ProviderMark, sourceUrl } from "@/lib/providers";
import { formatDate, formatNumber } from "@/lib/format";
import { prepareDocumentHtml } from "@/lib/content";
import { useContentLinkNavigation } from "@/lib/contentLinks";
import { getWorkCollection, listCollectionsForWork } from "@/services/collectionApi";
import { getAssetUrl, getReaderContentPage, getReaderMetadata, getReaderOutline, isTauriRuntime, openLocalAsset, searchDownloadsV2, searchReaderContent } from "@/services/dbApi";
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
                <Group key={bookmark.id} gap={4} wrap="nowrap" align="flex-start">
                  <Button variant="subtle" color="gray" size="compact-xs" h="auto" py={5} justify="flex-start" flex={1} miw={0} onClick={() => { setOpened(false); onOpen(bookmark); }}>
                    {/* 番号と割合だけでは、どれがどれだか分からなかった。 */}
                    <Stack gap={1} align="flex-start" miw={0} w="100%">
                      <Text size="xs" fw={700}>{bookmark.label}</Text>
                      {bookmark.excerpt && <Text size="xs" c="dimmed" ta="left" lineClamp={2}>{bookmark.excerpt}</Text>}
                    </Stack>
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

/**
 * 画面の上端に見えている本文の書き出し。
 *
 * しおりの見出しが「3ページ · 42%」だけでは、並んだときにどれがどれだか
 * 分からない。挟んだところに何が書いてあったかを一緒に憶えておく。
 */
function excerptAt(viewport: HTMLElement | null, article: HTMLElement | null): string {
  if (!viewport || !article || typeof document === "undefined") return "";
  const viewportTop = viewport.getBoundingClientRect().top;
  const walker = document.createTreeWalker(article, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const text = node as Text;
    if (!text.data.trim()) continue;
    const range = document.createRange();
    range.selectNodeContents(text);
    if (range.getBoundingClientRect().bottom < viewportTop) continue;
    return text.data.trim().slice(0, 40);
  }
  return "";
}

interface ReaderSettings {
  fontSize: number;
  lineHeight: number;
  maxWidth: number;
  font: "serif" | "sans";
  theme: "paper" | "white" | "night";
  /**
   * 縦書き。EPUB には最初からある設定なのに、読む画面にだけ無かった。
   * 日本語の小説を読むための道具として、これは欠けたままにできない。
   */
  writing: "horizontal" | "vertical";
}

const defaults: ReaderSettings = { fontSize: 18, lineHeight: 1.72, maxWidth: 680, font: "serif", theme: "white", writing: "horizontal" };

/** 版の選択で「編集版」を表す値。数字の版番号と混ざらない印。 */
const EDITED_VERSION = "edit";

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
  const [outlineOpened, outlineDrawer] = useDisclosure(false);
  const [readerSearch, setReaderSearch] = useState("");
  const [debouncedReaderSearch] = useDebouncedValue(readerSearch, 180);
  // いま本文に印を入れている語。検索の窓を閉じても印は残す ―― 閉じた途端に
  // 見つけた場所が分からなくなるのでは、探した意味がない。
  const [markedTerm, setMarkedTerm] = useState("");
  const [hitIndex, setHitIndex] = useState(0);
  const [zoomImage, setZoomImage] = useState<{ src: string; alt: string } | null>(null);
  const [progress, setProgress] = useState(0);
  const [sourcePage, setSourcePage] = useState(1);
  const [jumpPage, setJumpPage] = useState<number | string>(1);
  const [jumpOpened, setJumpOpened] = useState(false);
  const [fullscreen, setFullscreen] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);
  const articleRef = useRef<HTMLElement>(null);
  const restoredKeyRef = useRef<string | null>(null);
  const pendingBookmarkRef = useRef<{ page: number; top: number; anchor?: number } | null>(null);
  // ページを跨いで検索結果へ飛ぶとき、そのページが届くまで持っておく。
  const pendingHitRef = useRef<number | null>(null);
  // ページを送ったあと、届いた本文の先頭へ寄せるための印。
  const pendingPageStartRef = useRef(false);
  const { bookmarks, add: addBookmark, remove: removeBookmark } = useBookmarks(id);
  const metadataQuery = useQuery({
    queryKey: ["reader-metadata", id],
    queryFn: () => runtime ? getReaderMetadata(id) : Promise.resolve((() => { const demo = getDemoReader(id); return { download: demo.download, versions: demo.versions, assetCount: demo.assets.length, isEdited: demo.isEdited, activeEditRevision: demo.activeEditRevision }; })()),
    enabled: Number.isFinite(id),
  });
  const contentQuery = useQuery({
    queryKey: ["reader-content-page", id, version, sourcePage - 1],
    queryFn: () => runtime ? getReaderContentPage(id, version, sourcePage - 1) : Promise.resolve((() => { const demo = getDemoReader(id); return { page: 0, pageCount: 1, html: demo.html, plainText: demo.plainText, totalPlainTextChars: demo.plainText.length, sourcePageStarts: [0] }; })()),
    enabled: Number.isFinite(id),
    placeholderData: (previous) => previous,
  });
  const outlineQuery = useQuery({
    queryKey: ["reader-outline", id, version],
    queryFn: () => runtime ? getReaderOutline(id, version) : Promise.resolve([]),
    enabled: Number.isFinite(id),
    staleTime: 60_000,
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
  }, [authorWorksQuery.data, explicitCollectionQuery.data, memberCollectionsQuery.data, metadataQuery.data?.download, officialSeriesQuery.data]);
  // Only while reading: an address you cannot follow is noise here, whereas the
  // detail screen deliberately shows the work as its author wrote it.
  const contentReady = !contentQuery.isPlaceholderData && contentQuery.data?.page === sourcePage - 1;
  const currentContent = contentReady ? contentQuery.data : null;
  const cleanHtml = useMemo(() => prepareDocumentHtml(currentContent?.html ?? "", getAssetUrl, { linkifyBareUrls: true }), [currentContent?.html]);
  // 探した語には本文の中で印を入れる。ページ番号を返すだけだったころは、
  // 押しても本文の頭へ動くだけで、どこに在ったのかは自分で探し直しだった。
  const marked = useMemo(() => highlightMatches(cleanHtml, markedTerm), [cleanHtml, markedTerm]);
  const preparedHtml = marked.html;
  const hitCount = marked.count;
  const pageCount = contentQuery.data?.pageCount ?? 1;
  const hasSourcePages = pageCount > 1;
  const vertical = settings.writing === "vertical";
  const positionKey = `piep.reader-position.${id}.${version ?? "current"}`;
  const sequenceContextLimited = Boolean(officialSeriesQuery.data?.truncated || authorWorksQuery.data?.truncated);

  useEffect(() => { setVersion(normalizedRequestedVersion); }, [normalizedRequestedVersion]);
  useEffect(() => {
    setSourcePage(1);
  }, [id, version]);
  // 手を鍵盤から離さずに読めるようにする。以前は Ctrl+F だけで、ページを
  // めくるには毎回下の小さな数字を狙って押す必要があった。
  const goToPageRef = useRef<(page: number) => void>(() => undefined);
  const sourcePageRef = useRef(sourcePage);
  sourcePageRef.current = sourcePage;
  useEffect(() => {
    const isTyping = (target: EventTarget | null) => {
      const element = target as HTMLElement | null;
      return Boolean(element) && (element!.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(element!.tagName));
    };
    const onKey = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f") {
        event.preventDefault();
        searchDrawer.open();
        return;
      }
      if (event.key === "Escape") {
        // 印だけを消す。窓が開いていればそちらは Mantine が閉じる。
        setMarkedTerm("");
        return;
      }
      if (isTyping(event.target) || event.ctrlKey || event.metaKey || event.altKey) return;
      const turn = (delta: number) => {
        event.preventDefault();
        goToPageRef.current(sourcePageRef.current + delta);
      };
      // 縦書きは右から左へ進む。矢印の向きも本の向きに合わせる。
      const forward = vertical ? "ArrowLeft" : "ArrowRight";
      const backward = vertical ? "ArrowRight" : "ArrowLeft";
      if (event.key === forward || event.key === "PageDown") return turn(1);
      if (event.key === backward || event.key === "PageUp") return turn(-1);
      if (event.key === "Home") { event.preventDefault(); return goToPageRef.current(1); }
      if (event.key === "End") { event.preventDefault(); return goToPageRef.current(pageCount); }
      if (event.key === "t") { event.preventDefault(); return outlineDrawer.open(); }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [outlineDrawer, pageCount, searchDrawer, vertical]);
  // 隣のページを先に取っておく。前後 1 ページだけ残して、あとは捨てる。
  //
  // 以前は現在ページ以外を**すべて**捨てていたので、前のページへ戻るたびに
  // IPC を往復して待たされた。本を読むのに、めくるたび待つ道理はない。
  useEffect(() => {
    if (!runtime || contentQuery.data?.page !== sourcePage - 1) return;
    const neighbours = [sourcePage - 2, sourcePage].filter((page) => page >= 0 && page < pageCount);
    for (const page of neighbours) {
      void queryClient.prefetchQuery({
        queryKey: ["reader-content-page", id, version, page],
        queryFn: () => getReaderContentPage(id, version, page),
        staleTime: 60_000,
      });
    }
    const keep = new Set([sourcePage - 2, sourcePage - 1, sourcePage]);
    queryClient.removeQueries({
      queryKey: ["reader-content-page", id, version],
      type: "inactive",
      predicate: (query) => !keep.has(query.queryKey[3] as number),
    });
  }, [contentQuery.data?.page, id, pageCount, queryClient, runtime, sourcePage, version]);

  useEffect(() => {
    const update = () => setFullscreen(Boolean(document.fullscreenElement));
    document.addEventListener("fullscreenchange", update);
    update();
    return () => document.removeEventListener("fullscreenchange", update);
  }, []);
  useEffect(() => { setJumpPage(sourcePage); }, [sourcePage]);

  // 届いたページの先頭から読み始める。縦書きは右端が先頭なので、寸法が
  // 決まってから寄せる。
  useEffect(() => {
    if (!pendingPageStartRef.current || !contentReady) return;
    pendingPageStartRef.current = false;
    const viewport = scrollRef.current;
    if (viewport) scrollToStart(viewport, vertical, false);
  }, [contentReady, sourcePage, vertical]);

  // 縦書きに切り替えた直後も、また本の先頭が右端になる。読んでいた行へ
  // 戻し直す。
  useEffect(() => {
    restoredKeyRef.current = null;
  }, [vertical]);

  // 探した語のところへ送る。縦書きでも効くように、向きの取り決めに依らない
  // `scrollIntoView` を使う。
  useEffect(() => {
    const article = articleRef.current;
    if (!article || !contentReady) return;
    article.querySelectorAll(".reader-hit--current").forEach((node) => node.classList.remove("reader-hit--current"));
    if (!markedTerm || hitCount === 0) return;
    const pending = pendingHitRef.current;
    const wanted = pending === null ? hitIndex : pending === -1 ? hitCount - 1 : pending;
    pendingHitRef.current = null;
    const index = Math.min(Math.max(0, wanted), hitCount - 1);
    if (index !== hitIndex) { setHitIndex(index); return; }
    const target = article.querySelector<HTMLElement>(`[${HIT_ATTRIBUTE}="${index}"]`);
    if (!target) return;
    target.classList.add("reader-hit--current");
    scrollElementIntoView(target, { block: "center", inline: "center", behavior: "smooth" });
  }, [contentReady, hitCount, hitIndex, markedTerm, preparedHtml]);

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
    // 縦書きで、桁の組み上がりを待つための控え。
    let lastLength = -1;
    const deadline = performance.now() + 2000;
    const attempt = () => {
      if (cancelled) return;
      // 行の目印があればそれで戻す。文字の大きさを変えていても同じ行へ着く。
      // px しか憶えていなかったころは、読書設定を触った瞬間に迷子になった。
      const article = articleRef.current;
      if (saved?.anchor !== undefined && article && scrollToAnchor(viewport, article, saved.anchor)) {
        restoredKeyRef.current = positionKey;
        return;
      }
      if (vertical) {
        // 縦書きは右端が本の先頭。px で憶えた位置は縦のものなので当てにせず、
        // 行の目印が無いときは先頭から読み直してもらう。
        //
        // 桁がまだ組み上がっていない最初のフレームでは横幅が窓と同じで、
        // 「先頭」も 0 になってしまう。**幅が二フレーム続けて変わらなく
        // なるまで**待つ ―― 一度で諦めていたころは、縦書きで開くと本の
        // 最後のページが出ていた。
        const length = flowLength(viewport, true);
        scrollToStart(viewport, true, false);
        if ((length === lastLength && flowOffset(viewport, true) < 2) || performance.now() >= deadline) {
          restoredKeyRef.current = positionKey;
          return;
        }
        lastLength = length;
        frame = requestAnimationFrame(attempt);
        return;
      }
      viewport.scrollTop = savedTop;
      if (Math.abs(viewport.scrollTop - savedTop) < 2 || performance.now() >= deadline) {
        restoredKeyRef.current = positionKey;
        return;
      }
      frame = requestAnimationFrame(attempt);
    };
    // 最初の一回はその場で。窓が隠れている間は requestAnimationFrame が
    // 動かないので、フレーム待ちだけに任せると位置がいつまでも戻らない。
    attempt();
    return () => { cancelled = true; cancelAnimationFrame(frame); };
  }, [contentReady, contentQuery.data, id, metadataQuery.data, pageCount, positionKey, sourcePage, version, vertical]);
  useEffect(() => {
    const pending = pendingBookmarkRef.current;
    const viewport = scrollRef.current;
    if (!pending || !viewport || !contentReady || contentQuery.data?.page !== pending.page - 1) return;
    let cancelled = false;
    let frame = 0;
    const deadline = performance.now() + 2_000;
    const apply = () => {
      if (cancelled) return;
      const article = articleRef.current;
      if (pending.anchor !== undefined && article && scrollToAnchor(viewport, article, pending.anchor)) {
        pendingBookmarkRef.current = null;
        return;
      }
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
      // 行の目印は書き込むときにだけ数える。スクロールのたびに数えていては、
      // 行の多い本で指を滑らせている間じゅう本文を走査することになる。
      const article = articleRef.current;
      const anchor = article ? currentAnchor(viewport, article) : null;
      writeReadingPosition(id, version, { page: sourcePage, top: pendingTop, ...(anchor === null ? {} : { anchor }) });
      pendingTop = null;
    };
    const update = (event?: Event) => {
      // 縦書きは横に流れる。始まりがどちら端かは端末の取り決め次第なので、
      // 「どれだけ動いたか」の絶対値で測る。
      const total = flowLength(viewport, vertical);
      const next = total <= 0 ? 100 : Math.max(0, Math.min(100, flowOffset(viewport, vertical) / total * 100));
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
  }, [contentReady, hasSourcePages, id, positionKey, sourcePage, pageCount, version, vertical]);

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
  /** pixiv の原稿ページ番号を、実際に運ばれてくるページ番号へ直す。
   *
   *  長い原稿ページは転送のために割られる。対応表を見ずに番号をそのまま
   *  使っていたので、割られた作品では `[jump:5]` が 5 ページ目ではない
   *  どこかへ飛んでいた。 */
  const transportPageForSourcePage = (page: number) => {
    const starts = contentQuery.data?.sourcePageStarts ?? [];
    const start = starts[Math.max(0, page - 1)];
    return start === undefined ? page : start + 1;
  };
  const handleArticleClick = async (event: React.MouseEvent<HTMLElement>) => {
    const image = (event.target as HTMLElement).closest<HTMLImageElement>("img.novel-image, .reader-content img");
    if (image?.currentSrc || image?.src) {
      // 挿絵は押しても何も起こらなかった。小さいまま眺めるしかない絵を
      // 本文に置いておく理由はない。
      event.preventDefault();
      setZoomImage({ src: image.currentSrc || image.src, alt: image.alt || "本文画像" });
      return;
    }
    const anchor = (event.target as HTMLElement).closest<HTMLAnchorElement>("a[href]");
    if (!anchor) return;
    const jumpPage = Number(anchor.dataset.page);
    if (anchor.classList.contains("jump-link") && Number.isFinite(jumpPage)) {
      event.preventDefault();
      goToSourcePage(transportPageForSourcePage(jumpPage));
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
    // 送り先が決まっていないときだけ、新しいページの先頭から読み始める。
    // ここで直に動かしていたころは、まだ届いていないページの寸法に対して
    // 動かしていたので、縦書きでは着く場所が定まらなかった。
    if (pendingBookmarkRef.current === null && pendingHitRef.current === null) pendingPageStartRef.current = true;
    setSourcePage(next);
  };
  goToPageRef.current = goToSourcePage;
  const addBookmarkHere = () => {
    if (!contentReady) return;
    const viewport = scrollRef.current;
    const article = articleRef.current;
    const top = Math.round(viewport?.scrollTop ?? 0);
    // Measured here rather than read from the progress state, which only
    // catches up on the next scroll event.
    const scrollable = (viewport?.scrollHeight ?? 0) - (viewport?.clientHeight ?? 0);
    const percent = scrollable > 0 ? Math.round(top / scrollable * 100) : 0;
    const label = `${hasSourcePages ? `${sourcePage}ページ · ` : ""}${percent}%`;
    const anchor = viewport && article ? currentAnchor(viewport, article) : null;
    // 番号と割合だけでは、どのしおりがどれだか分からない。挟んだところの
    // 書き出しを一緒に憶えておく。
    const excerpt = excerptAt(viewport, article);
    addBookmark({ page: sourcePage, top, label, ...(anchor === null ? {} : { anchor }), ...(excerpt ? { excerpt } : {}) });
    notifications.show({ color: "piep", message: `しおりを挟みました（${label}）` });
  };
  const jumpToBookmark = (bookmark: Bookmark) => {
    const page = Math.min(Math.max(1, bookmark.page), Math.max(1, pageCount));
    if (page === sourcePage && contentReady) {
      const viewport = scrollRef.current;
      const article = articleRef.current;
      if (bookmark.anchor !== undefined && viewport && article && scrollToAnchor(viewport, article, bookmark.anchor)) return;
      viewport?.scrollTo({ top: bookmark.top, behavior: "smooth" });
      return;
    }
    // Keep the target until the requested page has actually replaced the
    // placeholder. A single RAF runs long before an async page fetch completes.
    pendingBookmarkRef.current = { page, top: bookmark.top, anchor: bookmark.anchor };
    setSourcePage(page);
  };
  /** 検索結果や目次から、そのページの目当ての場所へ送る。 */
  const goToHit = (page: number, index: number) => {
    setMarkedTerm(readerSearch.trim());
    if (page === sourcePage) {
      setHitIndex(index);
      pendingHitRef.current = index;
    } else {
      pendingHitRef.current = index;
      setHitIndex(index);
      goToSourcePage(page);
    }
  };
  const stepHit = (delta: number) => {
    if (hitCount === 0) return;
    const next = hitIndex + delta;
    if (next >= 0 && next < hitCount) { setHitIndex(next); return; }
    // ページの端まで来たら、隣のページの端から続ける。
    const wrapPage = next < 0 ? sourcePage - 1 : sourcePage + 1;
    if (wrapPage < 1 || wrapPage > pageCount) return;
    pendingHitRef.current = next < 0 ? -1 : 0;
    goToSourcePage(wrapPage);
  };

  return (
    <div className={`reader-page reader-theme--${settings.theme}${vertical ? " reader-page--vertical" : ""}`}>
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
            {outlineQuery.data && outlineQuery.data.length > 0 && <Tooltip label="目次 (T)"><ActionIcon variant="subtle" color="gray" aria-label={`目次（${outlineQuery.data.length}章）`} onClick={outlineDrawer.open}><Icons.epubStructure size={IconSize.nav} /></ActionIcon></Tooltip>}
            <Tooltip label="本文内を検索 (Ctrl+F)"><ActionIcon variant="subtle" color="gray" aria-label="本文内を検索" onClick={searchDrawer.open}><Icons.search size={IconSize.nav} /></ActionIcon></Tooltip>
            {/* 編集版はそれと名乗る。以前は反映中でも土台の版番号を出していた
                ので、同じ番号を選び直すと黙って取り込んだ本文へ切り替わり、
                何が起きたのか読み手には分からなかった。 */}
            <Select
              size="xs"
              value={doc.isEdited && version === null ? EDITED_VERSION : String(currentVersion)}
              onChange={(value) => setVersion(!value || value === EDITED_VERSION ? null : Number(value))}
              data={[
                ...(doc.isEdited ? [{ value: EDITED_VERSION, label: "編集版" }] : []),
                ...doc.versions.map((item) => ({ value: String(item.version), label: `v${item.version}` })),
              ]}
              w={doc.isEdited ? 108 : 84}
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
              <article ref={articleRef} className="reader-content" onClick={handleArticleClick} dangerouslySetInnerHTML={{ __html: preparedHtml }} />
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
      {progress > 8 && <Tooltip label="先頭へ戻る"><ActionIcon className="reader-to-top" size="lg" radius="xl" variant="filled" aria-label="本文の先頭へ戻る" onClick={() => scrollRef.current && scrollToStart(scrollRef.current, vertical)}><Icons.up size={IconSize.nav} /></ActionIcon></Tooltip>}

      {/* 探した語の間を行き来する帯。ページ番号だけ返して終わりだったころは、
          結果を押しても本文の頭へ動くだけで、どこに在ったのかは自分で
          探し直しだった。ページが 1 つしかない作品では何も起きなかった。 */}
      {markedTerm && <Paper className="reader-hit-bar" shadow="lg" withBorder radius="md" p={5} aria-label="本文内検索の移動">
        <Group gap={6} wrap="nowrap">
          <Text size="xs" fw={700} px={4} className="line-clamp-1" maw={160}>{markedTerm}</Text>
          <Text size="xs" c="dimmed">{hitCount ? `${hitIndex + 1} / ${hitCount}` : "このページには無し"}</Text>
          <ActionIcon variant="subtle" color="gray" size="sm" aria-label="前の一致へ" disabled={!hitCount} onClick={() => stepHit(-1)}><Icons.up size={IconSize.menu} /></ActionIcon>
          <ActionIcon variant="subtle" color="gray" size="sm" aria-label="次の一致へ" disabled={!hitCount} onClick={() => stepHit(1)}><Icons.down size={IconSize.menu} /></ActionIcon>
          <Divider orientation="vertical" h={20} />
          <ActionIcon variant="subtle" color="gray" size="sm" aria-label="印を消す" onClick={() => setMarkedTerm("")}><Icons.cancel size={IconSize.menu} /></ActionIcon>
        </Group>
      </Paper>}

      <Modal opened={Boolean(zoomImage)} onClose={() => setZoomImage(null)} size="auto" centered withCloseButton={false} padding={0} className="reader-zoom">
        {zoomImage && <img src={zoomImage.src} alt={zoomImage.alt} className="reader-zoom__image" onClick={() => setZoomImage(null)} />}
      </Modal>

      <Drawer opened={outlineOpened} onClose={outlineDrawer.close} position="left" title="目次" size={340}>
        {outlineQuery.isLoading ? <LoadingState label="目次を読み込んでいます" /> : !outlineQuery.data?.length ? <Alert color="gray">この作品には見出しがありません。</Alert> : <Stack gap={2}>
          {outlineQuery.data.map((entry) => (
            <Button key={`${entry.page}-${entry.index}-${entry.title}`} variant={entry.page === sourcePage ? "light" : "subtle"} color="gray" size="compact-sm" h="auto" py={8} justify="flex-start" onClick={() => { goToSourcePage(entry.page); outlineDrawer.close(); }}>
              <Stack gap={2} align="flex-start"><Text size="sm" ta="left" lineClamp={2}>{entry.title}</Text>{hasSourcePages && <Text size="xs" c="dimmed">{entry.page}ページ</Text>}</Stack>
            </Button>
          ))}
        </Stack>}
      </Drawer>

      <Drawer opened={settingsOpened} onClose={settingsDrawer.close} position="right" title="読書設定" size={360}>
        <Stack gap="xl">
          <Box><Group justify="space-between" mb="sm"><Text size="sm" fw={700}>文字サイズ</Text><Text size="xs" c="dimmed">{settings.fontSize}px</Text></Group><Slider aria-label="文字サイズ" min={14} max={28} step={1} value={settings.fontSize} onChange={(fontSize) => setSettings({ ...settings, fontSize })} marks={[{ value: 14, label: "小" }, { value: 21, label: "標準" }, { value: 28, label: "大" }]} /></Box>
          <Box><Group justify="space-between" mb="sm"><Text size="sm" fw={700}>行間</Text><Text size="xs" c="dimmed">{settings.lineHeight.toFixed(2)}</Text></Group><Slider aria-label="行間" min={1.4} max={2.4} step={0.05} value={settings.lineHeight} onChange={(lineHeight) => setSettings({ ...settings, lineHeight })} /></Box>
          <Box><Group justify="space-between" mb="sm"><Text size="sm" fw={700}>本文幅</Text><Text size="xs" c="dimmed">{settings.maxWidth}px</Text></Group><Slider aria-label="本文幅" min={520} max={900} step={20} value={settings.maxWidth} onChange={(maxWidth) => setSettings({ ...settings, maxWidth })} /></Box>
          <Box><Text size="sm" fw={700} mb="sm">書体</Text><SegmentedControl aria-label="書体" fullWidth value={settings.font} onChange={(font) => setSettings({ ...settings, font: font as ReaderSettings["font"] })} data={[{ value: "serif", label: "明朝" }, { value: "sans", label: "ゴシック" }]} /></Box>
          {/* EPUB には最初からあった設定が、読む画面にだけ無かった。 */}
          <Box><Text size="sm" fw={700} mb="sm">組み方</Text><SegmentedControl aria-label="組み方" fullWidth value={settings.writing === "vertical" ? "vertical" : "horizontal"} onChange={(writing) => setSettings({ ...settings, writing: writing as ReaderSettings["writing"] })} data={[{ value: "horizontal", label: "横書き" }, { value: "vertical", label: "縦書き" }]} /><Text size="xs" c="dimmed" mt={6}>縦書きは右から左へ進みます。矢印キーの向きも入れ替わります。</Text></Box>
          <Box><Text size="sm" fw={700} mb="sm">背景</Text><Radio.Group value={settings.theme} onChange={(theme) => setSettings({ ...settings, theme: theme as ReaderSettings["theme"] })}><Stack gap="xs"><Radio value="paper" label="紙の色" /><Radio value="white" label="白" /><Radio value="night" label="ナイト" /></Stack></Radio.Group></Box>
          <Button variant="default" leftSection={<Icons.typography size={IconSize.menu} />} onClick={() => setSettings(defaults)}>初期設定に戻す</Button>
        </Stack>
      </Drawer>
      <Drawer opened={searchOpened} onClose={searchDrawer.close} position="left" title="本文内を検索" size={380}>
        <Stack gap="md">
          <TextInput value={readerSearch} onChange={(event) => setReaderSearch(event.currentTarget.value)} leftSection={<Icons.search size={IconSize.action} />} placeholder="検索語を入力" autoFocus aria-label="本文内の検索語" />
          {!readerSearch.trim() ? <Text size="sm" c="dimmed">全ページを対象に端末内で検索します。かな・カナ・全角半角の違いは吸収します。</Text> : readerSearchQuery.isLoading ? <LoadingState label="本文を検索しています" /> : readerSearchQuery.error ? <ErrorState error={readerSearchQuery.error} retry={() => readerSearchQuery.refetch()} /> : readerSearchQuery.data?.length ? <Stack gap="xs">{readerSearchQuery.data.map((hit) => <Button key={`${hit.page}-${hit.snippet}`} variant="default" h="auto" py="sm" px="md" justify="flex-start" onClick={() => { goToHit(hit.page, 0); searchDrawer.close(); }}><Stack gap={3} align="flex-start"><Text size="xs" fw={700}>{hasSourcePages ? `${hit.page}ページ · ` : ""}{hit.count}件</Text><Text size="xs" c="dimmed" ta="left" lineClamp={3}>{hit.snippet}</Text></Stack></Button>)}</Stack> : <Alert color="gray">一致する箇所はありません。</Alert>}
        </Stack>
      </Drawer>
    </div>
  );
}
