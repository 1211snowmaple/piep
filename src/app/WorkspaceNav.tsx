import { useEffect, useState, type ReactNode } from "react";
import { Badge, Divider, NavLink, ScrollArea, Stack, Text, Tooltip } from "@mantine/core";
import { useLocalStorage } from "@mantine/hooks";
import { useQuery } from "@tanstack/react-query";
import { Icons, IconSize, type LucideIcon } from "@/lib/icons";
import { ProviderGlyph } from "@/lib/providers";
import { useAppNavigate, useAppRouter } from "@/app/router";
import { useWorkspace } from "@/app/WorkspaceContext";
import { useOperationJobs } from "@/features/jobs/operationJobs";
import { readingWorkIds, subscribeReadingPositions } from "@/features/library/readingShelf";
import { formatCompactCount, formatNumber } from "@/lib/format";
import { isTauriRuntime } from "@/services/dbApi";
import { getLibraryShelfCounts, listSavedSearches } from "@/services/shelfApi";
import { activeShelf, LIBRARY_SHELVES, type LibraryShelf } from "@/app/libraryShelves";
import type { LibraryShelfCounts, SavedSearchRecord } from "@/types/library";

const SHELF_ICON: Record<LibraryShelf, LucideIcon> = {
  all: Icons.library,
  favorite: Icons.favorite,
  reading: Icons.read,
  // 更新センターと同じ記号にする。「取得元が直した」と「それを見つける場所」が
  // 別の絵だと、同じ話をしていることが伝わらない。
  revised: Icons.updates,
};

const PREVIEW_COUNTS: LibraryShelfCounts = { total: 1284, favorite: 84, watched: 7, reading: 12, revised: 3 };

/** Which groups are unfolded. Kept per install so the sidebar reopens as it was. */
type GroupId = "library" | "saved" | "collect" | "export";
const DEFAULT_OPEN: Record<GroupId, boolean> = { library: true, saved: true, collect: false, export: false };

interface RowProps {
  label: string;
  icon: LucideIcon;
  glyph?: ReactNode;
  active: boolean;
  railed: boolean;
  count?: number | null;
  onSelect: () => void;
}

function Row({ label, icon: Icon, glyph, active, railed, count, onSelect }: RowProps) {
  const link = (
    <NavLink
      component="button"
      type="button"
      className="app-nav__link"
      active={active}
      label={railed ? undefined : label}
      leftSection={glyph ?? <Icon size={IconSize.nav} strokeWidth={1.8} />}
      rightSection={!railed && typeof count === "number"
        ? <Text size="xs" c={active ? undefined : "dimmed"} className="app-nav__count">{formatCompactCount(count)}</Text>
        : undefined}
      onClick={onSelect}
      aria-label={railed ? label : undefined}
      aria-current={active ? "page" : undefined}
    />
  );
  return railed
    ? <Tooltip label={typeof count === "number" ? `${label}（${formatNumber(count)}）` : label} position="right" withArrow>{link}</Tooltip>
    : link;
}

/**
 * A group of destinations that can be folded away.
 *
 * Everything the app can do stays on the sidebar at once; a group only controls
 * how much of it is unfolded. A control that swapped the sidebar's contents
 * instead hid whole parts of the app behind a menu nobody had reason to open.
 */
function NavGroup({
  id, label, icon: Icon, open, onToggle, active, children,
}: {
  id: GroupId;
  label: string;
  icon: LucideIcon;
  open: boolean;
  onToggle: (id: GroupId, open: boolean) => void;
  active: boolean;
  children: ReactNode;
}) {
  return (
    <NavLink
      component="button"
      type="button"
      className="app-nav__group"
      label={label}
      leftSection={<Icon size={IconSize.nav} strokeWidth={1.8} />}
      childrenOffset={14}
      opened={open}
      aria-expanded={open}
      onChange={(next) => onToggle(id, next)}
      // The heading folds; it does not navigate. A heading that also moved the
      // reader would take them somewhere every time they tidied the sidebar.
      data-section-active={active || undefined}
    >
      {children}
    </NavLink>
  );
}

export function WorkspaceNav({ railed, onNavigate }: { railed: boolean; onNavigate?: () => void }) {
  const navigate = useAppNavigate();
  const location = useAppRouter();
  const runtime = isTauriRuntime();
  const { epubQueue } = useWorkspace();
  const [openGroups, setOpenGroups] = useLocalStorage<Record<GroupId, boolean>>({
    key: "piep.nav-groups.v1",
    defaultValue: DEFAULT_OPEN,
  });

  // Reading positions live in per-device storage, so their ids have to be read
  // here and resolved by the library, which alone knows what still exists.
  const [readingIds, setReadingIds] = useState(() => readingWorkIds());
  useEffect(() => subscribeReadingPositions(() => setReadingIds(readingWorkIds())), []);
  const counts = useQuery({
    queryKey: ["library-shelf-counts", readingIds],
    queryFn: () => runtime ? getLibraryShelfCounts(readingIds) : Promise.resolve(PREVIEW_COUNTS),
    staleTime: 30_000,
  });
  const savedSearches = useQuery({
    queryKey: ["saved-searches"],
    queryFn: () => runtime ? listSavedSearches() : Promise.resolve([] as SavedSearchRecord[]),
    staleTime: 60_000,
  });

  const go = (path: string) => {
    navigate(path);
    onNavigate?.();
  };
  const toggle = (id: GroupId, open: boolean) => setOpenGroups((current) => ({ ...(current ?? DEFAULT_OPEN), [id]: open }));
  const isOpen = (id: GroupId) => openGroups?.[id] ?? DEFAULT_OPEN[id];

  const shelf = activeShelf(location.pathname, location.searchParams);
  const activeSavedId = Number.parseInt(location.searchParams.get("saved") ?? "", 10);
  // Collections are a library tab now, so a collection screen keeps the
  // library section highlighted rather than needing an entry of its own.
  const inLibrary = location.pathname === "/library"
    || location.pathname.startsWith("/works/")
    || location.pathname.startsWith("/collections");
  const inCollect = location.pathname.startsWith("/save") || location.pathname.startsWith("/updates");
  const inExport = location.pathname.startsWith("/epub");
  const saved = savedSearches.data ?? [];
  const shelfCount = (value: LibraryShelf): number | null => {
    const data = counts.data;
    if (!data) return null;
    if (value === "all") return data.total;
    if (value === "favorite") return data.favorite;
    if (value === "reading") return data.reading;
    return data.revised;
  };

  const shelfRows = LIBRARY_SHELVES.map((item) => (
    <Row
      key={item.value}
      label={item.label}
      icon={SHELF_ICON[item.value]}
      active={shelf === item.value}
      railed={railed}
      count={shelfCount(item.value)}
      onSelect={() => go(item.search ? `/library?${item.search}` : "/library")}
    />
  ));
  const savedRows = saved.map((item) => (
    <Row
      key={item.id}
      label={item.name}
      icon={Icons.savedSearchItem}
      // In the rail a name cannot be shown, so each saved search takes its own
      // first character; otherwise they would be identical bookmark icons.
      glyph={railed ? <SavedSearchMark name={item.name} /> : undefined}
      active={activeSavedId === item.id}
      railed={railed}
      onSelect={() => go(`/library?saved=${item.id}`)}
    />
  ));
  const collectRows = [
    <Row key="pixiv" label="pixivから保存" icon={Icons.collect} glyph={<ProviderGlyph provider="pixiv" />} active={location.pathname === "/save/pixiv"} railed={railed} onSelect={() => go("/save/pixiv")} />,
    <Row key="fanbox" label="FANBOXから保存" icon={Icons.collect} glyph={<ProviderGlyph provider="fanbox" />} active={location.pathname === "/save/fanbox"} railed={railed} onSelect={() => go("/save/fanbox")} />,
    <Row key="updates" label="更新を確認" icon={Icons.updates} active={location.pathname.startsWith("/updates")} railed={railed} onSelect={() => go("/updates")} />,
  ];
  const exportRows = [
    <Row key="epub" label="EPUBキュー" icon={Icons.epub} active={location.pathname === "/epub"} railed={railed} count={epubQueue.length || null} onSelect={() => go("/epub")} />,
    <Row key="templates" label="テンプレートスタジオ" icon={Icons.epubTemplate} active={location.pathname.startsWith("/epub/templates")} railed={railed} onSelect={() => go("/epub/templates")} />,
  ];

  return (
    <ScrollArea flex={1} px={railed ? 6 : "xs"} py="xs" type="auto">
      <Stack gap={2}>
        <Row label="ホーム" icon={Icons.home} active={location.pathname === "/"} railed={railed} onSelect={() => go("/")} />

        {/* The rail has no room for headings, so everything is one flat column
            of icons there, divided rather than folded. */}
        {railed ? (
          <>
            <Divider my={5} />
            {shelfRows}
            {savedRows.length > 0 && <Divider my={5} />}
            {savedRows}
            <Divider my={5} />
            {collectRows}
            {exportRows}
          </>
        ) : (
          <>
            <NavGroup id="library" label="ライブラリ" icon={Icons.library} open={isOpen("library")} onToggle={toggle} active={inLibrary}>
              {shelfRows}
            </NavGroup>
            <NavGroup id="saved" label="保存した検索" icon={Icons.savedSearch} open={isOpen("saved")} onToggle={toggle} active={Number.isSafeInteger(activeSavedId)}>
              {saved.length === 0
                ? <Text px="sm" size="xs" c="dimmed" py={4}>検索条件を保存すると、ここから開けます。</Text>
                : savedRows}
            </NavGroup>
            <NavGroup id="collect" label="保存" icon={Icons.collect} open={isOpen("collect")} onToggle={toggle} active={inCollect}>
              {collectRows}
            </NavGroup>
            <NavGroup id="export" label="書き出し" icon={Icons.epub} open={isOpen("export")} onToggle={toggle} active={inExport}>
              {exportRows}
            </NavGroup>
          </>
        )}
      </Stack>
    </ScrollArea>
  );
}

/** A saved search's mark in the collapsed rail: its own first character. */
function SavedSearchMark({ name }: { name: string }) {
  return <span className="saved-search-mark" aria-hidden>{[...name.trim()][0] ?? "?"}</span>;
}

/** Always reachable, whichever part of the sidebar is unfolded. */
export function WorkspaceNavFooter({ railed, onNavigate }: { railed: boolean; onNavigate?: () => void }) {
  const navigate = useAppNavigate();
  const location = useAppRouter();
  const operationJobs = useOperationJobs();
  const active = operationJobs.filter((job) => ["queued", "running", "canceling"].includes(job.status)).length;
  const go = (path: string) => {
    navigate(path);
    onNavigate?.();
  };
  const isPath = (path: string) => location.pathname === path || location.pathname.startsWith(`${path}/`);
  return (
    <Stack gap={2} p={railed ? 6 : "xs"}>
      <Divider mb={4} />
      <NavLink
        component="button"
        type="button"
        className="app-nav__link"
        active={isPath("/operations")}
        label={railed ? undefined : "アクティビティ"}
        leftSection={<Icons.activity size={IconSize.nav} strokeWidth={1.8} />}
        rightSection={!railed && active > 0 ? <Badge size="xs" variant="filled">{formatCompactCount(active)}</Badge> : undefined}
        aria-label={railed ? `アクティビティ${active > 0 ? `、実行中${active}件` : ""}` : undefined}
        aria-current={isPath("/operations") ? "page" : undefined}
        onClick={() => go("/operations")}
      />
      <Row label="設定" icon={Icons.settings} active={isPath("/settings")} railed={railed} onSelect={() => go("/settings")} />
    </Stack>
  );
}
