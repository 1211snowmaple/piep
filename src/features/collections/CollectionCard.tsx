import { Badge, Card, Group, Stack, Text, Tooltip } from "@mantine/core";
import { AppLink } from "@/app/router";
import { CollectionCover } from "@/components/CollectionCover";
import { formatDate, formatNumber } from "@/lib/format";
import { Icons, IconSize } from "@/lib/icons";
import type { WorkCollectionSummary } from "@/types/collections";

const TRACK_LABEL: Record<string, string> = {
  sequence: "続き物",
  theme: "テーマ",
};

/**
 * 束をひとつ、ひと目で。ライブラリのタブと作者ページが共有する。
 *
 * 以前は表紙を受け取っておきながら描いていなかった。同じ画面に並ぶ作者と
 * シリーズのカードには表紙があるので、コレクションだけが文字の箱に見えていた。
 * 一覧の中で一種類だけ見え方が違うと、それは作りかけに見える。
 */
export function CollectionCard({ collection }: { collection: WorkCollectionSummary }) {
  const missing = collection.memberCount - collection.availableCount;
  const track = TRACK_LABEL[collection.track];
  return (
    <Card
      component={AppLink}
      to={`/collections/${collection.id}`}
      withBorder
      padding="md"
      className="collection-card surface--interactive focus-card"
    >
      <Group wrap="nowrap" align="stretch" gap="md">
        <CollectionCover collection={collection} variant="card" />
        <Stack gap={6} flex={1} miw={0} justify="center">
          <Group gap={6} wrap="nowrap">
            <Badge size="xs" variant="light" color={collection.collectionKind === "ordered" ? "piep" : "gray"}>
              {collection.collectionKind === "ordered" ? "順序付き" : "順序なし"}
            </Badge>
            {track && <Badge size="xs" variant="outline" color="gray">{track}</Badge>}
          </Group>
          <Tooltip label={collection.name} multiline maw={480} openDelay={350} withArrow>
            <Text fw={720} size="md" lh={1.35} className="line-clamp-2">{collection.name}</Text>
          </Tooltip>
          {collection.description && (
            <Text size="xs" c="dimmed" className="line-clamp-2">{collection.description}</Text>
          )}
          <Group gap="sm" wrap="wrap" className="collection-card__facts">
            <Text size="xs" c="dimmed">
              <Icons.collection size={IconSize.inline} />{formatNumber(collection.memberCount)}作品
            </Text>
            <Text size="xs" c="dimmed">
              <Icons.textLength size={IconSize.inline} />{formatNumber(collection.totalTextLength)}字
            </Text>
            <Text size="xs" c="dimmed">{formatDate(collection.updatedAt)}</Text>
            {missing > 0 && <Badge size="xs" color="orange" variant="light">未保存 {formatNumber(missing)}</Badge>}
          </Group>
        </Stack>
      </Group>
    </Card>
  );
}
