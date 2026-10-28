import { Stack, Switch, Text } from "@mantine/core";
import { optionalLibraryShelves, useLibrarySidebarVisibility } from "@/app/librarySidebarPreferences";

export function LibrarySidebarSettings() {
  const [visible, setVisible] = useLibrarySidebarVisibility();
  return <Stack gap="sm"><div><Text fw={700}>サイドバーのライブラリ</Text><Text size="sm" c="dimmed">「すべて」は常に表示します。</Text></div>
    {optionalLibraryShelves.map(({ value, label }) => <Switch key={value} label={label} checked={visible[value]} onChange={(event) => setVisible(value, event.currentTarget.checked)} />)}
  </Stack>;
}
