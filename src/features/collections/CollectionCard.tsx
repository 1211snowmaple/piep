import { Badge, Card, Group, Stack, Text } from "@mantine/core";
import { AppLink } from "@/app/router";
import { formatDate, formatNumber } from "@/lib/format";
import type { WorkCollectionSummary } from "@/types/collections";

/** One collection at a glance. Shared by the library tab and the author page so
 *  the same set reads identically wherever it is listed. */
export function CollectionCard({ collection }: { collection: WorkCollectionSummary }) {
  const missing = collection.memberCount - collection.availableCount;
  return (
    <Card component={AppLink} to={`/collections/${collection.id}`} withBorder padding="lg">
      <Stack gap="sm">
        <Group justify="space-between">
          <Badge variant="light" color={collection.collectionKind === "ordered" ? "blue" : "gray"}>
            {collection.collectionKind === "ordered" ? "順序付き" : "順序なし"}
          </Badge>
          <Text size="xs" c="dimmed">{formatDate(collection.updatedAt)}</Text>
        </Group>
        <Text fw={700} size="lg">{collection.name}</Text>
        {collection.description && <Text size="sm" c="dimmed" lineClamp={2}>{collection.description}</Text>}
        <Text size="sm">{formatNumber(collection.memberCount)}作品 · {formatNumber(collection.totalTextLength)}字</Text>
        {missing > 0 && <Badge color="orange" variant="light">未保存 {formatNumber(missing)}作品</Badge>}
      </Stack>
    </Card>
  );
}
