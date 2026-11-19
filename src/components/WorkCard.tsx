import { memo, useCallback } from "react";
import { ActionIcon, Avatar, Badge, Card, Group, Menu, Text, Tooltip, UnstyledButton } from "@mantine/core";
import { modals } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { EpubQueueGlyph } from "@/components/EpubQueueGlyph";
import { Icons, IconSize } from "@/lib/icons";
import { useAppNavigate } from "@/app/router";
import { useWorkspace } from "@/app/WorkspaceContext";
import { ClippedTooltip } from "@/components/ClippedTooltip";
import { TagRow } from "@/components/TagRow";
import { WorkCover } from "@/components/WorkCover";
import { ProviderMark } from "@/lib/providers";
import { errorMessage, formatDateNumeric, formatNumber } from "@/lib/format";
import { summaryText } from "@/lib/content";
import { deleteDownload, getAssetUrl, isTauriRuntime, setFavorite, setWatchUpdates } from "@/services/dbApi";
import { deleteThenCleanup } from "@/features/library/deletedWorkCleanup";
import { invalidateWorkFlagViews } from "@/features/library/workSetInvalidation";
import { listPendingRevisionsCommand } from "@/services/updateJobApi";
import type { DownloadEntry, ListedWork, ShelfEntry } from "@/types/library";
import { invalidateShelf, isShelfEntry, useShelfWritable } from "@/features/library/shelfV2";
import { libraryV2RemoveWork, libraryV2SetUserState } from "@/services/libraryV2Api";
import { DANGER_CONFIRM } from "@/components/DangerButton";

interface WorkCardProps {
  work: ListedWork;
  /** Lists that already loaded the revision set pass the membership result so
   * hundreds of cards do not each subscribe to and scan the same query. */
  revised?: boolean;
  selected?: boolean;
  selectionMode?: boolean;
  /** Receives the work id so lists can pass one stable handler for every card. */
  onSelect?: (id: number, selected: boolean) => void;
  /** 一枚ごとの選択の受け口。新しい棚の作品（番号を持たない）も選べる。あれば `onSelect` より先。 */
  onToggleSelect?: (selected: boolean) => void;
  /** 新しい棚の作品の選択。一覧が全カードに同じ受け口を渡せるよう、作品ごと返す。 */
  onSelectShelf?: (work: ShelfEntry, selected: boolean) => void;
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
function VersionChip({ work, interactive = true }: { work: ListedWork; interactive?: boolean }) {
  const navigate = useAppNavigate();
  if (work.currentVersion <= 1) return null;
  return (
    <Tooltip label={`手元に${work.currentVersion}版保存されています。保存済み履歴を開く`}>
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
        履歴 {work.currentVersion}版
      </Badge>
    </Tooltip>
  );
}

/** A provider rewrite that has been found but is not one of the saved versions. */
function PendingRevisionChip({ work, visible, interactive = true }: { work: ListedWork; visible: boolean; interactive?: boolean }) {
  const navigate = useAppNavigate();
  if (!visible) return null;
  return (
    <Tooltip label="取得元に未保存の改稿があります。保存せずに差分を確認">
      <Badge
        component="button"
        type="button"
        size="xs"
        variant="light"
        color="yellow"
        aria-label={`${work.title}の未保存の改稿を比較`}
        aria-hidden={interactive ? undefined : true}
        tabIndex={interactive ? undefined : -1}
        disabled={!interactive}
        onClick={(event) => {
          event.stopPropagation();
          navigate(`/works/${work.id}?tab=history&compare=pending`);
        }}
      >
        未保存の改稿
      </Badge>
    </Tooltip>
  );
}

/** The series above the title doubles as the link to it. */
function SeriesLink({ work, className, interactive = true }: { work: ListedWork; className?: string; interactive?: boolean }) {
  const navigate = useAppNavigate();
  if (!work.seriesTitle) return null;
  const key = work.seriesId;
  const label = <Text className={["work-card__series identity-name", className].filter(Boolean).join(" ")}>{work.seriesTitle}</Text>;
  const content = !key ? label : (
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
  return (
    <ClippedTooltip label={work.seriesTitle} disabled={!interactive}>
      {content}
    </ClippedTooltip>
  );
}

/**
 * 作者名は、行の中でいちばん最後に譲る。
 *
 * あらすじは地の文なので端が切れても読めるが、名前は人を名指す語で、
 * 3文字に切られたら誰のことか分からない。それでも入りきらない幅では
 * 切るしかないので、切れたぶんはホバーで読めるようにしておく。
 */
function AuthorLine({ work, size = 20, interactive = true }: { work: ListedWork; size?: number; interactive?: boolean }) {
  const navigate = useAppNavigate();
  const name = work.personName || work.authorName;
  const icon = getAssetUrl(work.personIconPath);
  const key = work.personId || work.authorId;
  return (
    <ClippedTooltip label={name} disabled={!interactive}>
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
        <Text size="xs" c="dimmed" className="identity-name">{name}</Text>
      </Group>
    </ClippedTooltip>
  );
}

const MATCH_FIELD_LABELS: Record<string, string> = {
  title: "タイトル",
  author_name: "作者",
  tags: "タグ",
  series_title: "シリーズ",
  excerpt: "概要",
  body: "本文",
  content: "本文の傾向",
  metadata: "作品情報",
};

function SearchMatchReason({ work }: { work: ListedWork }) {
  const navigate = useAppNavigate();
  const fields = (work.matchFields ?? []).slice(0, 3);
  const historicalVersion = work.matchedVersion ?? null;
  if (!fields.length && historicalVersion === null) return null;
  const semantic = work.scoreReasons?.some((reason) => reason.matchType === "semantic") ?? false;
  const reasonDetails = (work.scoreReasons ?? [])
    .slice(0, 5)
    .map((reason) => reason.matchType === "semantic"
      ? `${MATCH_FIELD_LABELS[reason.field] ?? reason.field}: 作品単位で意味が近い候補`
      : `${MATCH_FIELD_LABELS[reason.field] ?? reason.field}: ${reason.term}（${reason.matchType}）`)
    .join("\n");
  const historyDetails = historicalVersion === null
    ? ""
    : `現在版ではなく保存済みの v${historicalVersion} に一致しました${(work.historicalMatchCount ?? 0) > 1 ? `（一致した過去版: ${work.historicalMatchCount}版）` : ""}`;
  const details = [historyDetails, reasonDetails].filter(Boolean).join("\n");
  return (
    <Tooltip label={details || (semantic ? "意味が近い候補です" : "検索語が一致した項目です")} multiline maw={360} withArrow>
      <Group gap={4} wrap="nowrap" className="work-card__search-reason" aria-label={historicalVersion === null ? `${semantic ? "類似理由" : "一致理由"}: ${fields.map((field) => MATCH_FIELD_LABELS[field] ?? field).join("、")}` : `過去版 v${historicalVersion} に一致`}>
        <Icons.searchMatch size={IconSize.inline} aria-hidden />
        <Text size="xs" c="piep" fw={650}>{semantic ? "類似" : "一致"}</Text>
        {historicalVersion !== null && (
          <Badge
            component="button"
            type="button"
            size="xs"
            variant="filled"
            color="piep"
            aria-label={`${work.title}の過去版 v${historicalVersion} を読む`}
            onClick={(event) => {
              event.stopPropagation();
              navigate(`/reader/${work.id}?version=${historicalVersion}`);
            }}
          >
            過去版 v{historicalVersion}
          </Badge>
        )}
        {fields.map((field) => <Badge key={field} size="xs" variant="light" color="piep">{MATCH_FIELD_LABELS[field] ?? field}</Badge>)}
      </Group>
    </Tooltip>
  );
}

function SelectionToggle({ work, selected, compact, onSelect }: {
  work: ListedWork;
  selected: boolean;
  compact?: boolean;
  onSelect: (selected: boolean) => void;
}) {
  // 一覧（リスト表示）の行は、行そのものが細いので隅の印のまま。
  // 表紙が主役のカードだけ、中央の大きな丸に変える。
  if (compact) {
    return (
      <UnstyledButton
        className="work-row__select"
        data-checked={selected || undefined}
        aria-label={`${work.title}を${selected ? "選択解除" : "選択"}`}
        aria-pressed={selected}
        onClick={(event) => { event.stopPropagation(); onSelect(!selected); }}
      >
        <span className="work-selection-toggle__indicator" aria-hidden>{selected && <Icons.confirm size={IconSize.menu} strokeWidth={3} />}</span>
      </UnstyledButton>
    );
  }
  return (
    <span className="card-select" data-checked={selected || undefined} aria-hidden>
      <span className="card-select__ring">
        <Icons.confirm size={IconSize.nav} strokeWidth={3} />
      </span>
    </span>
  );
}

export const WorkCard = memo(function WorkCard({
  work,
  revised: revisedFromList,
  selected,
  selectionMode = false,
  onSelect,
  onToggleSelect,
  onSelectShelf,
  onToggleFavorite,
  onToggleWatch,
  compact = false,
}: WorkCardProps) {
  const navigate = useAppNavigate();
  const { addToEpubQueue, removeFromEpubQueue, isQueuedForEpub } = useWorkspace();
  // 新しい棚の作品は旧棚の番号を持たない。番号で動く操作（選択・EPUB・印の照合）には渡さない。
  const legacyId = isShelfEntry(work) ? null : work.id;
  const queued = legacyId !== null && isQueuedForEpub(legacyId);
  // 印の出どころをカードの中に置くのは、カードが出るところすべてで同じ顔に
  // するため。同じキーの問い合わせは共有されるので、何枚並んでも通信は1回。
  const pendingRevisions = useQuery({
    queryKey: ["pending-revisions"],
    queryFn: () => isTauriRuntime() ? listPendingRevisionsCommand() : Promise.resolve([]),
    // 無効化は更新確認の側から届く（invalidateAfterUpdateJob）。
    // 隣の棚件数と同じ間隔にして、画面ごとに作法を変えない。
    staleTime: 30_000,
    enabled: revisedFromList === undefined,
  });
  const revised = revisedFromList
    ?? (pendingRevisions.data ?? []).some((entry) => entry.work === String(work.id));
  const excerpt = work.excerpt ? summaryText(work.excerpt) : "";
  const select = useCallback((next: boolean) => {
    if (onToggleSelect) onToggleSelect(next);
    else if (legacyId !== null) onSelect?.(legacyId, next);
    else if (isShelfEntry(work)) onSelectShelf?.(work, next);
  }, [legacyId, onSelect, onSelectShelf, onToggleSelect, work]);
  const open = useCallback(() => {
    if (selectionMode) select(!selected);
    else navigate(`/works/${work.id}`);
  }, [navigate, select, selected, selectionMode, work.id]);
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
    if (!selectionMode || (event.target instanceof Element && event.target.closest(".work-row__select"))) return;
    event.preventDefault();
    event.stopPropagation();
    select(!selected);
  };
  const selectFromKeyboardCapture = (event: React.KeyboardEvent<HTMLElement>) => {
    if (!selectionMode || (event.key !== "Enter" && event.key !== " ") || (event.target instanceof Element && event.target.closest(".work-row__select"))) return;
    event.preventDefault();
    event.stopPropagation();
    select(!selected);
  };
  const actions = (
    <WorkActions
      work={work}
      queued={queued}
      onQueue={() => {
        if (legacyId === null) return;
        if (queued) removeFromEpubQueue(legacyId);
        else addToEpubQueue(legacyId);
      }}
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
            {selectionMode && <SelectionToggle work={work} selected={Boolean(selected)} compact onSelect={select} />}
          </div>
          <div className="work-row__main" inert={selectionMode || undefined} aria-hidden={selectionMode || undefined}>
            <SeriesLink work={work} className="work-row__series" interactive={!selectionMode} />
            <ClippedTooltip label={work.title}>
              <UnstyledButton className="work-row__open" onClick={open} onKeyDown={keyboardOpen} role={selectionMode ? undefined : "link"} aria-label={selectionMode ? undefined : `${work.title}を開く`} aria-hidden={selectionMode || undefined} tabIndex={selectionMode ? -1 : undefined}>
                <Text fw={650} size="sm" className="line-clamp-2">{work.title}</Text>
              </UnstyledButton>
            </ClippedTooltip>
            <div className="work-row__identity">
              <AuthorLine work={work} size={17} interactive={!selectionMode} />
              <span className="work-card__identity-divider" aria-hidden />
              <ProviderMark provider={work.source} compact className="work-row__provider" />
              {work.excerpt && (
                <>
                  <span className="work-card__identity-divider work-row__excerpt-divider" aria-hidden />
                  {/* 題名の下に1行占めさせず、作者の横の余りを受ける。窓が狭まれば
                      行を増やさずここが先に縮む。切れた先は題名と同じくホバーで読める。 */}
                  <ClippedTooltip label={excerpt}>
                    <Text size="xs" c="dimmed" className="work-row__excerpt">{excerpt}</Text>
                  </ClippedTooltip>
                </>
              )}
            </div>
            <SearchMatchReason work={work} />
            <div className="work-row__tags" inert={selectionMode || undefined} aria-hidden={selectionMode || undefined}><TagRow tags={work.tags} interactive={!selectionMode} /></div>
          </div>
          {/* 最後の行はカードの足元と同じ組み。事実が左、操作が右。 */}
          <div className="work-row__footer">
            <Group gap="sm" wrap="nowrap" className="work-row__facts" inert={selectionMode || undefined} aria-hidden={selectionMode || undefined}>
              <Text size="xs" c="dimmed"><Icons.publishedDate size={IconSize.inline} />{formatDateNumeric(work.sourceCreatedAt)}</Text>
              <Text size="xs" c="dimmed"><Icons.textLength size={IconSize.inline} />{formatNumber(work.textLength)}字</Text>
              {work.assetCount > 0 && <Text size="xs" c="dimmed"><Icons.assets size={IconSize.inline} />{work.assetCount}</Text>}
              <PendingRevisionChip work={work} visible={revised} interactive={!selectionMode} />
              <VersionChip work={work} interactive={!selectionMode} />
            </Group>
            {!selectionMode && <div className="work-row__actions">{actions}</div>}
          </div>
        </div>
      </Card>
    );
  }

  // 選択モードでは、カードそのものが選ぶための操作子になる。中央の印は
  // 見た目だけなので、名前も押し込みの状態もカードが持つ - そうしないと
  // キーボードで選べる場所がどこにも無くなる。
  return (
    <Card
      className="work-card surface--interactive"
      padding={0}
      onClick={openFromCardSurface}
      onClickCapture={selectFromCardCapture}
      onKeyDownCapture={selectFromKeyboardCapture}
      data-selection-mode={selectionMode || undefined}
      data-selected={selectionMode && selected || undefined}
      role={selectionMode ? "button" : undefined}
      tabIndex={selectionMode ? 0 : undefined}
      aria-pressed={selectionMode ? Boolean(selected) : undefined}
      aria-label={selectionMode ? `${work.title}を${selected ? "選択解除" : "選択"}` : undefined}
    >
      <div className="work-card__inner">
        <div className="work-card__cover-rail">
          <WorkCover work={work} variant="card" className="work-card__cover" />
        </div>
        <div className="work-card__body" inert={selectionMode || undefined} aria-hidden={selectionMode || undefined}>
          {work.seriesTitle && <div className="work-card__meta"><SeriesLink work={work} interactive={!selectionMode} /></div>}
          <ClippedTooltip label={work.title}>
            <UnstyledButton className="work-card__open" onClick={open} onKeyDown={keyboardOpen} role={selectionMode ? undefined : "link"} aria-label={selectionMode ? undefined : `${work.title}を開く`} aria-hidden={selectionMode || undefined} tabIndex={selectionMode ? -1 : undefined}>
              <Text fw={720} className="work-card__title line-clamp-2" lh={1.32}>{work.title}</Text>
            </UnstyledButton>
          </ClippedTooltip>
          <div className="work-card__identity">
            <AuthorLine work={work} interactive={!selectionMode} />
            <span className="work-card__identity-divider" aria-hidden />
            <ProviderMark provider={work.source} compact className="work-card__provider" />
          </div>
          {work.excerpt && (
            <ClippedTooltip label={excerpt} disabled={selectionMode}>
              <Text size="xs" c="dimmed" className="line-clamp-2 work-card__excerpt">{excerpt}</Text>
            </ClippedTooltip>
          )}
          <SearchMatchReason work={work} />
          {work.tags.length > 0 && <div className="work-card__tagslot"><TagRow tags={work.tags} interactive={!selectionMode} /></div>}
        </div>
        <div className="work-card__footer" inert={selectionMode || undefined} aria-hidden={selectionMode || undefined}>
          <Group gap="sm" wrap="nowrap" className="work-card__facts">
            <Text size="xs" c="dimmed"><Icons.publishedDate size={IconSize.inline} />{formatDateNumeric(work.sourceCreatedAt)}</Text>
            <Text size="xs" c="dimmed"><Icons.textLength size={IconSize.inline} />{formatNumber(work.textLength)}字</Text>
            {work.assetCount > 0 && <Text size="xs" c="dimmed"><Icons.assets size={IconSize.inline} />{work.assetCount}</Text>}
            <PendingRevisionChip work={work} visible={revised} interactive={!selectionMode} />
            <VersionChip work={work} interactive={!selectionMode} />
          </Group>
          {!selectionMode && actions}
        </div>
        {/* カード全体を覆う。表紙の上だけに置くと、印がカードの中心ではなく
            表紙の中心に来て、横長のカードでは左に寄って見える。 */}
        {selectionMode && <SelectionToggle work={work} selected={Boolean(selected)} onSelect={select} />}
      </div>
    </Card>
  );
});

interface WorkActionsProps {
  work: ListedWork;
  queued: boolean;
  onQueue: () => void;
  onToggleFavorite?: (id: number, favorite: boolean) => void;
  onToggleWatch?: (id: number, watch: boolean) => void;
}

function WorkActions(props: WorkActionsProps) {
  const { work } = props;
  return isShelfEntry(work) ? <ShelfWorkActions work={work} /> : <LegacyWorkActions {...props} work={work} />;
}

/**
 * 新しい棚の作品の操作。棚を正本に切り替えるまでは旧棚から移した写しなので、書き換える操作
 * （お気に入り・更新監視・棚から外す）は押せない形で同じ場所に置く（並びと幅を変えない）。
 * EPUB のキューは、まだ新しい棚につないでいない。
 */
function ShelfWorkActions({ work }: { work: ShelfEntry }) {
  const navigate = useAppNavigate();
  const queryClient = useQueryClient();
  const writable = useShelfWritable(isTauriRuntime());
  const off = { "--ai-color": "var(--flag-off)" } as React.CSSProperties;
  const change = async (next: { favorite?: boolean; watching?: boolean }, failure: string) => {
    try {
      await libraryV2SetUserState(work.id, next);
      await invalidateShelf(queryClient);
      return true;
    } catch (error) {
      notifications.show({ color: "red", title: failure, message: errorMessage(error) });
      return false;
    }
  };
  const toggleFavorite = () => change({ favorite: !work.favorite }, "お気に入りを変更できません");
  const toggleWatch = async () => {
    const next = !work.watchUpdates;
    if (await change({ watching: next }, "更新監視を変更できません")) {
      notifications.show({ color: next ? "piep" : "gray", message: next ? "更新監視をオンにしました" : "更新監視をオフにしました" });
    }
  };
  const confirmRemove = () => modals.openConfirmModal({
    title: "この作品を棚から外しますか？",
    children: <Text size="sm">「{work.title}」を一覧から外します。保存した版・編集・コレクションの記録は残り、もう一度保存すると戻ります。</Text>,
    labels: { confirm: "外す", cancel: "キャンセル" },
    confirmProps: DANGER_CONFIRM,
    onConfirm: async () => {
      try {
        await libraryV2RemoveWork(work.id);
        await invalidateShelf(queryClient);
        notifications.show({ color: "green", message: "作品を棚から外しました" });
      } catch (error) {
        notifications.show({ color: "red", title: "外せません", message: errorMessage(error) });
      }
    },
  });
  const favoriteLabel = work.favorite ? "お気に入りを解除" : "お気に入りに追加";
  const watchLabel = work.watchUpdates ? "更新監視をオフにする" : "更新監視をオンにする";
  const locked = (label: string) => `${label}（新しい棚では変えられません）`;
  return (
    <Group gap={2} wrap="nowrap" justify="flex-end" className="work-card__actions" onClick={(event) => event.stopPropagation()} data-read-only={writable ? undefined : true}>
      <Tooltip label="読む">
        <ActionIcon variant="subtle" style={{ "--ai-color": "var(--flag-watch)" }} size="md" aria-label={`${work.title}：読む`} onClick={() => navigate(`/reader/${work.id}`)}>
          <Icons.read size={IconSize.action} />
        </ActionIcon>
      </Tooltip>
      <Tooltip label={writable ? favoriteLabel : locked("お気に入り")}>
        {writable ? (
          <ActionIcon variant="subtle" color={work.favorite ? "red" : "gray"} style={{ "--ai-color": work.favorite ? "var(--flag-favorite)" : "var(--flag-off)", "--ai-bg": work.favorite ? "var(--flag-favorite-bg)" : "transparent" }} size="md" aria-label={`${work.title}：${favoriteLabel}`} aria-pressed={work.favorite} onClick={toggleFavorite}>
            <Icons.favorite size={IconSize.action} fill={work.favorite ? "currentColor" : "none"} />
          </ActionIcon>
        ) : (
          <ActionIcon variant="subtle" color="gray" style={off} size="md" disabled aria-label={`${work.title}：${locked("お気に入り")}`} aria-pressed={work.favorite}>
            <Icons.favorite size={IconSize.action} fill={work.favorite ? "currentColor" : "none"} />
          </ActionIcon>
        )}
      </Tooltip>
      <Tooltip label={writable ? watchLabel : locked("更新監視")}>
        {writable ? (
          <ActionIcon variant="subtle" color={work.watchUpdates ? "piep" : "gray"} style={{ "--ai-color": work.watchUpdates ? "var(--flag-watch)" : "var(--flag-off)", "--ai-bg": work.watchUpdates ? "var(--flag-watch-bg)" : "transparent" }} size="md" aria-label={`${work.title}：${watchLabel}`} aria-pressed={work.watchUpdates} onClick={toggleWatch}>
            <Icons.watch size={IconSize.action} />
          </ActionIcon>
        ) : (
          <ActionIcon variant="subtle" color="gray" style={off} size="md" disabled aria-label={`${work.title}：${locked("更新監視")}`} aria-pressed={work.watchUpdates}>
            <Icons.watch size={IconSize.action} />
          </ActionIcon>
        )}
      </Tooltip>
      <ActionIcon variant="subtle" color="gray" style={off} size="md" disabled aria-label={`${work.title}：EPUBキュー（新しい棚ではまだ使えません）`}>
        <EpubQueueGlyph queued={false} />
      </ActionIcon>
      <Menu position="bottom-end" withinPortal>
        <Menu.Target><Tooltip label="その他"><ActionIcon variant="subtle" color="gray" style={off} size="md" aria-label={`${work.title}：その他`}><Icons.more size={IconSize.nav} /></ActionIcon></Tooltip></Menu.Target>
        <Menu.Dropdown>
          <Menu.Item leftSection={<Icons.workDetail size={IconSize.menu} />} onClick={() => navigate(`/works/${work.id}`)}>詳細</Menu.Item>
          <Menu.Divider />
          <Menu.Item color="red" leftSection={<Icons.delete size={IconSize.menu} />} disabled={!writable} aria-label={writable ? undefined : locked("棚から外す")} onClick={confirmRemove}>棚から外す</Menu.Item>
        </Menu.Dropdown>
      </Menu>
    </Group>
  );
}

function LegacyWorkActions({ work, queued, onQueue, onToggleFavorite, onToggleWatch }: WorkActionsProps & { work: DownloadEntry }) {
  const navigate = useAppNavigate();
  const queryClient = useQueryClient();
  const { removeFromEpubQueue } = useWorkspace();
  const refresh = () => invalidateWorkFlagViews(queryClient, work.id);
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
    children: <Text size="sm">「{work.title}」と保存済みの画像・添付をライブラリから削除します。この操作は元に戻せません。</Text>,
    labels: { confirm: "削除する", cancel: "キャンセル" },
    confirmProps: DANGER_CONFIRM,
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

  // ツールチップの文言をそのまま読み上げ名にも使う。一覧に何千枚も並ぶので、
  // 題名を前に付けてボタン名を一意にする。見えている文言と読み上げの文言が
  // 食い違うと、音声操作で名前を呼んでも押せない。
  const favoriteLabel = work.favorite ? "お気に入りを解除" : "お気に入りに追加";
  const watchLabel = work.watchUpdates ? "更新監視をオフにする" : "更新監視をオンにする";
  const queueLabel = queued ? "EPUBキューから外す" : "EPUBキューに追加";

  return (
    <Group gap={2} wrap="nowrap" justify="flex-end" className="work-card__actions" onClick={(event) => event.stopPropagation()}>
      {/* Nothing here carries a filled chip. A row of five solid squares is the
          loudest thing on a screen of two thousand works, and it competed with
          the covers the listing exists to show. State is drawn in the glyph
          itself: grey when off, its own colour when on, and filled in where the
          shape has an inside to fill. */}
      <Tooltip label="読む">
        <ActionIcon variant="subtle" style={{ "--ai-color": "var(--flag-watch)" }} size="md" aria-label={`${work.title}：読む`} onClick={() => navigate(`/reader/${work.id}`)}>
          <Icons.read size={IconSize.action} />
        </ActionIcon>
      </Tooltip>
      <Tooltip label={favoriteLabel}>
        <ActionIcon variant="subtle" color={work.favorite ? "red" : "gray"} style={{ "--ai-color": work.favorite ? "var(--flag-favorite)" : "var(--flag-off)", "--ai-bg": work.favorite ? "var(--flag-favorite-bg)" : "transparent" }} size="md" aria-label={`${work.title}：${favoriteLabel}`} aria-pressed={work.favorite} onClick={toggleFavorite}>
          <Icons.favorite size={IconSize.action} fill={work.favorite ? "currentColor" : "none"} />
        </ActionIcon>
      </Tooltip>
      {/* A rotating arrow has no inside, so this one can only change colour. */}
      <Tooltip label={watchLabel}>
        <ActionIcon variant="subtle" color={work.watchUpdates ? "piep" : "gray"} style={{ "--ai-color": work.watchUpdates ? "var(--flag-watch)" : "var(--flag-off)", "--ai-bg": work.watchUpdates ? "var(--flag-watch-bg)" : "transparent" }} size="md" aria-label={`${work.title}：${watchLabel}`} aria-pressed={work.watchUpdates} onClick={toggleWatch}>
          <Icons.watch size={IconSize.action} />
        </ActionIcon>
      </Tooltip>
      {/* One glyph in both states. Swapping the shape as well as the colour
          said the same thing twice, and the shape that changed was the part
          naming what the button does. Green is the "ep" half of the wordmark. */}
      <Tooltip label={queueLabel}>
        <ActionIcon variant="subtle" color={queued ? "leaf" : "gray"} style={{ "--ai-color": queued ? "var(--flag-epub)" : "var(--flag-off)", "--ai-bg": queued ? "var(--flag-epub-bg)" : "transparent" }} size="md" aria-label={`${work.title}：${queueLabel}`} aria-pressed={queued} onClick={onQueue}>
          <EpubQueueGlyph queued={queued} />
        </ActionIcon>
      </Tooltip>
      <Menu position="bottom-end" withinPortal>
        <Menu.Target><Tooltip label="その他"><ActionIcon variant="subtle" color="gray" style={{ "--ai-color": "var(--flag-off)" }} size="md" aria-label={`${work.title}：その他`}><Icons.more size={IconSize.nav} /></ActionIcon></Tooltip></Menu.Target>
        <Menu.Dropdown>
          <Menu.Item leftSection={<Icons.workDetail size={IconSize.menu} />} onClick={() => navigate(`/works/${work.id}`)}>詳細</Menu.Item>
          <Menu.Item leftSection={<Icons.watch size={IconSize.menu} />} onClick={() => navigate(`/updates?work=${work.id}`)}>新作・改稿の画面で確認</Menu.Item>
          <Menu.Divider />
          <Menu.Item color="red" leftSection={<Icons.delete size={IconSize.menu} />} onClick={confirmDelete}>ライブラリから削除</Menu.Item>
        </Menu.Dropdown>
      </Menu>
    </Group>
  );
}

export type { WorkCardProps };
