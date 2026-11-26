import { Badge, Button, Checkbox, Group, Stack, Text } from "@mantine/core";
import { mergePageRows, type PageRows } from "@/features/browser/downloadCandidates";

/** フォロー一覧から読み取った作者（クリエイター）の行。`watched` は監視に登録済みかどうか。 */
export interface FollowedUserRow {
  userId: string;
  name: string;
  watched: boolean;
  selected: boolean;
}

/** すでにある行の選択を引き継ぎ、監視への登録状態を更新する。 */
export function mergeFollowedUsers(
  current: PageRows<FollowedUserRow> | undefined,
  incoming: FollowedUserRow[],
  pageNumber: number,
): PageRows<FollowedUserRow> {
  return mergePageRows(current, incoming, pageNumber, (row) => row.userId, (row, fresh) => (
    row.watched === fresh.watched ? row : { ...row, watched: fresh.watched }
  ));
}

export function FollowedUsersList({ rows, adding, person, onToggle, onAdd }: {
  rows: FollowedUserRow[];
  adding: boolean;
  /** 作者にあたる人の呼び方（pixiv は作者、FANBOX はクリエイター）。 */
  person: string;
  onToggle: (userId: string, selected: boolean) => void;
  onAdd: () => void;
}) {
  const count = rows.filter((row) => row.selected && !row.watched).length;
  return (
    <Stack gap="xs" pb="md">
      <Button size="xs" loading={adding} disabled={!count} onClick={onAdd}>
        {count ? `${count}人を監視に追加` : `監視に追加する${person}を選択`}
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
