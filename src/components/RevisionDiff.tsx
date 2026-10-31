import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type MouseEvent, type ReactNode } from "react";
import { ActionIcon, Alert, Badge, Box, Group, SegmentedControl, Stack, Text, Tooltip } from "@mantine/core";
import { RevisionMetadataDiff } from "./RevisionMetadataDiff";
import { LoadingState } from "./AsyncState";
import type { RevisionMetadata } from "@/services/workInspectionApi";
import { compactTextDiff, type DisplayDiffPart } from "@/lib/textDiff";
import { useTextDiff } from "@/lib/useTextDiff";
import { overviewMarks, type OverviewKind, type OverviewMark } from "@/lib/diffOverview";
import { Icons, IconSize } from "@/lib/icons";

interface RevisionDiffProps {
  beforeMetadata?: RevisionMetadata;
  afterMetadata?: RevisionMetadata;
  beforeVersion: number | null;
  afterVersion: number;
  beforeText: string | null;
  afterText: string;
  beforeLabel?: string;
  afterLabel?: string;
  actions?: ReactNode;
}

export function RevisionDiff({ beforeMetadata, afterMetadata, beforeVersion, afterVersion, beforeText, afterText, beforeLabel, afterLabel, actions }: RevisionDiffProps) {
  const [view, setView] = useState<"changes" | "text">("changes");
  const { diff, pending } = useTextDiff(beforeText, afterText);
  const displayParts = useMemo(() => diff ? compactTextDiff(diff.parts) : [], [diff]);
  const resolvedBeforeLabel = beforeLabel ?? (beforeVersion === null ? null : `v${beforeVersion}`);
  const resolvedAfterLabel = afterLabel ?? `v${afterVersion}`;

  useEffect(() => setView("changes"), [afterVersion, beforeVersion, resolvedAfterLabel, resolvedBeforeLabel]);

  return <Stack gap="md">
    <Group justify="space-between" align="flex-start" gap="sm">
      <Box>
        <Text fw={700}>{resolvedBeforeLabel === null ? `${resolvedAfterLabel}の本文` : `${resolvedBeforeLabel} → ${resolvedAfterLabel} の変更`}</Text>
        {diff?.changed && <Group gap={6} mt={6} aria-label={`追加${diff.addedCharacters}字、削除${diff.removedCharacters}字`}>
          <Badge size="sm" color="green" variant="light">+{diff.addedCharacters.toLocaleString("ja-JP")}字</Badge>
          <Badge size="sm" color="red" variant="light">−{diff.removedCharacters.toLocaleString("ja-JP")}字</Badge>
          {diff.granularity === "range" && <Badge size="sm" color="gray" variant="light">大きな変更</Badge>}
        </Group>}
      </Box>
      <Group gap="xs" justify="flex-end">
        {beforeVersion !== null && <SegmentedControl
          size="xs"
          aria-label="履歴の表示方法"
          value={view}
          onChange={(value) => setView(value as "changes" | "text")}
          data={[{ value: "changes", label: "差分" }, { value: "text", label: `${resolvedAfterLabel}全文` }]}
        />}
        {actions}
      </Group>
    </Group>

    {beforeMetadata && afterMetadata && <RevisionMetadataDiff before={beforeMetadata} after={afterMetadata} />}

    {view === "text" || beforeVersion === null
      ? <div className="version-preview__text">{afterText || "本文がありません"}</div>
      : pending
        ? <LoadingState label="本文の差分を計算しています" />
        : !diff?.changed
          ? <Alert color="gray">本文の変更はありません。</Alert>
          : <DiffContent parts={displayParts} label={`${resolvedBeforeLabel}から${resolvedAfterLabel}への本文差分`} />}
  </Stack>;
}

/**
 * 差分の本文。横に、どこが変わったかを示す帯（エディタの概観ルーラーにあたる）と、
 * 変更を一つずつ前後へ辿る操作を添える。
 */
function DiffContent({ parts, label }: { parts: DisplayDiffPart[]; label: string }) {
  const contentRef = useRef<HTMLDivElement>(null);
  const hunkTops = useRef<number[]>([]);
  const [marks, setMarks] = useState<OverviewMark[]>([]);
  const [viewport, setViewport] = useState<{ top: number; height: number } | null>(null);
  const [current, setCurrentState] = useState(-1);
  // Read by key repeats that arrive before React re-renders.
  const currentRef = useRef(-1);
  const setCurrent = useCallback((index: number) => {
    currentRef.current = index;
    setCurrentState(index);
  }, []);

  // A run of removed and added parts between unchanged text is one change.
  const { hunkOf, total } = useMemo(() => {
    let count = 0;
    let previous = false;
    const hunkOf = parts.map((part) => {
      const changed = part.kind === "added" || part.kind === "removed";
      if (changed && !previous) count += 1;
      previous = changed;
      return changed ? count - 1 : -1;
    });
    return { hunkOf, total: count };
  }, [parts]);

  useEffect(() => setCurrent(-1), [parts, setCurrent]);

  const updateViewport = useCallback(() => {
    const element = contentRef.current;
    if (!element || element.scrollHeight <= element.clientHeight) return setViewport(null);
    setViewport({ top: element.scrollTop / element.scrollHeight, height: element.clientHeight / element.scrollHeight });
  }, []);

  useLayoutEffect(() => {
    const element = contentRef.current;
    if (!element) return;
    const measure = () => {
      const base = element.getBoundingClientRect().top - element.scrollTop;
      const tops: number[] = [];
      const spans = Array.from(element.querySelectorAll<HTMLElement>(".revision-diff__part"), (part) => {
        const rect = part.getBoundingClientRect();
        const hunk = Number(part.dataset.hunk);
        if (tops[hunk] === undefined) tops[hunk] = rect.top - base;
        return { kind: part.dataset.kind as OverviewKind, top: rect.top - base, bottom: rect.bottom - base };
      });
      hunkTops.current = tops;
      setMarks(element.scrollHeight > element.clientHeight ? overviewMarks(spans, element.scrollHeight, element.clientHeight) : []);
      updateViewport();
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [parts, updateViewport]);

  useEffect(() => {
    const element = contentRef.current;
    if (!element) return;
    let frame = 0;
    const onScroll = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(updateViewport);
    };
    element.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      cancelAnimationFrame(frame);
      element.removeEventListener("scroll", onScroll);
    };
  }, [updateViewport]);

  /** いま選んでいる変更が画面に見えていればそこから、見えていなければ読者がスクロールした位置から辿る。 */
  const go = (direction: 1 | -1) => {
    const element = contentRef.current;
    const tops = hunkTops.current;
    if (!element || !tops.length) return;
    const viewTop = element.scrollTop;
    const viewBottom = viewTop + element.clientHeight;
    const current = currentRef.current;
    let target = -1;
    if (current >= 0 && tops[current] >= viewTop - 1 && tops[current] < viewBottom) target = current + direction;
    else if (direction === 1) target = tops.findIndex((top) => top >= viewTop - 1);
    else for (let index = tops.length - 1; index >= 0 && target < 0; index -= 1) if (tops[index] < viewTop) target = index;
    if (target < 0 || target >= tops.length) return;
    setCurrent(target);
    element.scrollTo({ top: Math.max(0, tops[target] - element.clientHeight / 3) });
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.altKey || event.ctrlKey || event.metaKey) return;
    const key = event.key.toLowerCase();
    if (key !== "n" && key !== "p") return;
    event.preventDefault();
    go(key === "n" ? 1 : -1);
  };

  const jump = (event: MouseEvent<HTMLDivElement>) => {
    const element = contentRef.current;
    if (!element) return;
    const rect = event.currentTarget.getBoundingClientRect();
    const fraction = (event.clientY - rect.top) / rect.height;
    element.scrollTo({ top: fraction * element.scrollHeight - element.clientHeight / 2 });
  };

  return <Stack gap={6} onKeyDown={onKeyDown}>
    <Group justify="space-between" gap="xs">
      <Text size="xs" c="dimmed">N・P キーでも移動できます</Text>
      <Group gap={4}>
        <Tooltip label="前の変更（P）"><ActionIcon variant="default" size="sm" aria-label="前の変更" disabled={current === 0} onClick={() => go(-1)}><Icons.up size={IconSize.action} /></ActionIcon></Tooltip>
        <Text size="xs" miw={64} ta="center" aria-live="polite" aria-label={current < 0 ? `変更${total}か所` : `${total}か所中${current + 1}か所目の変更`}>
          {current < 0 ? "–" : (current + 1).toLocaleString("ja-JP")} / {total.toLocaleString("ja-JP")}
        </Text>
        <Tooltip label="次の変更（N）"><ActionIcon variant="default" size="sm" aria-label="次の変更" disabled={current === total - 1} onClick={() => go(1)}><Icons.down size={IconSize.action} /></ActionIcon></Tooltip>
      </Group>
    </Group>
    <div className="revision-diff__frame" data-overview={viewport ? "" : undefined}>
      <div ref={contentRef} className="revision-diff__content" role="region" tabIndex={0} aria-label={label}>
        {parts.map((part, index) => {
          const hunk = hunkOf[index];
          const currentHunk = hunk === current || undefined;
          if (part.kind === "added") return <ins key={index} className="revision-diff__part" data-kind="added" data-hunk={hunk} data-current={currentHunk}>{part.value}</ins>;
          if (part.kind === "removed") return <del key={index} className="revision-diff__part" data-kind="removed" data-hunk={hunk} data-current={currentHunk}>{part.value}</del>;
          if (part.kind === "omitted") return <span key={index} className="revision-diff__omission">{part.value}</span>;
          return <span key={index}>{part.value}</span>;
        })}
      </div>
      {viewport && <div className="revision-diff__overview" aria-hidden="true" title="クリックでその位置へ移動" onClick={jump}>
        {marks.map((mark, index) => <span key={index} className="revision-diff__overview-mark" data-kind={mark.kind} style={{ top: `${mark.top * 100}%`, height: `${mark.height * 100}%` }} />)}
        <span className="revision-diff__overview-viewport" style={{ top: `${viewport.top * 100}%`, height: `${viewport.height * 100}%` }} />
      </div>}
    </div>
  </Stack>;
}
