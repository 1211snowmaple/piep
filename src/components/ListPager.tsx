import { useCallback, useEffect, useRef, useState } from "react";
import { Button, Group, SegmentedControl, Stack, Text, Tooltip } from "@mantine/core";
import { useLocalStorage } from "@mantine/hooks";
import { Icons, IconSize, type LucideIcon } from "@/lib/icons";
import { formatNumber } from "@/lib/format";

/**
 * How the reader wants long listings to advance.
 *
 * `pages` is only offered where it can be honoured. Relevance results are
 * walked with a score cursor, which has no nth page, so a search ranked by
 * relevance stays on scrolling however this is set - and says so rather than
 * showing page numbers that would land somewhere else.
 *
 * Scrolling still keeps its button, so there is no separate "button only" mode
 * to choose: it was the same screen with one behaviour removed.
 */
export type PagingMode = "auto" | "pages";

/**
 * 読み込み方を別々に覚える単位。
 *
 * 画面ではなく**一覧**ごとに分ける。ライブラリは一つの画面だが、作品・作者・
 * コレクションは中身も件数も違うので、同じ好みで縛る理由が無い。
 * 増やしたときは `PAGING_SCOPES` にも足す - 設定画面はそこから作る。
 */
export type PagingScope =
  | "library-works"
  | "library-entities"
  | "library-collections"
  | "entity"
  | "collection-members";

/** 個別の設定。`inherit` は「全体に従う」。 */
export type PagingPreference = PagingMode | "inherit";

/** 設定画面が個別に並べる一覧。名前は画面上の呼び方に合わせる。 */
export const PAGING_SCOPES: { value: PagingScope; label: string }[] = [
  { value: "library-works", label: "ライブラリ · 作品" },
  { value: "library-entities", label: "ライブラリ · 作者とシリーズ" },
  { value: "library-collections", label: "ライブラリ · コレクション" },
  { value: "entity", label: "作者・シリーズのページ" },
  { value: "collection-members", label: "コレクションの中身" },
];

const STORAGE_KEY = "piep.paging-mode";
const PAGE_SIZE_KEY = "piep.page-size";

/** The sizes the settings screen offers. Any size in range is honoured. */
export const PAGE_SIZE_OPTIONS = [20, 40, 60, 100] as const;
export const DEFAULT_PAGE_SIZE = 20;

/**
 * Read on the first render, not a tick later.
 *
 * The default is to report the fallback until an effect has run, which is meant
 * for server rendering and is actively harmful here: for one render the app
 * believes the reader is on scrolling mode when they are not. That falsehood
 * was enough to delete `?page=2` from the address on every mount - so returning
 * from a work opened on page two landed on page one - and to make a freshly
 * opened author screen scroll as though the page had just been turned.
 */
const READ_ON_FIRST_RENDER = { getInitialValueInEffect: false } as const;

/** What the backend will honour, whatever the settings screen chooses to offer. */
const MIN_PAGE_SIZE = 5;
const MAX_PAGE_SIZE = 200;

/**
 * Numbered pages are an optional convenience, not the primary traversal path.
 *
 * SQLite still has to walk every skipped row for an OFFSET query. A pasted
 * `?page=50000` therefore used to make opening a library link substantially
 * more expensive than scrolling to the same area with its keyset cursor. Keep
 * direct jumps inside a predictable budget; long listings continue through
 * the default automatic/keyset mode.
 */
export const MAX_NUMBERED_OFFSET = 5_000;

export function maxDirectNumberedPage(pageSize: number): number {
  const size = Number.isSafeInteger(pageSize)
    ? Math.min(MAX_PAGE_SIZE, Math.max(MIN_PAGE_SIZE, pageSize))
    : DEFAULT_PAGE_SIZE;
  return Math.floor(MAX_NUMBERED_OFFSET / size) + 1;
}

export interface NormalizedNumberedPage {
  page: number;
  maxPage: number;
  /** The URL asked for a valid positive page beyond the direct-jump budget. */
  exceededLimit: boolean;
  /** Canonical URL value. Page one is represented by an absent parameter. */
  urlValue: string | null;
}

/** Parses the URL strictly and caps it before it can become a backend OFFSET. */
export function normalizeNumberedPage(raw: string | null, pageSize: number): NormalizedNumberedPage {
  const maxPage = maxDirectNumberedPage(pageSize);
  const digitsOnly = raw !== null && /^\d+$/.test(raw);
  const numeric = digitsOnly ? Number(raw) : 1;
  const exceededLimit = digitsOnly && numeric > 0 && (!Number.isSafeInteger(numeric) || numeric > maxPage);
  const page = exceededLimit
    ? maxPage
    : Number.isSafeInteger(numeric) && numeric > 0
      ? numeric
      : 1;
  return { page, maxPage, exceededLimit, urlValue: page > 1 ? String(page) : null };
}

/**
 * Canonicalizes `?page=` and remembers when a deep link had to be bounded.
 *
 * The remembered flag intentionally survives the replace-navigation that this
 * hook performs. Otherwise the explanation would be rendered for one frame,
 * disappear with the invalid URL, and leave the reader on an unexpected page.
 */
export function useBoundedNumberedPage(
  enabled: boolean,
  params: URLSearchParams,
  setParams: (params: URLSearchParams, options?: { replace?: boolean }) => void,
  pageSize: number,
) {
  const raw = params.get("page");
  const normalized = normalizeNumberedPage(raw, pageSize);
  const desired = enabled ? normalized.urlValue : null;
  const [limitNotice, setLimitNotice] = useState(false);
  const normalizationTarget = useRef<string | null | undefined>(undefined);

  useEffect(() => {
    // This is the navigation caused by the previous pass. Preserve its notice;
    // a genuinely new, already-valid URL clears it below.
    if (normalizationTarget.current !== undefined && raw === normalizationTarget.current) {
      normalizationTarget.current = undefined;
      return;
    }

    setLimitNotice(enabled && normalized.exceededLimit);
    if (raw === desired) return;

    normalizationTarget.current = desired;
    const next = new URLSearchParams(params);
    if (desired === null) next.delete("page"); else next.set("page", desired);
    setParams(next, { replace: true });
  }, [desired, enabled, normalized.exceededLimit, params, raw, setParams]);

  const clearLimitNotice = useCallback(() => setLimitNotice(false), []);
  return { ...normalized, limitNotice, clearLimitNotice };
}

export function usePageSize(): [number, (size: number) => void] {
  const [stored, setStored] = useLocalStorage<unknown>({ key: PAGE_SIZE_KEY, defaultValue: DEFAULT_PAGE_SIZE, ...READ_ON_FIRST_RENDER });
  // Any usable size is accepted, not only the four the settings screen offers:
  // the list there is a set of sensible choices, not the definition of what is
  // valid, and rejecting anything else turned a hand-set value into a silent
  // reset to twenty.
  const size = typeof stored === "number" && Number.isSafeInteger(stored) && stored >= MIN_PAGE_SIZE && stored <= MAX_PAGE_SIZE
    ? stored
    : DEFAULT_PAGE_SIZE;
  return [size, (next) => setStored(next)];
}

function readMode(stored: unknown): PagingMode {
  // "manual" was a third mode that only turned scrolling off; anyone left on it
  // gets scrolling back, which still has its button.
  return stored === "pages" ? "pages" : "auto";
}

/**
 * 全体の既定。個別に決めていない一覧は、これに従う。
 *
 * 設定画面の「まとめて変える」がここを書く。個別の上書きも同時に消すので、
 * 押した結果がどの画面でも同じになる。
 */
export function useDefaultPagingMode(): [PagingMode, (mode: PagingMode) => void] {
  const [stored, setStored] = useLocalStorage<unknown>({ key: STORAGE_KEY, defaultValue: "auto", ...READ_ON_FIRST_RENDER });
  return [readMode(stored), (next) => setStored(next)];
}

/**
 * 一覧ひとつぶんの上書き。`inherit` は「全体に従う」。
 *
 * 設定画面が個別に選ばせる単位でもある。一覧の読み込み方は画面ごとに向き不向き
 * があり（束の中身は番号で飛びたいが、棚は流し読みしたい、など）、全部を一つの
 * 好みに縛ると、どちらかが必ず不便になる。
 */
export function useScopedPagingPreference(
  scope: PagingScope,
): [PagingPreference, (preference: PagingPreference) => void] {
  const [stored, setStored] = useLocalStorage<unknown>({
    key: `${STORAGE_KEY}.${scope}`,
    defaultValue: "inherit",
    ...READ_ON_FIRST_RENDER,
  });
  const preference: PagingPreference = stored === "pages" || stored === "auto" ? stored : "inherit";
  return [preference, (next) => setStored(next)];
}

/**
 * その一覧で実際に使う読み込み方。
 *
 * **ボタンで変えると、その一覧の上書きとして残る。** localStorage なので、
 * アプリを閉じても次に開いたときの状態は同じである。上書きを持たない一覧は
 * 全体の既定に従うので、設定でまとめて変えるとそれらは追いついてくる。
 */
export function usePagingMode(scope: PagingScope): [PagingMode, (mode: PagingMode) => void] {
  const [fallback] = useDefaultPagingMode();
  const [preference, setPreference] = useScopedPagingPreference(scope);
  return [preference === "inherit" ? fallback : preference, (next) => setPreference(next)];
}

/**
 * Switches how a listing advances, beside the count it changes.
 *
 * A toggle rather than a menu: there are two states and the one in force should
 * be visible without opening anything. It also has to be here, at the top - the
 * same switch under the pager is unreachable while scrolling keeps extending
 * the list, which left the settings screen as the only way back out.
 */
export function PagingModeToggle({ scope, size = "xs" }: { scope: PagingScope; size?: "xs" | "sm" }) {
  const [mode, setMode] = usePagingMode(scope);
  const options: { value: PagingMode; label: string; icon: LucideIcon; hint: string }[] = [
    { value: "auto", label: "自動", icon: Icons.pagingContinuous, hint: "スクロールすると続きを自動で読み込みます" },
    { value: "pages", label: "ページ番号", icon: Icons.pagingNumbered, hint: "ページ番号で移動します（並び順を選んでいるときのみ）" },
  ];
  return (
    <SegmentedControl
      size={size}
      value={mode}
      onChange={(value) => setMode(value as PagingMode)}
      aria-label="一覧の読み込み方"
      data={options.map((option) => ({
        value: option.value,
        label: (
          <Tooltip label={option.hint} multiline w={240}>
            <Group gap={5} wrap="nowrap" component="span">
              <option.icon size={IconSize.inline} aria-hidden />
              <span>{option.label}</span>
            </Group>
          </Tooltip>
        ),
      }))}
    />
  );
}

export interface ListPagerProps {
  /** どの一覧か。読み込み方をこの単位で覚える。 */
  scope: PagingScope;
  hasNext: boolean;
  loading: boolean;
  /** How many items are on screen now. */
  loaded: number;
  /** How many there are in total, when that is known. */
  total: number | null;
  onLoad: () => void;
  /** Replaces the normal completion text when loading stopped at a safety limit. */
  endMessage?: string;
  unit?: string;
  /** Present when the listing can jump to a numbered page. */
  pages?: {
    /** One-based. */
    current: number;
    size: number;
    onGoTo: (page: number) => void;
    /** Highest page that may be translated to an OFFSET query. */
    maxDirectPage?: number;
    /** A pasted/deep-linked page was normalized to the direct-jump limit. */
    limitNotice?: boolean;
    /** Why numbers are unavailable right now, if they are. */
    unavailableReason?: string | null;
  };
}

/**
 * Page numbers around the current one, with gaps marked.
 *
 * `slots` is how many numbers actually fit: a wide window should show a usable
 * run of pages rather than the same three it would show on a narrow one.
 */
export function pageWindow(current: number, last: number, slots: number): (number | "gap")[] {
  const budget = Math.max(5, Math.min(slots, last));
  if (last <= budget) return Array.from({ length: last }, (_, index) => index + 1);

  // The first and last page are always reachable; the rest of the budget is a
  // run centred on where the reader is, minus the two gap markers.
  const inner = Math.max(1, budget - 2 - (current > 3 ? 1 : 0) - (current < last - 2 ? 1 : 0));
  let start = Math.max(2, current - Math.floor((inner - 1) / 2));
  let end = start + inner - 1;
  if (end > last - 1) {
    end = last - 1;
    start = Math.max(2, end - inner + 1);
  }

  const out: (number | "gap")[] = [1];
  if (start > 2) out.push("gap");
  for (let page = start; page <= end; page += 1) out.push(page);
  if (end < last - 1) out.push("gap");
  out.push(last);
  return out;
}

/** Roughly what one page button occupies, including the gap after it. */
const PAGE_BUTTON_WIDTH = 50;
/** The 前へ / 次へ controls at either end. */
const PAGE_EDGE_WIDTH = 196;
/**
 * Past this the numbers stop being scannable, however wide the window is.
 *
 * A row of twenty is not read as a row of numbers, it is read as clutter you
 * have to search - and the pages near the one you are on are the only ones
 * anybody aims at, with the first and last always reachable at the ends.
 */
const MAX_PAGE_SLOTS = 9;

/** How many page numbers the available width can hold. */
function usePageSlots(ref: React.RefObject<HTMLDivElement | null>): number {
  const [slots, setSlots] = useState(7);
  useEffect(() => {
    const element = ref.current;
    if (!element) return;
    const measure = () => {
      const usable = element.clientWidth - PAGE_EDGE_WIDTH;
      setSlots(Math.max(5, Math.min(MAX_PAGE_SLOTS, Math.floor(usable / PAGE_BUTTON_WIDTH))));
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [ref]);
  return slots;
}

/**
 * Advances a long listing, and says where in it you are.
 *
 * Two things were missing. Entity pages only ever offered a button, so reaching
 * the end of a prolific author's works meant pressing it over and over, while
 * the library loaded as you scrolled - the same listing behaving two different
 * ways. And neither said how much was left, so there was no way to tell a short
 * listing from a long one until you got there.
 *
 * Loading as you scroll is the default because it is what a listing wants to
 * do; the preference exists because it also makes the end of the page a moving
 * target, which is miserable when you are trying to reach the controls below it.
 */
export function ListPager({ scope, hasNext, loading, loaded, total, onLoad, endMessage, unit = "件", pages }: ListPagerProps) {
  const [stored] = usePagingMode(scope);
  // Numbers were asked for but this listing cannot honour them: fall back to
  // scrolling rather than showing controls that would land somewhere else.
  const numbersUnavailable = stored === "pages" && (!pages || Boolean(pages.unavailableReason));
  const mode: PagingMode = numbersUnavailable ? "auto" : stored;
  const sentinelRef = useRef<HTMLDivElement>(null);
  const pagerRef = useRef<HTMLDivElement>(null);
  const slots = usePageSlots(pagerRef);
  const loadRef = useRef(onLoad);
  const canLoadRef = useRef(false);
  loadRef.current = onLoad;
  canLoadRef.current = hasNext && !loading && mode === "auto";

  useEffect(() => {
    const node = sentinelRef.current;
    if (!node || !hasNext || mode !== "auto" || typeof IntersectionObserver === "undefined") return;
    const observer = new IntersectionObserver((entries) => {
      if (canLoadRef.current && entries.some((entry) => entry.isIntersecting)) {
        // Cleared immediately so one long scroll does not fire the same page
        // request several times before the first lands.
        canLoadRef.current = false;
        loadRef.current();
      }
    }, { rootMargin: "480px" });
    observer.observe(node);
    return () => observer.disconnect();
  }, [hasNext, mode]);

  const remaining = total === null ? null : Math.max(0, total - loaded);
  const reason = numbersUnavailable && pages?.unavailableReason
    ? <Text size="xs" c="dimmed" ta="center" maw={420}>{pages.unavailableReason}</Text>
    : null;

  // The outer element is the same in every mode so the width is measured from
  // the moment the pager exists. Measuring only in numbered mode meant that
  // switching into it never triggered a measurement at all, and a window with
  // room for twenty pages kept showing the seven of the initial guess.
  return (
    <Stack className="list-pager" align="center" gap={10} mt="xl" w="100%" ref={pagerRef}>
      {mode === "pages" && pages ? (
        <PageNumbers
          current={pages.current}
          // Some listings cannot say how many rows they have without a second
          // count over the whole table. Not knowing the last page is no reason
          // to refuse numbers: the one after this one is known to exist, which
          // is all 次へ needs, and treating the current page as the last is how
          // this used to strand the reader on page one.
          actualLast={total === null
            ? pages.current + (hasNext ? 1 : 0)
            : Math.max(1, Math.ceil(total / pages.size))}
          maxDirectPage={pages.maxDirectPage}
          limitNotice={pages.limitNotice}
          openEnded={total === null && hasNext}
          slots={slots}
          loading={loading}
          onGoTo={pages.onGoTo}
          total={total}
          unit={unit}
        />
      ) : !hasNext ? (
        <>
          {loaded > 0 && <Text size="xs" c="dimmed">{endMessage ?? `すべて表示しました（${formatNumber(loaded)}${unit}）`}</Text>}
          {reason}
        </>
      ) : (
        <>
          {/* Given a size on purpose: a zero-area element is not reliably
              reported as intersecting, and this one decides when the next
              batch is fetched. */}
          <div ref={sentinelRef} aria-hidden style={{ width: "100%", height: 1 }} />
          <Button
            variant="default"
            loading={loading}
            onClick={onLoad}
            leftSection={<Icons.down size={IconSize.menu} />}
          >
            {remaining === null ? "さらに読み込む" : `さらに読み込む（残り${formatNumber(remaining)}${unit}）`}
          </Button>
          {total !== null && (
            <Text size="xs" c="dimmed">{formatNumber(loaded)} / {formatNumber(total)}{unit}を表示中</Text>
          )}
          {reason}
        </>
      )}
    </Stack>
  );
}

/**
 * The numbers themselves.
 *
 * This is the control a hundred page listing is driven by, so the targets are
 * full sized rather than the compact ones the rest of a toolbar uses, and the
 * run of numbers spends the width it is given instead of a fixed few. The mode
 * switch that used to sit under here is gone: it is already beside the count at
 * the top of every listing, where it is reachable without paging to the bottom.
 */
function PageNumbers({ current, actualLast, maxDirectPage, limitNotice = false, openEnded = false, slots, loading, onGoTo, total, unit }: {
  current: number;
  actualLast: number;
  maxDirectPage?: number;
  limitNotice?: boolean;
  /** The end is not known, so the numbers stop rather than finish. */
  openEnded?: boolean;
  slots: number;
  loading: boolean;
  onGoTo: (page: number) => void;
  total: number | null;
  unit: string;
}) {
  const last = Math.max(1, Math.min(actualLast, maxDirectPage ?? actualLast));
  const directLimitReached = actualLast > last;
  return (
    <>
      <Group gap={6} justify="center" wrap="nowrap" w="100%" className="list-pager__pages">
        <Button className="list-pager__edge" size="sm" variant="default" disabled={current <= 1 || loading} onClick={() => onGoTo(current - 1)} leftSection={<Icons.previous size={IconSize.inline} />}>前へ</Button>
        {pageWindow(current, last, slots).map((page, index) => page === "gap"
          ? <Text key={`gap-${index}`} size="sm" c="dimmed" px={2}>…</Text>
          : (
            <Button
              key={page}
              className="list-pager__page"
              size="sm"
              variant={page === current ? "filled" : "default"}
              disabled={loading}
              onClick={() => onGoTo(page)}
              aria-current={page === current ? "page" : undefined}
              aria-label={`${page}ページ目`}
            >
              {page}
            </Button>
          ))}
        {openEnded && <Text size="sm" c="dimmed" px={2}>…</Text>}
        <Button className="list-pager__edge" size="sm" variant="default" disabled={current >= last || loading} onClick={() => onGoTo(current + 1)} rightSection={<Icons.next size={IconSize.inline} />}>次へ</Button>
      </Group>
      {total !== null
        ? directLimitReached
          ? <Text size="xs" c="dimmed">{formatNumber(total)}{unit}中 {current}ページ目 · ページ番号は{last}ページ目まで</Text>
          : <Text size="xs" c="dimmed">{formatNumber(total)}{unit}中 {current} / {last}ページ</Text>
        : <Text size="xs" c="dimmed">{current}ページ目{openEnded ? "" : "（最後）"}</Text>}
      {(directLimitReached || limitNotice) && (
        <Text size="xs" c="dimmed" ta="center" maw={520}>
          負荷を抑えるため、ページ番号で直接移動できるのは{maxDirectPage ?? last}ページ目までです。それより先は「自動」を選び、検索や絞り込みを使ってください。
        </Text>
      )}
    </>
  );
}
