import { memo, useCallback } from "react";
import { ActionIcon, Avatar, Badge, Card, Group, Menu, Text, Tooltip, UnstyledButton } from "@mantine/core";
import { modals } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { useQueryClient } from "@tanstack/react-query";
import { Icons, IconSize } from "@/lib/icons";
import { useAppNavigate } from "@/app/router";
import { useWorkspace } from "@/app/WorkspaceContext";
import { TagRow } from "@/components/TagRow";
import { WorkCover } from "@/components/WorkCover";
import { ProviderMark } from "@/lib/providers";
import { errorMessage, formatDateNumeric, formatNumber } from "@/lib/format";
import { summaryText } from "@/lib/content";
import { deleteDownload, getAssetUrl, isTauriRuntime, setFavorite, setWatchUpdates } from "@/services/dbApi";
import { deleteThenCleanup } from "@/features/library/deletedWorkCleanup";
import type { DownloadEntry } from "@/types/library";

interface WorkCardProps {
  work: DownloadEntry;
  selected?: boolean;
  selectionMode?: boolean;
  /** Receives the work id so lists can pass one stable handler for every card. */
  onSelect?: (id: number, selected: boolean) => void;
  onToggleFavorite?: (id: number, favorite: boolean) => void;
  onToggleWatch?: (id: number, watch: boolean) => void;
  compact?: boolean;
}

const CARD_CONTROL_SELECTOR = "a, button, input, select, textarea, [role='button'], [role='link'], [role='menuitem'], [data-card-control]";

function isCardControl(target: EventTarget | null) {
  return target instanceof Element && Boolean(target.closest(CARD_CONTROL_SELECTOR));
}

/**
 * "v1" told the reader nothing and could not be acted on. The chip now appears
 * only once a work has actually been re-saved, and opens the history tab where
 * the earlier revisions can be read and restored.
 */
function VersionChip({ work, interactive = true }: { work: DownloadEntry; interactive?: boolean }) {
  const navigate = useAppNavigate();
  if (work.currentVersion <= 1) return null;
  return (
    <Tooltip label={`${work.currentVersion}版まで保存されています。履歴を開く`}>
      <Badge
        component="button"
        type="button"
        size="xs"
        variant="light"
        color="piep"
        className="work-card__version"
        aria-label={`${work.title}のバージョン履歴を開く`}
        aria-hidden={interactive ? undefined : true}
        tabIndex={interactive ? undefined : -1}
        disabled={!interactive}
        onClick={(event) => { event.stopPropagation(); navigate(`/works/${work.id}?tab=history`); }}
      >
        v{work.currentVersion}
      </Badge>
    </Tooltip>
  );
}

/** The series above the title doubles as the link to it. */
function SeriesLink({ work, className, interactive = true }: { work: DownloadEntry; className?: string; interactive?: boolean }) {
  const navigate = useAppNavigate();
  if (!work.seriesTitle) return null;
  const key = work.seriesId;
  const label = <Text className={["work-card__series line-clamp-1", className].filter(Boolean).join(" ")}>{work.seriesTitle}</Text>;
  if (!key) return label;
  return (
    <UnstyledButton
      className="work-card__series-link"
      aria-label={`シリーズ「${work.seriesTitle}」を開く`}
      aria-hidden={interactive ? undefined : true}
      tabIndex={interactive ? undefined : -1}
      onClick={(event) => { event.stopPropagation(); navigate(`/series/${encodeURIComponent(work.source)}/${encodeURIComponent(key)}`); }}
    >
      {label}
    </UnstyledButton>
  );
}

function AuthorLine({ work, size = 20, interactive = true }: { work: DownloadEntry; size?: number; interactive?: boolean }) {
  const navigate = useAppNavigate();
  const name = work.personName || work.authorName;
  const icon = getAssetUrl(work.personIconPath);
  const key = work.personId || work.authorId;
  return (
    <Group
      gap={6}
      wrap="nowrap"
      className="work-card__author"
      role={key && interactive ? "link" : undefined}
      tabIndex={key ? (interactive ? 0 : -1) : undefined}
      aria-label={key && interactive ? `${name}の作品を見る` : undefined}
      aria-hidden={interactive ? undefined : true}
      onClick={(event) => { if (!key) return; event.stopPropagation(); navigate(`/people/${encodeURIComponent(work.source)}/${encodeURIComponent(key)}`); }}
      onKeyDown={(event) => {
        if (!key || (event.key !== "Enter" && event.key !== " ")) return;
        event.preventDefault();
        event.stopPropagation();
        navigate(`/people/${encodeURIComponent(work.source)}/${encodeURIComponent(key)}`);
      }}
    >
      <Avatar src={icon} size={size} radius="xl" color="gray" className="work-card__avatar" imageProps={{ loading: "lazy", decoding: "async" }}>
        <Icons.person size={Math.round(size * 0.58)} />
      </Avatar>
      <Text size="xs" c="dimmed" className="line-clamp-1">{name}</Text>
    </Group>
  );
}

const MATCH_FIELD_LABELS: Record<string, string> = {
  title: "タイトル",
  author_name: "作者",
  tags: "タグ",
  series_title: "シリーズ",
  excerpt: "概要",
  body: "本文",
};

function SearchMatchReason({ work }: { work: DownloadEntry }) {
  const fields = (work.matchFields ?? []).slice(0, 3);
  if (!fields.length) return null;
  const details = (work.scoreReasons ?? [])
    .slice(0, 5)
    .map((reason) => `${MATCH_FIELD_LABELS[reason.field] ?? reason.field}: ${reason.term}（${reason.matchType}）`)
    .join("\n");
  return (
    <Tooltip label={details || "検索語が一致した項目です"} multiline maw={360} withArrow>
      <Group gap={4} wrap="nowrap" className="work-card__search-reason" aria-label={`一致理由: ${fields.map((field) => MATCH_FIELD_LABELS[field] ?? field).join("、")}`}>
        <Icons.searchMatch size={IconSize.inline} aria-hidden />
        <Text size="xs" c="piep" fw={650}>一致</Text>
        {fields.map((field) => <Badge key={field} size="xs" variant="light" color="piep">{MATCH_FIELD_LABELS[field] ?? field}</Badge>)}
      </Group>
    </Tooltip>
  );
}

function SelectionToggle({ work, selected, compact, onSelect }: {
  work: DownloadEntry;
  selected: boolean;
  compact?: boolean;
  onSelect?: (id: number, selected: boolean) => void;
}) {
  return (
    <UnstyledButton
      className={compact ? "work-row__select" : "work-card__select"}
      data-checked={selected || undefined}
      aria-label={`${work.title}を${selected ? "選択解除" : "選択"}`}
      aria-pressed={selected}
      onClick={(event) => { event.stopPropagation(); onSelect?.(work.id, !selected); }}
    >
      <span className="work-selection-toggle__indicator" aria-hidden>{selected && <Icons.confirm size={IconSize.menu} strokeWidth={3} />}</span>
      {!compact && <span className="work-selection-toggle__label">{selected ? "選択中" : "選択"}</span>}
    </UnstyledButton>
  );
}

export const WorkCard = memo(function WorkCard({
  work,
  selected,
  selectionMode = false,
  onSelect,
  onToggleFavorite,
  onToggleWatch,
  compact = false,
}: WorkCardProps) {
  const navigate = useAppNavigate();
  const { addToEpubQueue, removeFromEpubQueue, isQueuedForEpub } = useWorkspace();
  const queued = isQueuedForEpub(work.id);
  const open = useCallback(() => {
    if (selectionMode) onSelect?.(work.id, !selected);
    else navigate(`/works/${work.id}`);
  }, [navigate, onSelect, selected, selectionMode, work.id]);
  const keyboardOpen = (event: React.KeyboardEvent) => {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      open();
    }
  };
  const openFromCardSurface = (event: React.MouseEvent<HTMLElement>) => {
    if (!isCardControl(event.target)) open();
  };
  // Selection mode is modal: every part of the card selects the work instead
  // of unexpectedly following an author, series, tag, or action link.
  const selectFromCardCapture = (event: React.MouseEvent<HTMLElement>) => {
    if (!selectionMode || (event.target instanceof Element && event.target.closest(".work-card__select, .work-row__select"))) return;
    event.preventDefault();
    event.stopPropagation();
    onSelect?.(work.id, !selected);
  };
  const selectFromKeyboardCapture = (event: React.KeyboardEvent<HTMLElement>) => {
    if (!selectionMode || (event.key !== "Enter" && event.key !== " ") || (event.target instanceof Element && event.target.closest(".work-card__select, .work-row__select"))) return;
    event.preventDefault();
    event.stopPropagation();
    onSelect?.(work.id, !selected);
  };
  const actions = (
    <WorkActions
      work={work}
      queued={queued}
      onQueue={() => queued ? removeFromEpubQueue(work.id) : addToEpubQueue(work.id)}
      onToggleFavorite={onToggleFavorite}
      onToggleWatch={onToggleWatch}
    />
  );

  if (compact) {
    // A table, not a shrunken card: the columns line up down the list so the
    // eye can scan one attribute at a time, and the extra width on a wide
    // window goes to tags rather than to a title stretched across the screen.
    return (
      <Card className="work-row surface--interactive" padding={0} onClick={openFromCardSurface} onClickCapture={selectFromCardCapture} onKeyDownCapture={selectFromKeyboardCapture} data-selection-mode={selectionMode || undefined} data-selected={selectionMode && selected || undefined}>
        <div className="work-row__grid">
          <div className="work-row__cover">
            <WorkCover work={work} variant="compact" />
            {selectionMode && <SelectionToggle work={work} selected={Boolean(selected)} compact onSelect={onSelect} />}
          </div>
          <div className="work-row__main" inert={selectionMode || undefined} aria-hidden={selectionMode || undefined}>
            <SeriesLink work={work} className="work-row__series" interactive={!selectionMode} />
            <UnstyledButton className="work-row__open" onClick={open} onKeyDown={keyboardOpen} role={selectionMode ? undefined : "link"} aria-label={selectionMode ? undefined : `${work.title}を開く`} aria-hidden={selectionMode || undefined} tabIndex={selectionMode ? -1 : undefined}>
              <Text fw={650} size="sm" className="line-clamp-1">{work.title}</Text>
            </UnstyledButton>
            <div className="work-row__identity">
              <AuthorLine work={work} size={17} interactive={!selectionMode} />
              <span className="work-card__identity-divider" aria-hidden />
              <ProviderMark provider={work.source} compact className="work-row__provider" />
            </div>
            <SearchMatchReason work={work} />
          </div>
          <div className="work-row__tags" inert={selectionMode || undefined} aria-hidden={selectionMode || undefined}><TagRow tags={work.tags} interactive={!selectionMode} /></div>
          <Group gap="sm" wrap="nowrap" className="work-row__facts" inert={selectionMode || undefined} aria-hidden={selectionMode || undefined}>
            <Text size="xs" c="dimmed"><Icons.publishedDate size={IconSize.inline} />{formatDateNumeric(work.sourceCreatedAt)}</Text>
            <Text size="xs" c="dimmed"><Icons.textLength size={IconSize.inline} />{formatNumber(work.textLength)}字</Text>
            {work.assetCount > 0 && <Text size="xs" c="dimmed"><Icons.assets size={IconSize.inline} />{work.assetCount}</Text>}
            <VersionChip work={work} interactive={!selectionMode} />
          </Group>
          {!selectionMode && <div className="work-row__actions">{actions}</div>}
        </div>
      </Card>
    );
  }

  return (
    <Card className="work-card surface--interactive" padding={0} onClick={openFromCardSurface} onClickCapture={selectFromCardCapture} onKeyDownCapture={selectFromKeyboardCapture} data-selection-mode={selectionMode || undefined} data-selected={selectionMode && selected || undefined}>
      <div className="work-card__inner">
        <div className="work-card__cover-rail">
          <WorkCover work={work} variant="card" className="work-card__cover" />
          {selectionMode && <SelectionToggle work={work} selected={Boolean(selected)} onSelect={onSelect} />}
        </div>
        <div className="work-card__body" inert={selectionMode || undefined} aria-hidden={selectionMode || undefined}>
          {work.seriesTitle && <div className="work-card__meta"><SeriesLink work={work} interactive={!selectionMode} /></div>}
          <UnstyledButton className="work-card__open" onClick={open} onKeyDown={keyboardOpen} role={selectionMode ? undefined : "link"} aria-label={selectionMode ? undefined : `${work.title}を開く`} aria-hidden={selectionMode || undefined} tabIndex={selectionMode ? -1 : undefined}>
            <Text fw={720} className="work-card__title line-clamp-2" lh={1.32}>{work.title}</Text>
          </UnstyledButton>
          <div className="work-card__identity">
            <AuthorLine work={work} interactive={!selectionMode} />
            <span className="work-card__identity-divider" aria-hidden />
            <ProviderMark provider={work.source} compact className="work-card__provider" />
          </div>
          {work.excerpt && <Text size="xs" c="dimmed" className="line-clamp-2 work-card__excerpt">{summaryText(work.excerpt)}</Text>}
          <SearchMatchReason work={work} />
          {work.tags.length > 0 && <div className="work-card__tagslot"><TagRow tags={work.tags} interactive={!selectionMode} /></div>}
        </div>
        <div className="work-card__footer" inert={selectionMode || undefined} aria-hidden={selectionMode || undefined}>
          <Group gap="sm" wrap="nowrap" className="work-card__facts">
            <Text size="xs" c="dimmed"><Icons.publishedDate size={IconSize.inline} />{formatDateNumeric(work.sourceCreatedAt)}</Text>
            <Text size="xs" c="dimmed"><Icons.textLength size={IconSize.inline} />{formatNumber(work.textLength)}字</Text>
            {work.assetCount > 0 && <Text size="xs" c="dimmed"><Icons.assets size={IconSize.inline} />{work.assetCount}</Text>}
            <VersionChip work={work} interactive={!selectionMode} />
          </Group>
          {!selectionMode && actions}
        </div>
      </div>
    </Card>
  );
});

function WorkActions({ work, queued, onQueue, onToggleFavorite, onToggleWatch }: {
  work: DownloadEntry;
  queued: boolean;
  onQueue: () => void;
  onToggleFavorite?: (id: number, favorite: boolean) => void;
  onToggleWatch?: (id: number, watch: boolean) => void;
}) {
  const navigate = useAppNavigate();
  const queryClient = useQueryClient();
  const { removeFromEpubQueue } = useWorkspace();
  const refresh = () => Promise.all([
    queryClient.invalidateQueries({ queryKey: ["library"] }),
    queryClient.invalidateQueries({ queryKey: ["dashboard"] }),
    queryClient.invalidateQueries({ queryKey: ["reader-metadata", work.id] }),
  ]);
  const toggleFavorite = async () => {
    const next = !work.favorite;
    if (onToggleFavorite) return onToggleFavorite(work.id, next);
    try {
      if (isTauriRuntime()) await setFavorite(work.id, next);
      await refresh();
    } catch (error) {
      notifications.show({ color: "red", title: "お気に入りを変更できません", message: errorMessage(error) });
    }
  };
  // Watching was previously read-only here: the button only linked to the
  // update centre and was disabled precisely when you wanted to turn it on.
  const toggleWatch = async () => {
    const next = !work.watchUpdates;
    if (onToggleWatch) return onToggleWatch(work.id, next);
    try {
      if (isTauriRuntime()) await setWatchUpdates(work.id, next);
      await refresh();
      notifications.show({ color: next ? "piep" : "gray", message: next ? "更新監視をオンにしました" : "更新監視をオフにしました" });
    } catch (error) {
      notifications.show({ color: "red", title: "更新監視を変更できません", message: errorMessage(error) });
    }
  };
  const confirmDelete = () => modals.openConfirmModal({
    title: "この作品を削除しますか？",
    children: <Text size="sm">「{work.title}」と保存済みアセットをライブラリから削除します。この操作は元に戻せません。</Text>,
    labels: { confirm: "削除する", cancel: "キャンセル" },
    confirmProps: { color: "red" },
    onConfirm: async () => {
      try {
        await deleteThenCleanup(
          () => isTauriRuntime() ? deleteDownload(work.id) : Promise.resolve(),
          { queryClient, ids: [work.id], removeFromEpubQueue },
        );
        notifications.show({ color: "green", message: "作品を削除しました" });
      } catch (error) {
        notifications.show({ color: "red", title: "削除できません", message: errorMessage(error) });
      }
    },
  });

  return (
    <Group gap={2} wrap="nowrap" justify="flex-end" className="work-card__actions" onClick={(event) => event.stopPropagation()}>
      {/* Nothing here carries a filled chip. A row of five solid squares is the
          loudest thing on a screen of two thousand works, and it competed with
          the covers the listing exists to show. State is drawn in the glyph
          itself: grey when off, its own colour when on, and filled in where the
          shape has an inside to fill. */}
      <Tooltip label="読む">
        <ActionIcon variant="subtle" style={{ "--ai-color": "var(--flag-watch)" }} size="md" aria-label={`${work.title}を読む`} onClick={() => navigate(`/reader/${work.id}`)}>
          <Icons.read size={IconSize.action} />
        </ActionIcon>
      </Tooltip>
      <Tooltip label={work.favorite ? "お気に入りを解除" : "お気に入りに追加"}>
        <ActionIcon variant="subtle" color={work.favorite ? "red" : "gray"} style={{ "--ai-color": work.favorite ? "var(--flag-favorite)" : "var(--flag-off)", "--ai-bg": work.favorite ? "var(--flag-favorite-bg)" : "transparent" }} size="md" aria-label={work.favorite ? "お気に入りを解除" : "お気に入りに追加"} aria-pressed={work.favorite} onClick={toggleFavorite}>
          <Icons.favorite size={IconSize.action} fill={work.favorite ? "currentColor" : "none"} />
        </ActionIcon>
      </Tooltip>
      {/* A rotating arrow has no inside, so this one can only change colour. */}
      <Tooltip label={work.watchUpdates ? "更新監視をオフにする" : "更新監視をオンにする"}>
        <ActionIcon variant="subtle" color={work.watchUpdates ? "piep" : "gray"} style={{ "--ai-color": work.watchUpdates ? "var(--flag-watch)" : "var(--flag-off)", "--ai-bg": work.watchUpdates ? "var(--flag-watch-bg)" : "transparent" }} size="md" aria-label={work.watchUpdates ? "更新監視をオフにする" : "更新監視をオンにする"} aria-pressed={work.watchUpdates} onClick={toggleWatch}>
          <Icons.watch size={IconSize.action} />
        </ActionIcon>
      </Tooltip>
      {/* One glyph in both states. Swapping the shape as well as the colour
          said the same thing twice, and the shape that changed was the part
          naming what the button does. Green is the "ep" half of the wordmark. */}
      <Tooltip label={queued ? "EPUBキューから外す" : "EPUBキューに追加"}>
        <ActionIcon variant="subtle" color={queued ? "leaf" : "gray"} style={{ "--ai-color": queued ? "var(--flag-epub)" : "var(--flag-off)", "--ai-bg": queued ? "var(--flag-epub-bg)" : "transparent" }} size="md" aria-label={queued ? "EPUBキューから外す" : "EPUBキューに追加"} aria-pressed={queued} onClick={onQueue}>
          <Icons.epubAdd size={IconSize.action} />
        </ActionIcon>
      </Tooltip>
      <Menu position="bottom-end" withinPortal>
        <Menu.Target><Tooltip label="その他"><ActionIcon variant="subtle" color="gray" style={{ "--ai-color": "var(--flag-off)" }} size="md" aria-label={`${work.title}のその他の操作`}><Icons.more size={IconSize.nav} /></ActionIcon></Tooltip></Menu.Target>
        <Menu.Dropdown>
          <Menu.Item leftSection={<Icons.workDetail size={IconSize.menu} />} onClick={() => navigate(`/works/${work.id}`)}>詳細</Menu.Item>
          <Menu.Item leftSection={<Icons.watch size={IconSize.menu} />} onClick={() => navigate(`/updates?work=${work.id}`)}>更新センターで確認</Menu.Item>
          <Menu.Divider />
          <Menu.Item color="red" leftSection={<Icons.delete size={IconSize.menu} />} onClick={confirmDelete}>ライブラリから削除</Menu.Item>
        </Menu.Dropdown>
      </Menu>
    </Group>
  );
}

export type { WorkCardProps };
