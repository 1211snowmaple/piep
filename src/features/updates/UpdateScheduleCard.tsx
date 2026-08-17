import { useEffect, useState } from "react";
import { Box, Card, Group, NumberInput, SegmentedControl, Stack, Switch, Text, Title } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { errorMessage } from "@/lib/format";
import { isTauriRuntime } from "@/services/dbApi";
import {
  describeSchedule,
  loadSchedule,
  saveSchedule,
  updateScheduleDefaults,
  type UpdateScheduleSettings,
} from "@/features/updates/updateSchedule";

/**
 * When the check runs by itself, and what it does with what it finds.
 *
 * Every switch here writes straight through: there is no Save button because
 * there is nothing to lose by applying a preference the moment it is set.
 */
export function UpdateScheduleCard() {
  const runtime = isTauriRuntime();
  const [settings, setSettings] = useState<UpdateScheduleSettings>(updateScheduleDefaults);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void loadSchedule().then((stored) => {
      if (!cancelled) {
        setSettings(stored);
        setLoaded(true);
      }
    });
    return () => { cancelled = true; };
  }, []);

  const apply = (change: Partial<UpdateScheduleSettings>) => {
    const next = { ...settings, ...change };
    setSettings(next);
    saveSchedule(next).catch((error) => {
      notifications.show({ color: "red", title: "設定を保存できません", message: errorMessage(error) });
    });
  };

  return (
    <Card p="lg">
      <Stack gap="md">
        <Box>
          <Title order={3}>自動確認</Title>
          <Text size="sm" c="dimmed">{loaded ? describeSchedule(settings) : "読み込んでいます…"}</Text>
        </Box>

        <Switch
          label="起動時に確認する"
          description="間隔を過ぎていれば、アプリを開いたあと一度だけ実行します"
          checked={settings.onStartup}
          disabled={!loaded}
          onChange={(event) => apply({ onStartup: event.currentTarget.checked })}
        />

        <NumberInput
          label="確認の間隔"
          description="0 で間隔による実行を止めます"
          suffix=" 時間"
          min={0}
          max={336}
          hideControls
          allowDecimal={false}
          allowNegative={false}
          clampBehavior="strict"
          value={settings.intervalHours}
          disabled={!loaded}
          onChange={(value) => apply({ intervalHours: typeof value === "number" ? value : 0 })}
        />

        <Box>
          <Text size="sm" fw={500} mb={6}>自動実行の動作</Text>
          <SegmentedControl
            fullWidth
            aria-label="自動実行の動作"
            data={[{ value: "check_only", label: "確認のみ" }, { value: "auto_save", label: "確認して保存" }]}
            value={settings.mode}
            disabled={!loaded}
            onChange={(value) => apply({ mode: value as UpdateScheduleSettings["mode"] })}
          />
        </Box>

        <Switch
          label="保存した作品を監視に追加"
          description="更新確認で保存した作品を、そのまま追いかけます"
          checked={settings.watchSaved}
          disabled={!loaded}
          onChange={(event) => apply({ watchSaved: event.currentTarget.checked })}
        />

        <Switch
          label="結果を通知する"
          description={runtime ? "確認が終わったら、OSの通知でお知らせします" : "デスクトップアプリでのみ動作します"}
          checked={settings.notify}
          disabled={!loaded || !runtime}
          onChange={(event) => apply({ notify: event.currentTarget.checked })}
        />

        <Group gap={6} wrap="nowrap">
          <Text size="xs" c="dimmed">
            自動確認はアプリを開いている間だけ動きます。閉じている間に過ぎた分は、次に開いたときに一度だけ実行されます。
          </Text>
        </Group>
      </Stack>
    </Card>
  );
}
