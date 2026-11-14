import { Badge, Card, Group, Switch, Text } from "@mantine/core";
import { isTauriRuntime } from "@/services/dbApi";
import type { LibraryV2Status } from "@/services/libraryV2Api";
import { useLibraryV2Status, useShelfV2Preference } from "@/features/library/shelfV2";
import { formatNumber } from "@/lib/format";

const STATE_LABEL: Record<LibraryV2Status["state"], { label: string; color: string }> = {
  starting: { label: "開いています", color: "gray" },
  absent: { label: "ありません", color: "gray" },
  opening: { label: "開いています", color: "gray" },
  indexing: { label: "検索の準備中", color: "yellow" },
  ready: { label: "使えます", color: "green" },
  failed: { label: "開けません", color: "red" },
};

/**
 * 新しい棚（試験中）の一覧を試すための切り替え。棚が無ければ何も出さない。
 *
 * 新しい棚は旧棚から移行して作った写しで、アプリは読むだけにしている。切り替えるのは
 * ライブラリの作品の一覧だけ。
 */
export function ShelfV2Settings() {
  const runtime = isTauriRuntime();
  const status = useLibraryV2Status(runtime);
  const [enabled, setEnabled] = useShelfV2Preference();
  const data = status.data;
  if (!runtime || !data || data.state === "absent") return null;
  const state = STATE_LABEL[data.state];
  const unresolved = data.state === "ready" ? data.unresolved.length : 0;
  return (
    <Card p="lg">
      <Group justify="space-between" align="flex-start" wrap="nowrap" gap="sm">
        <Text fw={700}>新しい棚（試験中）</Text>
        <Badge variant="light" color={state.color}>{state.label}</Badge>
      </Group>
      <Text size="sm" c="dimmed" mt={4} mb="md">
        移行した時点の写しで、その後に保存した作品は入っていません。読むだけで、ここからは何も書き換えません。
      </Text>
      <Switch
        checked={enabled}
        onChange={(event) => setEnabled(event.currentTarget.checked)}
        label="ライブラリの作品の一覧を新しい棚から出す"
        description="お気に入りなどの変更・複数選択・作品の画面・読む画面は、まだ使えません。検索語での検索は、検索の準備が済んでから使えます。"
      />
      {data.state === "failed" && <Text size="sm" c="red" mt="sm">{data.message}</Text>}
      {unresolved > 0 && <Text size="sm" c="dimmed" mt="sm">移行の未解決の記録が{formatNumber(unresolved)}件あります。</Text>}
    </Card>
  );
}
