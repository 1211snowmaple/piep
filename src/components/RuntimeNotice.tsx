import { Alert, Button, Group, Text } from "@mantine/core";
import { Icons, IconSize } from "@/lib/icons";
import { isTauriRuntime } from "@/services/dbApi";

export function RuntimeNotice() {
  if (isTauriRuntime()) return null;
  return (
    <Alert color="blue" variant="light" icon={<Icons.desktopApp size={IconSize.nav} />} title="ブラウザプレビュー">
      <Group justify="space-between" align="center" wrap="wrap">
        <Text size="sm" flex={1} miw={240}>レイアウト確認用のデモデータを表示しています。保存・ファイル操作・認証はTauriアプリで利用できます。</Text>
        <Button component="a" href="https://tauri.app/" target="_blank" rel="noopener noreferrer" size="xs" variant="subtle" rightSection={<Icons.externalLink size={IconSize.inline} />}>Tauriについて</Button>
      </Group>
    </Alert>
  );
}
