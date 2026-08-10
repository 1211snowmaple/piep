import { useEffect, useRef, useState } from "react";
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

const STORAGE_KEY = "piep.paging-mode";
const PAGE_SIZE_KEY = "piep.page-size";

/** How many works a page holds, or one scroll batch fetches. */
export const PAGE_SIZE_OPTIONS = [20, 40, 60, 100] as const;
export const DEFAULT_PAGE_SIZE = 20;

export function usePageSize(): [number, (size: number) => void] {
  const [stored, setStored] = useLocalStorage<unknown>({ key: PAGE_SIZE_KEY, defaultValue: DEFAULT_PAGE_SIZE });
  const size = typeof stored === "number" && (PAGE_SIZE_OPTIONS as readonly number[]).includes(stored)
    ? stored
    : DEFAULT_PAGE_SIZE;
  return [size, (next) => setStored(next)];
}

export function usePagingMode(): [PagingMode, (mode: PagingMode) => void] {
  const [stored, setStored] = useLocalStorage<unknown>({ key: STORAGE_KEY, defaultValue: "auto" });
  // "manual" was a third mode that only turned scrolling off; anyone left on it
  // gets scrolling back, which still has its button.
  const mode: PagingMode = stored === "pages" ? "pages" : "auto";
  return [mode, (next) => setStored(next)];
}

/**
 * Switches how a listing advances, beside the count it changes.
 *
 * A toggle rather than a menu: there are two states and the one in force should
 * be visible without opening anything. It also has to be here, at the top - the
 * same switch under the pager is unreachable while scrolling keeps extending
 * the list, which left the settings screen as the only way back out.
 */
export function PagingModeToggle({ size = "xs" }: { size?: "xs" | "sm" }) {
  const [mode, setMode] = usePagingMode();
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
  hasNext: boolean;
  loading: boolean;
  /** How many items are on screen now. */
  loaded: number;
  /** How many there are in total, when that is known. */
  total: number | null;
  onLoad: () => void;
  unit?: string;
  /** Present when the listing can jump to a numbered page. */
  pages?: {
    /** One-based. */
    current: number;
    size: number;
    onGoTo: (page: number) => void;
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
const PAGE_BUTTON_WIDTH = 44;
/** The 前へ / 次へ controls at either end. */
const PAGE_EDGE_WIDTH = 200;

/** How many page numbers the available width can hold. */
function usePageSlots(ref: React.RefObject<HTMLDivElement | null>): number {
  const [slots, setSlots] = useState(7);
  useEffect(() => {
    const element = ref.current;
    if (!element || typeof ResizeObserver === "undefined") return;
    const measure = () => {
      const usable = element.clientWidth - PAGE_EDGE_WIDTH;
      setSlots(Math.max(5, Math.min(21, Math.floor(usable / PAGE_BUTTON_WIDTH))));
    };
    measure();
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
export function ListPager({ hasNext, loading, loaded, total, onLoad, unit = "件", pages }: ListPagerProps) {
  const [stored] = usePagingMode();
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
  const modeSwitch = <PagingModeToggle />;

  if (mode === "pages" && pages) {
    const last = total === null ? pages.current : Math.max(1, Math.ceil(total / pages.size));
    return (
      <Stack align="center" gap={8} mt="xl" w="100%" ref={pagerRef}>
        <Group gap={4} justify="center" wrap="nowrap" maw="100%">
          <Button size="compact-sm" variant="default" disabled={pages.current <= 1 || loading} onClick={() => pages.onGoTo(pages.current - 1)} leftSection={<Icons.previous size={IconSize.inline} />}>前へ</Button>
          {pageWindow(pages.current, last, slots).map((page, index) => page === "gap"
            ? <Text key={`gap-${index}`} size="sm" c="dimmed" px={4}>…</Text>
            : (
              <Button
                key={page}
                size="compact-sm"
                variant={page === pages.current ? "filled" : "default"}
                disabled={loading}
                onClick={() => pages.onGoTo(page)}
                aria-current={page === pages.current ? "page" : undefined}
                aria-label={`${page}ページ目`}
              >
                {page}
              </Button>
            ))}
          <Button size="compact-sm" variant="default" disabled={pages.current >= last || loading} onClick={() => pages.onGoTo(pages.current + 1)} rightSection={<Icons.next size={IconSize.inline} />}>次へ</Button>
        </Group>
        {total !== null && <Text size="xs" c="dimmed">{formatNumber(total)}{unit}中 {pages.current} / {last}ページ</Text>}
        {modeSwitch}
      </Stack>
    );
  }

  if (!hasNext) {
    return (
      <Stack align="center" gap={8} mt="xl">
        {loaded > 0 && <Text size="xs" c="dimmed">すべて表示しました（{formatNumber(loaded)}{unit}）</Text>}
        {loaded > 0 && modeSwitch}
        {numbersUnavailable && pages?.unavailableReason && <Text size="xs" c="dimmed">{pages.unavailableReason}</Text>}
      </Stack>
    );
  }

  return (
    <Stack align="center" gap={8} mt="xl" ref={sentinelRef}>
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
      {numbersUnavailable && pages?.unavailableReason && (
        <Text size="xs" c="dimmed" ta="center" maw={420}>{pages.unavailableReason}</Text>
      )}
      {modeSwitch}
    </Stack>
  );
}
