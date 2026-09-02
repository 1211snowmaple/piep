import type { ReactNode } from "react";
import { Alert, Button, Card, Group, Loader, Stack, Text } from "@mantine/core";
import { Icons, IconSize } from "@/lib/icons";
import { AssistLauncher } from "@/features/assist/AssistLauncher";
import type { AssistFeatureId } from "@/services/assistApi";
import { usePageAssist } from "@/app/PageAssistContext";

/**
 * モデルに何かを頼むところの、共通の見た目。
 *
 * 三つの約束を、形で守らせるための部品である。
 *
 * 1. 設定していなければ新しく頼むボタンは出さない。ただし保存済みの結果は残す
 * 2. **押したときだけ動く。** 開いただけでは何も起きない
 * 3. **返ってきたものは案。** 採る操作は別に置き、既定では何も変わらない
 *
 * 本文を送る仕事は `needsBody` を立てる。許可が無いときは、頼めない理由と
 * どこで許可するかを出す — 黙って消えると、機能が無いのか壊れているのか
 * 分からない。
 */
export function AssistPanel({
  title,
  featureId,
  hint,
  engineReady,
  needsBody = false,
  bodyAllowed = false,
  busy,
  actionLabel,
  onRun,
  disabled,
  placement = "local",
  registrationKey,
  children,
}: {
  title: string;
  featureId: AssistFeatureId;
  hint: string;
  /** モデルの手伝いが使える状態か。使えないときは新規実行だけを隠す。 */
  engineReady: boolean;
  /** 本文を送る仕事か。 */
  needsBody?: boolean;
  bodyAllowed?: boolean;
  busy: boolean;
  actionLabel: string;
  onRun: () => void;
  disabled?: boolean;
  placement?: "local" | "header";
  registrationKey?: string;
  /** 返ってきた案。無ければ何も渡さない。 */
  children?: ReactNode;
}) {
  const blockedByBody = needsBody && !bodyAllowed;
  const launcherItems = [{
    id: featureId,
    label: actionLabel,
    description: hint,
    enabled: engineReady && !busy && !disabled && !blockedByBody,
    unavailableReason: busy
      ? "処理中です"
      : blockedByBody
        ? "設定で本文の送信を許可してください"
        : engineReady
          ? "現在は実行できません"
          : "設定でこの機能を有効にしてください",
    onSelect: onRun,
  }];
  const headerHosted = usePageAssist(
    registrationKey ?? `assist-${featureId}`,
    `${title}の手伝い`,
    launcherItems,
    placement === "header",
  );
  const launcher = (
    <AssistLauncher size="lg" label={`${title}の手伝い`} items={launcherItems} />
  );

  // Before a result exists, the feature occupies one icon only. Availability,
  // the explanation, and the settings link all live in its menu.
  if (!children && !busy) return headerHosted && placement === "header" ? null : <Group justify="flex-end">{launcher}</Group>;

  return (
    <Card withBorder padding="md" className="assist-panel">
      <Stack gap="sm">
        <Group justify="space-between" align="flex-start" wrap="nowrap">
          <div>
            <Group gap={6}>
              <Icons.optimize size={IconSize.action} />
              <Text fw={650} size="sm">{title}</Text>
            </Group>
            <Text size="xs" c="dimmed">{hint}</Text>
          </div>
          {(!headerHosted || placement === "local") && launcher}
        </Group>

        {blockedByBody && (
          <Alert color="gray" icon={<Icons.secure size={IconSize.action} />} p="xs">
            <Text size="xs">
              これは本文をモデルへ送ります。設定の「AIの手伝い」で
              <b>「本文を送ることも許可する」</b>を入れてください。
            </Text>
          </Alert>
        )}

        {busy && (
          <Group gap={6}>
            <Loader size="xs" />
            <Text size="xs" c="dimmed">考えてもらっています…</Text>
          </Group>
        )}

        {children}
      </Stack>
    </Card>
  );
}

/**
 * モデルが書いた文であることを、見て分かるようにする印。
 *
 * 取得元の文と混ざると、どこまでが事実でどこからが推測か分からなくなる。
 */
export function AssistNoteBody({
  text,
  modelId,
  createdAt,
  promptVersion,
  promptStale = false,
  inputStale = false,
  onDiscard,
}: {
  text: string;
  modelId?: string;
  createdAt?: string;
  promptVersion?: string;
  promptStale?: boolean;
  inputStale?: boolean;
  onDiscard?: () => void;
}) {
  return (
    <Stack gap={4} className="assist-note">
      <Text size="sm">{text}</Text>
      <Group gap="xs" justify="space-between">
        <Text size="xs" c="dimmed">
          モデルが書いた文です{modelId ? ` · ${modelId}` : ""}
          {createdAt ? ` · ${new Date(createdAt).toLocaleDateString("ja-JP")}` : ""}
          {promptVersion ? ` · 指示 ${promptVersion}` : ""}
          {promptStale ? " · 指示が更新されています" : ""}
          {inputStale ? " · 元の作品情報が変わっています" : ""}
        </Text>
        {onDiscard && (
          <Button size="compact-xs" variant="subtle" color="gray" onClick={onDiscard}>消す</Button>
        )}
      </Group>
    </Stack>
  );
}
