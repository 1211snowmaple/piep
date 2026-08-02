import { memo, useCallback, useRef, useState } from "react";
import { ActionIcon, Badge, Box, Card, Checkbox, Group, Menu, Popover, Stack, Text, Tooltip, UnstyledButton } from "@mantine/core";
import { modals } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { useQueryClient } from "@tanstack/react-query";
import { BookCheck, BookOpen, BookPlus, CalendarDays, Ellipsis, FileText, Heart, Images, Library, RefreshCw, Trash2 } from "lucide-react";
import { useAppNavigate } from "@/app/router";
import { useWorkspace } from "@/app/WorkspaceContext";
import { WorkCover } from "@/components/WorkCover";
import { ProviderMark } from "@/lib/providers";
import { contentTypeLabel, errorMessage, formatDate, formatNumber } from "@/lib/format";
import { summaryText } from "@/lib/content";
import { deleteDownload, isTauriRuntime, setFavorite } from "@/services/dbApi";
import type { DownloadEntry } from "@/types/library";

export const WorkCard = memo(function WorkCard({
  work,
  selected,
  selectionMode = false,
  onSelect,
  onToggleFavorite,
  compact = false,
}: {
  work: DownloadEntry;
  selected?: boolean;
  selectionMode?: boolean;
  onSelect?: (selected: boolean) => void;
  onToggleFavorite?: (favorite: boolean) => void;
  compact?: boolean;
}) {
  const navigate = useAppNavigate();
  const { addToEpubQueue, removeFromEpubQueue, isQueuedForEpub } = useWorkspace();
  const queued = isQueuedForEpub(work.id);
  const open = useCallback(() => {
    if (selectionMode) onSelect?.(!selected);
    else navigate(`/works/${work.id}`);
  }, [navigate, onSelect, selected, selectionMode, work.id]);
  const keyboardOpen = (event: React.KeyboardEvent) => {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      open();
    }
  };

  if (compact) {
    return (
      <Card className="work-row surface--interactive" p="sm">
        <Group wrap="nowrap" gap="md">
          {selectionMode && <Checkbox checked={selected} onChange={(e) => onSelect?.(e.currentTarget.checked)} onClick={(e) => e.stopPropagation()} aria-label={`${work.title}を選択`} />}
          <WorkCover work={work} variant="compact" />
          <UnstyledButton className="work-row__open" onClick={open} onKeyDown={keyboardOpen} role="link" aria-label={`${work.title}を開く`}>
            <Stack gap={3} flex={1} miw={0}>
              <Group gap="xs" wrap="nowrap"><ProviderMark provider={work.source} compact /><Text fw={650} size="sm" className="line-clamp-1">{work.title}</Text></Group>
              <Text size="xs" c="dimmed" className="line-clamp-1">{work.authorName} · {contentTypeLabel(work.contentType)} · {formatNumber(work.textLength)}字</Text>
            </Stack>
          </UnstyledButton>
          <Text size="xs" c="dimmed" visibleFrom="sm">{formatDate(work.downloadedAt)}</Text>
          <WorkActions work={work} queued={queued} onQueue={() => queued ? removeFromEpubQueue(work.id) : addToEpubQueue(work.id)} onToggleFavorite={onToggleFavorite} />
        </Group>
      </Card>
    );
  }

  return (
    <Card className="work-card surface--interactive" padding={0}>
      <Group wrap="nowrap" align="stretch" gap={0} h="100%">
        <Box className="work-card__cover-rail" onClick={(event) => { if (!(event.target as HTMLElement).closest("input")) open(); }}>
          <WorkCover work={work} variant="card" className="work-card__cover" />
          <Box className="work-card__provider"><ProviderMark provider={work.source} /></Box>
          {selectionMode && <Checkbox className="work-card__select" checked={selected} onChange={(e) => onSelect?.(e.currentTarget.checked)} onClick={(e) => e.stopPropagation()} aria-label={`${work.title}を選択`} />}
        </Box>
        <Stack gap="xs" p="md" flex={1} miw={0}>
          <UnstyledButton className="work-card__open" onClick={open} onKeyDown={keyboardOpen} role="link" aria-label={`${work.title}を開く`}>
            <Stack gap={3} flex={1}>
              {work.seriesTitle && <Text className="work-card__series line-clamp-1">{work.seriesTitle}</Text>}
              <Text fw={720} className="work-card__title line-clamp-2" lh={1.32}>{work.title}</Text>
              <Text size="sm" c="dimmed" className="line-clamp-1">{work.authorName}</Text>
              {work.excerpt && <Text size="xs" c="dimmed" className="line-clamp-2" mt={3}>{summaryText(work.excerpt)}</Text>}
            </Stack>
          </UnstyledButton>
          <Group gap={5} className="work-card__tags">
            {work.tags.slice(0, 3).map((tag) => <Badge key={tag} variant="light" color="gray" size="xs">{tag}</Badge>)}
            {work.tags.length > 3 && <TagOverflow tags={work.tags.slice(3)} />}
          </Group>
          <Group gap="sm" wrap="nowrap" className="work-card__facts">
            <Text size="xs" c="dimmed"><CalendarDays size={12} />{formatDate(work.sourceCreatedAt)}</Text>
            <Text size="xs" c="dimmed"><FileText size={12} />{formatNumber(work.textLength)}字</Text>
            {work.assetCount > 0 && <Text size="xs" c="dimmed"><Images size={12} />{work.assetCount}</Text>}
          </Group>
          <Group justify="space-between" wrap="nowrap">
            <Text size="10px" c="dimmed">v{work.currentVersion}</Text>
            <WorkActions work={work} queued={queued} onQueue={() => queued ? removeFromEpubQueue(work.id) : addToEpubQueue(work.id)} onToggleFavorite={onToggleFavorite} />
          </Group>
        </Stack>
      </Group>
    </Card>
  );
});

function TagOverflow({ tags }: { tags: string[] }) {
  const navigate = useAppNavigate();
  const [opened, setOpened] = useState(false);
  const [pinned, setPinned] = useState(false);
  const closeTimer = useRef<number | null>(null);
  const open = () => {
    if (closeTimer.current !== null) window.clearTimeout(closeTimer.current);
    setOpened(true);
  };
  const closeSoon = () => {
    if (pinned) return;
    if (closeTimer.current !== null) window.clearTimeout(closeTimer.current);
    closeTimer.current = window.setTimeout(() => setOpened(false), 260);
  };
  const openTag = (tag: string) => {
    setOpened(false);
    setPinned(false);
    navigate(`/library?q=${encodeURIComponent(`tag:${tag}`)}`);
  };

  return (
    <Popover opened={opened} onChange={(value) => { setOpened(value); if (!value) setPinned(false); }} position="top-start" withArrow shadow="md" withinPortal>
      <Popover.Target>
        <Badge
          component="button"
          type="button"
          className="work-card__tag-overflow"
          variant="outline"
          color="piep"
          size="xs"
          aria-label={`残りのタグ ${tags.join("、")}`}
          aria-expanded={opened}
          onClick={(event) => { event.stopPropagation(); setPinned((value) => { const next = !value; setOpened(next); return next; }); }}
          onPointerDown={(event) => event.stopPropagation()}
          onMouseEnter={open}
          onMouseLeave={closeSoon}
          onFocus={open}
          onBlur={closeSoon}
          onKeyDown={(event) => event.stopPropagation()}
        >+{tags.length}</Badge>
      </Popover.Target>
      <Popover.Dropdown className="work-card__tag-popover" onClick={(event) => event.stopPropagation()} onMouseEnter={open} onMouseLeave={closeSoon}>
        <Text size="xs" c="dimmed" mb={7}>タグで絞り込む</Text>
        <Group gap={6} maw={320}>
          {tags.map((tag) => <Badge component="button" type="button" key={tag} size="sm" variant="light" color="piep" onClick={() => openTag(tag)}>#{tag}</Badge>)}
        </Group>
      </Popover.Dropdown>
    </Popover>
  );
}

function WorkActions({ work, queued, onQueue, onToggleFavorite }: { work: DownloadEntry; queued: boolean; onQueue: () => void; onToggleFavorite?: (favorite: boolean) => void }) {
  const navigate = useAppNavigate();
  const queryClient = useQueryClient();
  const toggleFavorite = async () => {
    const next = !work.favorite;
    if (onToggleFavorite) return onToggleFavorite(next);
    try {
      if (isTauriRuntime()) await setFavorite(work.id, next);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["library"] }),
        queryClient.invalidateQueries({ queryKey: ["dashboard"] }),
        queryClient.invalidateQueries({ queryKey: ["reader-document", work.id] }),
      ]);
    } catch (error) {
      notifications.show({ color: "red", title: "お気に入りを変更できません", message: errorMessage(error) });
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
    <Group gap={2} wrap="nowrap" className="work-card__actions" onClick={(event) => event.stopPropagation()}>
      <Tooltip label="読む">
        <ActionIcon variant="subtle" color="piep" size="sm" aria-label={`${work.title}を読む`} onClick={() => navigate(`/reader/${work.id}`)}>
          <BookOpen size={15} />
        </ActionIcon>
      </Tooltip>
      <Tooltip label={work.favorite ? "お気に入りを解除" : "お気に入りに追加"}>
        <ActionIcon variant={work.favorite ? "filled" : "subtle"} color="orange" size="sm" aria-label={work.favorite ? "お気に入りを解除" : "お気に入りに追加"} onClick={toggleFavorite}>
          <Heart size={15} fill={work.favorite ? "currentColor" : "none"} />
        </ActionIcon>
      </Tooltip>
      <Tooltip label={work.watchUpdates ? "更新状況を確認" : "更新監視はオフです"}>
        <ActionIcon variant="subtle" color="teal" size="sm" disabled={!work.watchUpdates} aria-label="更新状況を確認" onClick={() => navigate(`/updates?work=${work.id}`)}>
          <RefreshCw size={15} />
        </ActionIcon>
      </Tooltip>
      <Tooltip label={queued ? "EPUBキューから外す" : "EPUBキューに追加"}>
        <ActionIcon variant={queued ? "filled" : "subtle"} color="grape" size="sm" aria-label={queued ? "EPUBキューから外す" : "EPUBキューに追加"} onClick={onQueue}>
          {queued ? <BookCheck size={15} /> : <BookPlus size={15} />}
        </ActionIcon>
      </Tooltip>
      <Menu position="bottom-end" withinPortal>
        <Menu.Target><Tooltip label="その他"><ActionIcon variant="subtle" color="gray" size="sm" aria-label={`${work.title}のその他の操作`}><Ellipsis size={16} /></ActionIcon></Tooltip></Menu.Target>
        <Menu.Dropdown>
          <Menu.Item leftSection={<Library size={15} />} onClick={() => navigate(`/works/${work.id}`)}>詳細</Menu.Item>
          <Menu.Divider />
          <Menu.Item color="red" leftSection={<Trash2 size={15} />} onClick={confirmDelete}>ライブラリから削除</Menu.Item>
        </Menu.Dropdown>
      </Menu>
    </Group>
  );
}
