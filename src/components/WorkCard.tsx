import { memo, useCallback } from "react";
import { ActionIcon, Avatar, Badge, Card, Checkbox, Group, Menu, Text, Tooltip, UnstyledButton } from "@mantine/core";
import { modals } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { useQueryClient } from "@tanstack/react-query";
import { BookCheck, BookOpen, BookPlus, Ellipsis, Heart, Images, Library, RefreshCw, Trash2, UserRound } from "lucide-react";
import { useAppNavigate } from "@/app/router";
import { useWorkspace } from "@/app/WorkspaceContext";
import { TagRow } from "@/components/TagRow";
import { WorkCover } from "@/components/WorkCover";
import { ProviderMark } from "@/lib/providers";
import { errorMessage, formatDateNumeric, formatNumber } from "@/lib/format";
import { summaryText } from "@/lib/content";
import { deleteDownload, getAssetUrl, isTauriRuntime, setFavorite, setWatchUpdates } from "@/services/dbApi";
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

/**
 * "v1" told the reader nothing and could not be acted on. The chip now appears
 * only once a work has actually been re-saved, and opens the history tab where
 * the earlier revisions can be read and restored.
 */
function VersionChip({ work }: { work: DownloadEntry }) {
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
        onClick={(event) => { event.stopPropagation(); navigate(`/works/${work.id}?tab=history`); }}
      >
        v{work.currentVersion}
      </Badge>
    </Tooltip>
  );
}

function AuthorLine({ work, size = 20 }: { work: DownloadEntry; size?: number }) {
  const navigate = useAppNavigate();
  const name = work.personName || work.authorName;
  const icon = getAssetUrl(work.personIconPath);
  const key = work.personId || work.authorId;
  return (
    <Group
      gap={6}
      wrap="nowrap"
      className="work-card__author"
      role={key ? "link" : undefined}
      tabIndex={key ? 0 : undefined}
      aria-label={key ? `${name}の作品を見る` : undefined}
      onClick={(event) => { if (!key) return; event.stopPropagation(); navigate(`/people/${encodeURIComponent(work.source)}/${encodeURIComponent(key)}`); }}
      onKeyDown={(event) => {
        if (!key || (event.key !== "Enter" && event.key !== " ")) return;
        event.preventDefault();
        event.stopPropagation();
        navigate(`/people/${encodeURIComponent(work.source)}/${encodeURIComponent(key)}`);
      }}
    >
      <Avatar src={icon} size={size} radius="xl" color="gray" className="work-card__avatar">
        <UserRound size={Math.round(size * 0.58)} />
      </Avatar>
      <Text size="xs" c="dimmed" className="line-clamp-1">{name}</Text>
    </Group>
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
      <Card className="work-row surface--interactive" padding={0}>
        <div className="work-row__grid">
          {/* The source leads the row, ahead of the cover, matching the card's
              top-left stamp. */}
          <ProviderMark provider={work.source} compact className="work-row__provider" />
          <div className="work-row__cover" onClick={(event) => { if (!(event.target as HTMLElement).closest("input")) open(); }}>
            <WorkCover work={work} variant="compact" />
            {selectionMode && <Checkbox className="work-row__select" checked={selected} onChange={(e) => onSelect?.(work.id, e.currentTarget.checked)} onClick={(e) => e.stopPropagation()} aria-label={`${work.title}を選択`} />}
          </div>
          <div className="work-row__main">
            <Group gap={7} wrap="nowrap" className="work-row__meta">
              {work.seriesTitle && <Text className="work-card__series line-clamp-1 work-row__series">{work.seriesTitle}</Text>}
            </Group>
            <UnstyledButton className="work-row__open" onClick={open} onKeyDown={keyboardOpen} role="link" aria-label={`${work.title}を開く`}>
              <Text fw={650} size="sm" className="line-clamp-1">{work.title}</Text>
            </UnstyledButton>
            <AuthorLine work={work} size={17} />
          </div>
          <div className="work-row__tags"><TagRow tags={work.tags} /></div>
          <Group gap="sm" wrap="nowrap" className="work-row__facts">
            <Text size="xs" c="dimmed">{formatDateNumeric(work.sourceCreatedAt)}</Text>
            <Text size="xs" c="dimmed">{formatNumber(work.textLength)}字</Text>
            {work.assetCount > 0 && <Text size="xs" c="dimmed"><Images size={12} />{work.assetCount}</Text>}
            <VersionChip work={work} />
          </Group>
          <div className="work-row__actions">{actions}</div>
        </div>
      </Card>
    );
  }

  return (
    <Card className="work-card surface--interactive" padding={0}>
      <div className="work-card__inner">
        {/* The source stamps the card itself, at its top-left corner over the
            cover, rather than living in a column of its own. */}
        <ProviderMark provider={work.source} className="work-card__provider" />
        <div className="work-card__cover-rail" onClick={(event) => { if (!(event.target as HTMLElement).closest("input")) open(); }}>
          <WorkCover work={work} variant="card" className="work-card__cover" />
          {selectionMode && <Checkbox className="work-card__select" checked={selected} onChange={(e) => onSelect?.(work.id, e.currentTarget.checked)} onClick={(e) => e.stopPropagation()} aria-label={`${work.title}を選択`} />}
        </div>
        <div className="work-card__body">
          <div className="work-card__meta">
            {work.seriesTitle && <Text className="work-card__series line-clamp-1">{work.seriesTitle}</Text>}
          </div>
          <UnstyledButton className="work-card__open" onClick={open} onKeyDown={keyboardOpen} role="link" aria-label={`${work.title}を開く`}>
            <Text fw={720} className="work-card__title line-clamp-2" lh={1.32}>{work.title}</Text>
          </UnstyledButton>
          <AuthorLine work={work} />
          {work.excerpt && <Text size="xs" c="dimmed" className="line-clamp-2 work-card__excerpt">{summaryText(work.excerpt)}</Text>}
          <TagRow tags={work.tags} />
          {/* One bottom line: the facts read left, the controls sit right, so
              the card ends on a single row instead of two half-empty ones. */}
          <div className="work-card__footer">
            <Group gap="sm" wrap="nowrap" className="work-card__facts">
              <Text size="xs" c="dimmed">{formatDateNumeric(work.sourceCreatedAt)}</Text>
              <Text size="xs" c="dimmed">{formatNumber(work.textLength)}字</Text>
              {work.assetCount > 0 && <Text size="xs" c="dimmed"><Images size={12} />{work.assetCount}</Text>}
              <VersionChip work={work} />
            </Group>
            {actions}
          </div>
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
  const refresh = () => Promise.all([
    queryClient.invalidateQueries({ queryKey: ["library"] }),
    queryClient.invalidateQueries({ queryKey: ["dashboard"] }),
    queryClient.invalidateQueries({ queryKey: ["reader-document", work.id] }),
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
      notifications.show({ color: next ? "teal" : "gray", message: next ? "更新監視をオンにしました" : "更新監視をオフにしました" });
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
        if (isTauriRuntime()) await deleteDownload(work.id);
        await Promise.all([
          queryClient.invalidateQueries({ queryKey: ["library"] }),
          queryClient.invalidateQueries({ queryKey: ["library-facets"] }),
          queryClient.invalidateQueries({ queryKey: ["dashboard"] }),
          queryClient.invalidateQueries({ queryKey: ["entity-works"] }),
        ]);
        notifications.show({ color: "green", message: "作品を削除しました" });
      } catch (error) {
        notifications.show({ color: "red", title: "削除できません", message: errorMessage(error) });
      }
    },
  });

  return (
    <Group gap={2} wrap="nowrap" justify="flex-end" className="work-card__actions" onClick={(event) => event.stopPropagation()}>
      <Tooltip label="読む">
        <ActionIcon variant="light" color="piep" size="sm" aria-label={`${work.title}を読む`} onClick={() => navigate(`/reader/${work.id}`)}>
          <BookOpen size={15} />
        </ActionIcon>
      </Tooltip>
      <Tooltip label={work.favorite ? "お気に入りを解除" : "お気に入りに追加"}>
        <ActionIcon variant={work.favorite ? "filled" : "subtle"} color="orange" size="sm" aria-label={work.favorite ? "お気に入りを解除" : "お気に入りに追加"} aria-pressed={work.favorite} onClick={toggleFavorite}>
          <Heart size={15} fill={work.favorite ? "currentColor" : "none"} />
        </ActionIcon>
      </Tooltip>
      <Tooltip label={work.watchUpdates ? "更新監視をオフにする" : "更新監視をオンにする"}>
        <ActionIcon variant={work.watchUpdates ? "filled" : "subtle"} color="teal" size="sm" aria-label={work.watchUpdates ? "更新監視をオフにする" : "更新監視をオンにする"} aria-pressed={work.watchUpdates} onClick={toggleWatch}>
          <RefreshCw size={15} />
        </ActionIcon>
      </Tooltip>
      {/* Green matches the "ep" half of the wordmark; the old purple belonged
          to nothing in the palette. */}
      <Tooltip label={queued ? "EPUBキューから外す" : "EPUBキューに追加"}>
        <ActionIcon variant={queued ? "filled" : "subtle"} color="leaf" size="sm" aria-label={queued ? "EPUBキューから外す" : "EPUBキューに追加"} aria-pressed={queued} onClick={onQueue}>
          {queued ? <BookCheck size={15} /> : <BookPlus size={15} />}
        </ActionIcon>
      </Tooltip>
      <Menu position="bottom-end" withinPortal>
        <Menu.Target><Tooltip label="その他"><ActionIcon variant="subtle" color="gray" size="sm" aria-label={`${work.title}のその他の操作`}><Ellipsis size={16} /></ActionIcon></Tooltip></Menu.Target>
        <Menu.Dropdown>
          <Menu.Item leftSection={<Library size={15} />} onClick={() => navigate(`/works/${work.id}`)}>詳細</Menu.Item>
          <Menu.Item leftSection={<RefreshCw size={15} />} onClick={() => navigate(`/updates?work=${work.id}`)}>更新センターで確認</Menu.Item>
          <Menu.Divider />
          <Menu.Item color="red" leftSection={<Trash2 size={15} />} onClick={confirmDelete}>ライブラリから削除</Menu.Item>
        </Menu.Dropdown>
      </Menu>
    </Group>
  );
}

export type { WorkCardProps };
