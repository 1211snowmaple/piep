import { useEffect, useState } from "react";
import { Button, Card, Group, NumberInput, Stack, Text } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ErrorState, LoadingState } from "@/components/AsyncState";
import { Note } from "@/components/Note";
import { Icons } from "@/lib/icons";
import { errorMessage, formatNumber } from "@/lib/format";
import { applyAdvancedSettings } from "@/lib/advancedSettings";
import {
  defaultAdvancedValues,
  getAdvancedSettings,
  saveAdvancedSettings,
  type AdvancedSettingGroup,
  type AdvancedSettingSpec,
  type AdvancedSettingUnit,
} from "@/services/advancedSettingsApi";
import { SectionIntro } from "./SectionIntro";

const QUERY_KEY = ["advanced-settings"];

const GROUPS: { id: AdvancedSettingGroup; title: string; note?: string }[] = [
  {
    id: "source",
    title: "取得元との通信",
    note: "間隔は既定の半分までしか縮められません。縮めると取得元に断られやすくなり、続けるとアカウントが止められることがあります。",
  },
  { id: "history", title: "履歴" },
  { id: "server", title: "サーバーへの自動送信" },
  { id: "assist", title: "AI" },
  { id: "reading", title: "読む" },
];

/** 項目ごとの説明。名前と範囲は Rust から来る。ここには「変えると何が起きるか」だけ書く。 */
export const ADVANCED_DESCRIPTIONS: Record<string, string> = {
  requestGapMs: "保存・更新の確認・一覧の取得のすべてで、続けて問い合わせるときにあける時間です。",
  updateCheckDelayMs: "長くすると確認に時間がかかるぶん、取得元に断られにくくなります。",
  newWorksDelayMs: "新作だけを見る軽い確認で使います。",
  rateLimitRetries: "取得制限に当たった作品を、待ってから試し直す回数です。0 にすると、すぐ失敗として扱います。",
  consecutiveErrorLimit: "取得制限以外の失敗がこの回数続いたら、確認を一時停止します。",
  deepCheckIntervalDays: "一覧の情報が同じでも、この日数が過ぎた作品は本文まで確かめます。短くすると文字数の変わらない直しに早く気づきますが、取得が増えます。",
  sourceTimeoutSecs: "応答が無いまま、この時間が過ぎたら諦めます。回線が遅いときは長くしてください。",
  assetDownloadConcurrency: "1 にすると、画像の多い作品の保存が遅くなるかわりに、配信元への負担が減ります。",
  repairAttempts: "挿絵や添付が取れなかった作品、作品情報が伏せられていた作品を、同じ状態のまま何回まで取りに行くかです。最初の保存も 1 回に数えます。1 にすると取り直しません。",
  keepUpdateJobs: "終わった確認を、新しい方からこの件数だけ残します。",
  keepUpdateJobDays: "これより古い確認は、次に起動したときに消します。",
  operationJobsKept: "保存や書き出しなどの記録を、新しい方からこの件数だけ残します。",
  autoSendQuietMinutes: "保存の途中で何度も送らないよう、棚の変更が止んでからこれだけ待って送ります。",
  autoSendRetryMinutes: "送れなかったときは、これだけ待ってから試し直します。",
  assistTimeoutSecs: "手元で動かしている遅いモデルでは、長くすると最後まで答えを待てます。全文を送る問い合わせは、短くしても 300 秒は待ちます。",
  readingCharsPerMinute: "読み終わるまでの目安の時間を、この速さで計算します。",
};

/** 保存の単位と、見せる単位の違い。ミリ秒は秒で見せる。 */
function toDisplay(unit: AdvancedSettingUnit, value: number): number {
  return unit === "milliseconds" ? value / 1000 : value;
}

function fromDisplay(unit: AdvancedSettingUnit, value: number): number {
  return unit === "milliseconds" ? Math.round(value * 1000) : Math.round(value);
}

/** 単位の言葉。数えるものは名前の終わりで「件」「本」「回」を分ける。 */
function unitLabel(spec: AdvancedSettingSpec): string {
  switch (spec.unit) {
    case "milliseconds":
    case "seconds":
      return "秒";
    case "minutes":
      return "分";
    case "days":
      return "日";
    case "charsPerMinute":
      return "字/分";
    case "count":
      return spec.label.endsWith("回数") ? "回" : spec.label.endsWith("件数") ? "件" : "本";
  }
}

export function describeValue(spec: AdvancedSettingSpec, value: number): string {
  return `${formatNumber(toDisplay(spec.unit, value))} ${unitLabel(spec)}`;
}

function inRange(spec: AdvancedSettingSpec, value: number | undefined): boolean {
  return value !== undefined && Number.isFinite(value) && value >= spec.min && value <= spec.max;
}

export function AdvancedSection() {
  const queryClient = useQueryClient();
  const settings = useQuery({ queryKey: QUERY_KEY, queryFn: getAdvancedSettings });
  const [draft, setDraft] = useState<Record<string, number | undefined>>({});
  useEffect(() => {
    if (settings.data) setDraft(settings.data.values);
  }, [settings.data]);

  const save = useMutation({
    mutationFn: (values: Record<string, number>) => saveAdvancedSettings(values),
    onSuccess: (saved) => {
      applyAdvancedSettings(saved);
      queryClient.setQueryData(QUERY_KEY, saved);
      notifications.show({ color: "green", message: "詳細設定を保存しました" });
    },
    onError: (error) => notifications.show({ color: "red", title: "保存できませんでした", message: errorMessage(error) }),
  });

  const specs = settings.data?.specs ?? [];
  const valid = specs.every((spec) => inRange(spec, draft[spec.key]));
  const dirty = Boolean(settings.data) && specs.some((spec) => draft[spec.key] !== settings.data?.values[spec.key]);
  const atDefaults = specs.every((spec) => draft[spec.key] === spec.default);

  if (settings.isLoading) return <LoadingState label="詳細設定を読み込んでいます" />;
  if (settings.error) return <ErrorState error={settings.error} retry={() => settings.refetch()} />;

  return <Stack gap="lg">
    <SectionIntro title="詳細設定" description="取得の間隔、やり直しの回数、履歴を残す数などの細かい値です。ふだんは既定のままで構いません。" />
    {GROUPS.map((group) => {
      const items = specs.filter((spec) => spec.group === group.id);
      if (!items.length) return null;
      return <Card key={group.id} p="lg">
        <Stack gap="md">
          <Text fw={700}>{group.title}</Text>
          {group.note && <Note icon={Icons.warning}>{group.note}</Note>}
          {items.map((spec) => <AdvancedField
            key={spec.key}
            spec={spec}
            value={draft[spec.key]}
            onChange={(value) => setDraft((current) => ({ ...current, [spec.key]: value }))}
          />)}
        </Stack>
      </Card>;
    })}
    <Group gap="sm" justify="flex-end">
      <Button variant="default" disabled={atDefaults || save.isPending} onClick={() => setDraft(defaultAdvancedValues(specs))}>既定に戻す</Button>
      <Button disabled={!dirty || !valid} loading={save.isPending} onClick={() => save.mutate(Object.fromEntries(specs.map((spec) => [spec.key, draft[spec.key] as number])))}>保存</Button>
    </Group>
  </Stack>;
}

function AdvancedField({ spec, value, onChange }: { spec: AdvancedSettingSpec; value: number | undefined; onChange: (value: number | undefined) => void }) {
  const millis = spec.unit === "milliseconds";
  const shown = value === undefined ? "" : toDisplay(spec.unit, value);
  const range = `${describeValue(spec, spec.min)}〜${describeValue(spec, spec.max)}`;
  return <Group justify="space-between" align="flex-start" wrap="nowrap" gap="lg">
    <div style={{ minWidth: 0, flex: 1 }}>
      <Text size="sm" fw={650}>{spec.label}</Text>
      <Text size="xs" c="dimmed" mt={2}>{ADVANCED_DESCRIPTIONS[spec.key]}</Text>
      <Text size="xs" c="dimmed" mt={2}>既定 {describeValue(spec, spec.default)}・{range}</Text>
    </div>
    <NumberInput
      w={150}
      hideControls
      aria-label={spec.label}
      value={shown}
      min={toDisplay(spec.unit, spec.min)}
      max={toDisplay(spec.unit, spec.max)}
      step={millis ? 0.1 : 1}
      decimalScale={millis ? 2 : 0}
      allowNegative={false}
      clampBehavior="none"
      suffix={` ${unitLabel(spec)}`}
      // 文を出すと下の項目が押し下がる。範囲は左に書いてあるので、枠の色だけで知らせる。
      error={value === undefined || !inRange(spec, value)}
      onChange={(next) => onChange(typeof next === "number" ? fromDisplay(spec.unit, next) : undefined)}
    />
  </Group>;
}
