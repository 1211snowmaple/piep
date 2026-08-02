import { Alert, Button, Group, Text } from "@mantine/core";
import { AppWindow, ExternalLink } from "lucide-react";
import { isTauriRuntime } from "@/services/dbApi";

export function RuntimeNotice() {
  if (isTauriRuntime()) return null;
  return (
    <Alert color="blue" variant="light" icon={<AppWindow size={18} />} title="ブラウザプレビュー">
      <Group justify="space-between" align="center" wrap="wrap">
        <Text size="sm" flex={1} miw={240}>レイアウト確認用のデモデータを表示しています。保存・ファイル操作・認証はTauriアプリで利用できます。</Text>
        <Button component="a" href="https://tauri.app/" target="_blank" size="xs" variant="subtle" rightSection={<ExternalLink size={13} />}>Tauriについて</Button>
      </Group>
    </Alert>
  );
}
