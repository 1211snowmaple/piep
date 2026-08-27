import { useEffect, useRef, useState } from "react";
import { ActionIcon, Badge, Box, Button, Card, Collapse, Group, Text, Tooltip } from "@mantine/core";
import { WorkCard } from "@/components/WorkCard";
import { WorkCover } from "@/components/WorkCover";
import { formatNumber } from "@/lib/format";
import { Icons, IconSize } from "@/lib/icons";
import { ProviderMark } from "@/lib/providers";
import type { ViewMode } from "@/lib/viewMode";
import type { WorkCollectionMember } from "@/types/collections";

/** 共通の見方をそのまま使う。束の中だけ別の好みを覚えない。 */
export type MemberView = ViewMode;
const INITIAL_MEMBER_RENDER = 120;

interface MemberListProps {
  members: WorkCollectionMember[];
  ordered: boolean;
  view: MemberView;
  busy: boolean;
  selectionMode: boolean;
  selected: ReadonlySet<number>;
  onSelect: (downloadId: number, selected: boolean) => void;
  onMove: (index: number, delta: number) => void | boolean | Promise<boolean>;
  onDropAt: (from: number, to: number) => void | boolean | Promise<boolean>;
  onRemove: (member: WorkCollectionMember) => void;
}

/**
 * 束の中身を、棚とまったく同じカードで描く。
 *
 * これができていなかったのは型が合わなかったからで、好みの問題ではなかった。
 * メンバーの取得が縮小した投影を返していたので `WorkCard` に渡せず、表紙と
 * 題名と字数だけの行を手で組み直していた。保存側が `DownloadEntry` を返す
 * ようになった以上、棚の作り込み（タグ行・作者の顔・お気に入り・EPUBキュー・
 * 改稿の印）は**そのまま束の中でも効く**。
 *
 * 束にしかない情報 — 何番目か、順を入れ替える、外す、別版を畳む — だけを
 * カードの外側に足す。カードの中には手を入れない。
 */
export function CollectionMemberList({
  members,
  ordered,
  view,
  busy,
  selectionMode,
  selected,
  onSelect,
  onMove,
  onDropAt,
  onRemove,
}: MemberListProps) {
  const [dragFrom, setDragFrom] = useState<number | null>(null);
  const [dragOver, setDragOver] = useState<number | null>(null);
  const dragFromRef = useRef<number | null>(null);
  const dragOverRef = useRef<number | null>(null);
  const autoScrollFrameRef = useRef<number | null>(null);
  const scrollContainerRef = useRef<HTMLElement | null>(null);
  const pointerRef = useRef({ x: 0, y: 0 });
  const [announcement, setAnnouncement] = useState("");
  const [visibleCount, setVisibleCount] = useState(INITIAL_MEMBER_RENDER);
  const draggable = ordered && !selectionMode && !busy;
  useEffect(() => {
    setVisibleCount((current) => Math.min(members.length, Math.max(INITIAL_MEMBER_RENDER, current)));
  }, [members.length]);
  const visibleMembers = members.slice(0, visibleCount);
  const markDragOver = (index: number | null) => {
    dragOverRef.current = index;
    setDragOver(index);
  };
  const stopAutoScroll = () => {
    if (autoScrollFrameRef.current !== null) {
      window.cancelAnimationFrame(autoScrollFrameRef.current);
      autoScrollFrameRef.current = null;
    }
    scrollContainerRef.current = null;
  };
  const clearDrag = () => {
    stopAutoScroll();
    dragFromRef.current = null;
    dragOverRef.current = null;
    setDragFrom(null);
    setDragOver(null);
  };
  const beginDrag = (index: number) => {
    dragFromRef.current = index;
    setDragFrom(index);
    markDragOver(index);
  };
  // 掴んでいる間だけ Escape を見る。取っ手は焦点を取らないので（選択を止めるため
  // pointerdown の既定を止めている）、鍵の行き先は窓しかない。
  useEffect(() => {
    if (dragFrom === null) return;
    const cancel = (event: KeyboardEvent) => { if (event.key === "Escape") clearDrag(); };
    window.addEventListener("keydown", cancel);
    return () => window.removeEventListener("keydown", cancel);
  }, [dragFrom]);
  useEffect(() => () => stopAutoScroll(), []);
  const memberAtPoint = (x: number, y: number) => {
    // 坐標から引けない環境もある。引けなければ直前の行へ戻るだけで、例外にはしない。
    const row = document.elementFromPoint?.(x, y)?.closest<HTMLElement>(".collection-member");
    const index = Number(row?.dataset.memberIndex);
    return Number.isInteger(index) && index >= 0 && index < members.length ? index : null;
  };
  const announceMove = (member: WorkCollectionMember, to: number) => {
    setAnnouncement(`${member.title}を${to + 1}番目へ移動しました`);
  };
  const announceMoveResult = (
    result: void | boolean | Promise<boolean>,
    member: WorkCollectionMember,
    to: number,
  ) => {
    void Promise.resolve(result).then((saved) => {
      if (saved === false) {
        setAnnouncement(`${member.title}の順序を保存できなかったため、元の位置へ戻しました`);
      } else {
        announceMove(member, to);
      }
    }, () => {
      setAnnouncement(`${member.title}の順序を保存できなかったため、元の位置へ戻しました`);
    });
  };
  const moveWithKeyboard = (index: number, delta: number) => {
    const to = index + delta;
    announceMoveResult(onMove(index, delta), members[index], to);
  };
  const continueAutoScroll = () => {
    if (autoScrollFrameRef.current !== null) return;
    const step = () => {
      autoScrollFrameRef.current = null;
      if (dragFromRef.current === null) return;
      const scroller = scrollContainerRef.current;
      if (!scroller) return;
      const rect = scroller === document.scrollingElement
        ? { top: 0, bottom: window.innerHeight }
        : scroller.getBoundingClientRect();
      const edge = Math.min(72, Math.max(40, (rect.bottom - rect.top) / 5));
      const { x, y } = pointerRef.current;
      const topRatio = y < rect.top + edge ? (rect.top + edge - y) / edge : 0;
      const bottomRatio = y > rect.bottom - edge ? (y - (rect.bottom - edge)) / edge : 0;
      const delta = Math.round(Math.max(-1, Math.min(1, bottomRatio - topRatio)) * 18);
      if (delta === 0) return;
      scroller.scrollTop += delta;
      if (delta > 0) {
        // ドラッグ中に描画上限へ着いたら次の塊を出す。取っ手を離して
        // 「さらに表示」を押し直さなくても、末尾まで運べる。
        const remaining = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
        if (remaining < 320) {
          setVisibleCount((count) => Math.min(members.length, count + INITIAL_MEMBER_RENDER));
        }
      }
      const target = memberAtPoint(x, y);
      if (target !== null) markDragOver(target);
      autoScrollFrameRef.current = window.requestAnimationFrame(step);
    };
    autoScrollFrameRef.current = window.requestAnimationFrame(step);
  };

  return (
    <>
      <span className="visually-hidden" aria-live="polite" aria-atomic="true">{announcement}</span>
      <div className={`collection-members collection-members--${view}`} data-ordered={ordered || undefined} role={ordered ? "list" : undefined} aria-label={ordered ? "コレクションの作品順" : undefined}>
      {visibleMembers.map((member, index) => (
        <div
          key={`${member.source}:${member.sourceId}`}
          className="collection-member"
          data-member-index={index}
          data-dragging={dragFrom === index || undefined}
          data-drop-target={dragOver === index && dragFrom !== index || undefined}
          data-drop-after={dragOver === index && dragFrom !== null && dragFrom < index || undefined}
          role={ordered ? "listitem" : undefined}
          aria-posinset={ordered ? index + 1 : undefined}
          aria-setsize={ordered ? members.length : undefined}
        >
          {ordered && (
            <div className="collection-member__rail">
              {draggable && (
                <Tooltip label="掴んで並べ替え">
                  <ActionIcon
                    className="collection-member__grip"
                    variant="subtle"
                    color="gray"
                    size={40}
                    aria-hidden="true"
                    tabIndex={-1}
                    onPointerDown={(event) => {
                      if (event.pointerType === "mouse" && event.button !== 0) return;
                      // 既定を止めないと、文字選択が始まって運んでいる途中の行が青くなる。
                      event.preventDefault();
                      // 以降のポインタ事象をこの取っ手へ集める。捕捉できない環境でも、
                      // 例外で並べ替えごと止めない。
                      try {
                        event.currentTarget.setPointerCapture?.(event.pointerId);
                      } catch {
                        // 捕捉できないだけ。掴んだ状態は続ける。
                      }
                      scrollContainerRef.current = event.currentTarget.closest<HTMLElement>(".app-main")
                        ?? document.scrollingElement as HTMLElement | null;
                      pointerRef.current = { x: event.clientX, y: event.clientY };
                      beginDrag(index);
                    }}
                    onPointerMove={(event) => {
                      if (dragFromRef.current === null) return;
                      event.preventDefault();
                      pointerRef.current = { x: event.clientX, y: event.clientY };
                      const target = memberAtPoint(event.clientX, event.clientY);
                      if (target !== null) markDragOver(target);
                      continueAutoScroll();
                    }}
                    onPointerUp={(event) => {
                      const from = dragFromRef.current;
                      if (from === null) return;
                      const to = memberAtPoint(event.clientX, event.clientY) ?? dragOverRef.current;
                      const moved = members[from];
                      clearDrag();
                      if (to !== null && from !== to) {
                        announceMoveResult(onDropAt(from, to), moved, to);
                      }
                    }}
                    onPointerCancel={clearDrag}
                  >
                    <Icons.drag size={IconSize.menu} />
                  </ActionIcon>
                </Tooltip>
              )}
              <Text className="collection-member__index" fw={700}>{index + 1}</Text>
              {/* キーボードで並べ替えられる道を必ず残す。掴んで運ぶだけには
                  しない。 */}
              <Group gap={0} className="collection-member__nudge">
                <Tooltip label="一つ前へ">
                  <ActionIcon variant="subtle" size="sm" aria-label={`${member.title}を一つ前へ`} disabled={index === 0 || busy || selectionMode} onClick={() => moveWithKeyboard(index, -1)}>
                    <Icons.up size={IconSize.menu} />
                  </ActionIcon>
                </Tooltip>
                <Tooltip label="一つ後へ">
                  <ActionIcon variant="subtle" size="sm" aria-label={`${member.title}を一つ後へ`} disabled={index === members.length - 1 || busy || selectionMode} onClick={() => moveWithKeyboard(index, 1)}>
                    <Icons.down size={IconSize.menu} />
                  </ActionIcon>
                </Tooltip>
              </Group>
            </div>
          )}

          <div className="collection-member__body">
            {member.work ? (
              <WorkCard
                work={member.work}
                compact={view === "compact"}
                selectionMode={selectionMode}
                selected={member.downloadId ? selected.has(member.downloadId) : false}
                onSelect={onSelect}
              />
            ) : (
              <MissingMemberCard member={member} />
            )}
            {member.work && member.work.textLength === 0 && (
              <Group gap={4} className="collection-member__note">
                <Icons.notice size={IconSize.inline} />
                <Text size="xs" c="dimmed">
                  本文が取れていません。一冊のEPUBには入りません。
                </Text>
              </Group>
            )}
            {member.editions.length > 0 && <EditionFold member={member} />}
          </div>

          {!selectionMode && (
            <Tooltip label="コレクションから外す">
              <ActionIcon
                className="collection-member__remove"
                color="red"
                variant="subtle"
                aria-label={`${member.title}をコレクションから外す`}
                disabled={busy}
                onClick={() => onRemove(member)}
              >
                <Icons.delete size={IconSize.menu} />
              </ActionIcon>
            </Tooltip>
          )}
        </div>
      ))}
      {visibleMembers.length < members.length && (
        <Group justify="center" py="md" style={{ gridColumn: "1 / -1" }}>
          <Button variant="default" onClick={() => setVisibleCount((count) => Math.min(members.length, count + INITIAL_MEMBER_RENDER))}>
            さらに表示（残り{formatNumber(members.length - visibleMembers.length)}件）
          </Button>
        </Group>
      )}
      </div>
    </>
  );
}

/**
 * まだ保存していないメンバー。
 *
 * 束は `source` と `source_id` で作品を覚えているので、消したり取り直したり
 * しても所属は残る。ここに出るのは「覚えてはいるが、いま手元に本文が無い」
 * 作品で、束から外すべきものではない。
 */
function MissingMemberCard({ member }: { member: WorkCollectionMember }) {
  return (
    <Card withBorder padding="sm" className="collection-member__missing">
      <Group wrap="nowrap" align="center">
        <WorkCover
          work={{
            source: member.source,
            sourceId: member.sourceId,
            title: member.title,
            authorName: member.authorName,
            coverPath: member.coverPath,
          }}
          variant="compact"
        />
        <Box flex={1} miw={0}>
          <Group gap="xs" wrap="nowrap">
            <Tooltip label={member.title} multiline maw={480} openDelay={350} withArrow>
              <Text fw={650} size="sm" className="line-clamp-1">{member.title}</Text>
            </Tooltip>
            <Badge size="xs" color="orange" variant="light">未保存</Badge>
          </Group>
          <Group gap="xs">
            <ProviderMark provider={member.source} compact />
            <Text size="xs" c="dimmed" className="line-clamp-1">{member.authorName}</Text>
          </Group>
        </Box>
      </Group>
    </Card>
  );
}

/**
 * 同じ作品の別版。
 *
 * 【FANBOXサンプル】と【全文】は続きではないので、行としては1本にする。
 * ただし消しはしない — 手元にどちらもあることは事実で、EPUB に入れるのは
 * どちらか、を選べる必要がある。
 */
function EditionFold({ member }: { member: WorkCollectionMember }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="collection-member__editions">
      <Button
        variant="subtle"
        size="compact-xs"
        color="gray"
        leftSection={<Icons.expand size={IconSize.inline} style={{ transform: open ? "rotate(180deg)" : undefined }} />}
        onClick={() => setOpen((value) => !value)}
      >
        別版 {formatNumber(member.editions.length)}件
      </Button>
      <Collapse expanded={open}>
        <div className="collection-member__edition-list">
          {member.editions.map((edition) => (
            <Group key={edition.id} gap="xs" wrap="nowrap" className="collection-member__edition">
              <WorkCover work={edition} variant="compact" />
              <Box flex={1} miw={0}>
                <Tooltip label={edition.title} multiline maw={480} openDelay={350} withArrow>
                  <Text size="xs" fw={600} className="line-clamp-1">{edition.title}</Text>
                </Tooltip>
                <Text size="xs" c="dimmed">{formatNumber(edition.textLength)}字</Text>
              </Box>
            </Group>
          ))}
        </div>
      </Collapse>
    </div>
  );
}
