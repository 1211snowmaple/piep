import { Badge, Button, Checkbox, Group, Stack, Text } from "@mantine/core";

/** フォロー一覧から読み取った作者の行。`watched` は監視に登録済みかどうか。 */
export interface FollowedUserRow {
  userId: string;
  name: string;
  watched: boolean;
  selected: boolean;
}

/** 同じ一覧の続きを足す。すでにある行の選択は残す。 */
export function mergeFollowedUsers(current: FollowedUserRow[], incoming: FollowedUserRow[]): FollowedUserRow[] {
  const known = new Set(current.map((row) => row.userId));
  return [...current, ...incoming.filter((row) => !known.has(row.userId))];
}

export function FollowedUsersList({ rows, adding, onToggle, onAdd }: {
  rows: FollowedUserRow[];
  adding: boolean;
  onToggle: (userId: string, selected: boolean) => void;
  onAdd: () => void;
}) {
  const count = rows.filter((row) => row.selected && !row.watched).length;
  return (
    <Stack gap="xs" pb="md">
      <Button size="xs" loading={adding} disabled={!count} onClick={onAdd}>
        {count ? `${count}人を監視に追加` : "監視に追加する作者を選択"}
      </Button>
      {rows.map((row) => (
        <Group key={row.userId} gap="xs" wrap="nowrap" justify="space-between">
          <Checkbox
            size="xs"
            label={row.name}
            aria-label={`${row.name}を監視に追加する`}
            disabled={row.watched || adding}
            checked={row.selected && !row.watched}
            onChange={(event) => onToggle(row.userId, event.currentTarget.checked)}
            styles={{ label: { overflowWrap: "anywhere" } }}
          />
          {row.watched && (
            <Badge size="xs" variant="light" color="gray" tt="none" style={{ flexShrink: 0 }}>
              <Text span size="xs" inherit>監視に登録済み</Text>
            </Badge>
          )}
        </Group>
      ))}
    </Stack>
  );
}
