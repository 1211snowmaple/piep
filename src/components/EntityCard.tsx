import { Avatar, Badge, Box, Card, Group, Stack, Text, UnstyledButton } from "@mantine/core";
import { Icons, IconSize } from "@/lib/icons";
import { useAppNavigate } from "@/app/router";
import { ProviderMark } from "@/lib/providers";
import { formatDate, formatNumber } from "@/lib/format";
import { getAssetUrl } from "@/services/dbApi";
import type { EntityFacet } from "@/types/library";

export function EntityCard({ entity, kind, selectionMode = false, selected = false, onSelect }: {
  entity: EntityFacet;
  kind: "person" | "series";
  selectionMode?: boolean;
  selected?: boolean;
  /** Receives the entity so the list can keep one handler for every card. */
  onSelect?: (entity: EntityFacet, selected: boolean) => void;
}) {
  const navigate = useAppNavigate();
  const route = kind === "person" ? "people" : "series";
  const icon = getAssetUrl(entity.iconPath ?? entity.coverPath);
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
      {selectionMode && (
        <UnstyledButton
          className="entity-card__select"
          data-checked={selected || undefined}
          aria-hidden
          tabIndex={-1}
        >
          <span className="work-selection-toggle__indicator">{selected && <Icons.confirm size={IconSize.menu} strokeWidth={3} />}</span>
        </UnstyledButton>
      )}
      <Box className="entity-card__banner">
        {banner && <img src={banner} alt="" loading="lazy" decoding="async" />}
        <Box className="entity-card__wash" />
      </Box>
      <Group wrap="nowrap" align="center" p="md" className="entity-card__content">
        {kind === "person" ? <Avatar src={icon} color="piep" size={64} radius="xl" imageProps={{ loading: "lazy", decoding: "async" }}><Icons.person size={IconSize.feature} /></Avatar> : <Box className="entity-card__series-cover">{icon ? <img src={icon} alt="" loading="lazy" decoding="async" /> : <Icons.series size={IconSize.feature} />}</Box>}
        <Stack gap={5} flex={1} miw={0}>
          <Text fw={700} className="line-clamp-2" lh={1.35}>{entity.displayName}</Text>
          <Text size="sm" c="dimmed" className="line-clamp-2">{entity.description || entity.sampleTitle || (kind === "person" ? "保存作品の作者" : "保存作品のシリーズ")}</Text>
          <Group gap="xs" wrap="nowrap"><ProviderMark provider={entity.source} compact /><Badge variant="light" color="gray">{formatNumber(entity.count)}作品</Badge><Text size="xs" c="dimmed" className="line-clamp-1">最終保存 {formatDate(entity.latestDownloadedAt)}</Text></Group>
        </Stack>
        {!selectionMode && <Icons.next size={IconSize.action} color="var(--mantine-color-dimmed)" />}
      </Group>
    </Card>
  );
}
