import { Button, Group, Text } from "@mantine/core";
import { useAppNavigate } from "@/app/router";
import { requestListRead } from "@/features/save/saveDraft";

const LISTS = [
  { label: "フォロー新着", url: "https://www.pixiv.net/novel/bookmark_new.php" },
  { label: "ブックマーク", url: "https://www.pixiv.net/novel/bookmark.php" },
];

/**
 * 内蔵ブラウザで一覧を開いて、そのまま読み取る入口。
 *
 * 読むのは内蔵ブラウザで、ここからは通信しない。保存は保存の画面で候補を選んだ
 * あと、取得用の接続で行う。
 */
export function BrowserListShortcuts() {
  const navigate = useAppNavigate();
  const open = (url: string) => {
    requestListRead("pixiv");
    navigate(`/save/pixiv?url=${encodeURIComponent(url)}`);
  };
  return (
    <Group gap="xs" mt="xs" wrap="wrap">
      <Text size="xs" c="dimmed">ブラウザから読む</Text>
      {LISTS.map((list) => (
        <Button key={list.url} variant="light" size="compact-xs" onClick={() => open(list.url)}>{list.label}</Button>
      ))}
    </Group>
  );
}
