import { ActionIcon, Avatar, Badge, Box, Card, Group, Stack, Text, Tooltip } from "@mantine/core";
import { Icons, IconSize } from "@/lib/icons";
import { NoImageMark } from "@/components/NoImageMark";
import { useAppNavigate } from "@/app/router";
import { getProvider, ProviderMark } from "@/lib/providers";
import { formatDate, formatNumber } from "@/lib/format";
import { getAssetUrl } from "@/services/dbApi";
import type { EntityFacet } from "@/types/library";

/**
 * 追いかけているかどうか。
 *
 * 「登録していない」と「登録して止めている」は別の決定なので、別の言葉に
 * する。未登録には印を出さない - 無い状態に印は要らない。
 */
export type EntityWatchState = "watching" | "paused" | null;

function watchTooltip(watch: EntityWatchState, kind: "person" | "series"): string {
  const noun = kind === "person" ? "この作者" : "このシリーズ";
  if (watch === "watching") return `${noun}の更新監視を止める`;
  if (watch === "paused") return `${noun}の更新監視を再開する（いまは停止中）`;
  return `${noun}の更新を監視する`;
}

export function EntityCard({ entity, kind, selectionMode = false, selected = false, onSelect, watch = null, onToggleWatch }: {
  entity: EntityFacet;
  kind: "person" | "series";
  selectionMode?: boolean;
  selected?: boolean;
  /** Receives the entity so the list can keep one handler for every card. */
  onSelect?: (entity: EntityFacet, selected: boolean) => void;
  watch?: EntityWatchState;
  /** 追いかけるかどうかを、その場で切り替える。 */
  onToggleWatch?: (entity: EntityFacet, next: boolean) => void;
}) {
  const navigate = useAppNavigate();
  const route = kind === "person" ? "people" : "series";
  const icon = getAssetUrl(entity.iconPath ?? entity.coverPath);
  // 保存元が画像を持ちうるのに、この人は置いていない。「まだ無い」ではなく
  // 「置いていない」なので、保存元と同じく一枚の絵で示す。
  //
  // 人物だけ。あの一枚はプロフィール画像の代わりであって、縦長のシリーズ表紙枠
  // に敷くと文字が切れる。シリーズは今までどおり記号のまま。
  const noImage = kind === "person" && !icon && getProvider(entity.source).hasProfileImages;
  const banner = getAssetUrl(entity.bannerPath);
  const open = () => navigate(`/${route}/${encodeURIComponent(entity.source)}/${encodeURIComponent(entity.sourceKey)}`);
  // Selection mode is modal, as it is on works: the whole card picks instead of
  // following the link, so a press can never do the thing you did not mean.
  const activate = () => selectionMode ? onSelect?.(entity, !selected) : open();
  return (
    <Card
      p={0}
      className="entity-card surface--interactive focus-card"
      data-kind={kind}
      data-selection-mode={selectionMode || undefined}
      data-selected={selectionMode && selected || undefined}
      onClick={activate}
      onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); activate(); } }}
      tabIndex={0}
      role={selectionMode ? "button" : "link"}
      aria-pressed={selectionMode ? selected : undefined}
      aria-label={selectionMode ? `${entity.displayName}を${selected ? "選択解除" : "選択"}` : `${entity.displayName}を開く`}
    >
      {/* 選ぶ場所はカード全体。印は中央に大きく置く - 隅の小さな丸は
          「押せる場所」として狙う必要があり、選んだかどうかも遠目に
          分からなかった。膜がかかること自体が「いまは選ぶモードだ」を伝える。 */}
      {selectionMode && (
        <span className="card-select" data-checked={selected || undefined} aria-hidden>
          <span className="card-select__ring">
            <Icons.confirm size={IconSize.nav} strokeWidth={3} />
          </span>
        </span>
      )}
      <Box className="entity-card__banner">
        {banner && <img src={banner} alt="" loading="lazy" decoding="async" />}
        <Box className="entity-card__wash" />
      </Box>
      <Group wrap="nowrap" align="center" p="md" className="entity-card__content">
        {kind === "person"
          ? <Avatar src={icon} color="piep" size={64} radius="xl" className="entity-avatar" imageProps={{ loading: "lazy", decoding: "async" }}>{noImage ? <NoImageMark /> : <Icons.person size={IconSize.feature} />}</Avatar>
          : <Box className="entity-card__series-cover">{icon ? <img src={icon} alt="" loading="lazy" decoding="async" /> : <Icons.series size={IconSize.feature} />}</Box>}
        <Stack gap={5} flex={1} miw={0}>
          <Text fw={700} className="line-clamp-2" lh={1.35}>{entity.displayName}</Text>
          <Text size="sm" c="dimmed" className="line-clamp-2">{entity.description || entity.sampleTitle || (kind === "person" ? "保存作品の作者" : "保存作品のシリーズ")}</Text>
          {/* 帯は「取得元が言っている事実」だけにする。こちらの決めごと
              （追いかけるかどうか）は帯ではなくボタンで、作品カードと同じ
              場所・同じ色・同じ作法にする。
              印が増えたぶん、1行に入らない幅では最終保存が折り返す。 */}
          <Group gap={6} wrap="wrap" className="entity-card__meta">
            <ProviderMark provider={entity.source} compact />
            <Badge variant="light" color="gray" style={{ flex: "none" }}>{formatNumber(entity.count)}作品</Badge>
            {kind === "series" && entity.isConcluded === true && <Badge variant="light" color="gray" style={{ flex: "none" }}>完結</Badge>}
            {kind === "series" && entity.isConcluded === false && <Badge variant="light" color="piep" style={{ flex: "none" }}>連載中</Badge>}
            <Text size="xs" c="dimmed" className="line-clamp-1">最終保存 {formatDate(entity.latestDownloadedAt)}</Text>
          </Group>
        </Stack>
        {/* 状態は絵柄そのもので描く - 消えていれば灰、点いていればその色。
            登録があって止めているだけの状態は、灰に薄い下地を敷いて分ける。
            出したり消したりしないので、監視の有無で行の中身がずれない。 */}
        {!selectionMode && onToggleWatch && (
          <Tooltip label={watchTooltip(watch, kind)}>
            <ActionIcon
              className="entity-card__watch"
              variant="subtle"
              size="lg"
              data-state={watch ?? "off"}
              aria-label={watchTooltip(watch, kind)}
              aria-pressed={watch === "watching"}
              onClick={(event) => { event.stopPropagation(); onToggleWatch(entity, watch !== "watching"); }}
            >
              <Icons.watch size={IconSize.action} />
            </ActionIcon>
          </Tooltip>
        )}
        {!selectionMode && <Icons.next size={IconSize.action} color="var(--mantine-color-dimmed)" />}
      </Group>
    </Card>
  );
}
