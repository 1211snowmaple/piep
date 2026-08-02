import { Avatar, BackgroundImage, Badge, Box, Card, Group, Stack, Text } from "@mantine/core";
import { ChevronRight, UserRound, Library } from "lucide-react";
import { useAppNavigate } from "@/app/router";
import { ProviderMark } from "@/lib/providers";
import { formatDate, formatNumber } from "@/lib/format";
import { getAssetUrl } from "@/services/dbApi";
import type { EntityFacet } from "@/types/library";

export function EntityCard({ entity, kind }: { entity: EntityFacet; kind: "person" | "series" }) {
  const navigate = useAppNavigate();
  const route = kind === "person" ? "people" : "series";
  const icon = getAssetUrl(entity.iconPath ?? entity.coverPath);
  const banner = getAssetUrl(entity.bannerPath);
  const open = () => navigate(`/${route}/${encodeURIComponent(entity.source)}/${encodeURIComponent(entity.sourceKey)}`);
  return (
    <Card p={0} className="entity-card surface--interactive focus-card" data-kind={kind} onClick={open} onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); open(); } }} tabIndex={0} role="link" aria-label={`${entity.displayName}を開く`}>
      <Box className="entity-card__banner">
        {(banner || icon) && <BackgroundImage src={banner || icon!} h="100%" />}
        <Box className="entity-card__wash" />
      </Box>
      <Group wrap="nowrap" align="center" p="md" className="entity-card__content">
        {kind === "person" ? <Avatar src={icon} color="piep" size={64} radius="xl"><UserRound size={22} /></Avatar> : <Box className="entity-card__series-cover">{icon ? <img src={icon} alt="" /> : <Library size={23} />}</Box>}
        <Stack gap={5} flex={1} miw={0}>
          <Text size="10px" fw={700} c="dimmed" tt="uppercase" lts="0.06em">{kind === "person" ? "Creator" : "Series"}</Text>
          <Text fw={700} className="line-clamp-2" lh={1.35}>{entity.displayName}</Text>
          <Text size="sm" c="dimmed" className="line-clamp-2">{entity.description || entity.sampleTitle || (kind === "person" ? "保存作品の作者" : "保存作品のシリーズ")}</Text>
          <Group gap="xs" wrap="nowrap"><ProviderMark provider={entity.source} compact /><Badge variant="light" color="gray">{formatNumber(entity.count)}作品</Badge><Text size="xs" c="dimmed" className="line-clamp-1">最終保存 {formatDate(entity.latestDownloadedAt)}</Text></Group>
        </Stack>
        <ChevronRight size={17} color="var(--mantine-color-dimmed)" />
      </Group>
    </Card>
  );
}
