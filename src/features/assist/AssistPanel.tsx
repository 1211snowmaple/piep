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
  native = false,
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
  /** 外部モデルを使わない端末内の処理か。 */
  native?: boolean;
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
            <Text size="xs" c="dimmed">{native ? "集計しています…" : "考えてもらっています…"}</Text>
          </Group>
        )}

        {children}
      </Stack>
    </Card>
  );
}

/**
 * モデルによる生成・抽出の結果であることを、見て分かるようにする印。
 *
 * 取得元の文と混ざると、どこまでが事実でどこからが推測か分からなくなる。
 */
export function AssistNoteBody({
  text,
  sourceTail,
  modelId,
  createdAt,
  promptVersion,
  promptStale = false,
  inputStale = false,
  configStale,
  onDiscard,
}: {
  text: string;
  sourceTail?: string | null;
  modelId?: string;
  createdAt?: string;
  promptVersion?: string;
  promptStale?: boolean;
  inputStale?: boolean;
  configStale?: boolean | null;
  onDiscard?: () => void;
}) {
  const nativeStatistics = modelId === "piep:tag-statistics";
  const sourceExtract = ["work_synopsis/v9", "reader_recap/v9", "work_synopsis/v10", "reader_recap/v10", "work_synopsis/v11", "reader_recap/v11"].includes(promptVersion ?? "");
  return (
    <Stack gap={4} className="assist-note">
      <Text size="sm" style={{ whiteSpace: "pre-wrap" }}>{text}</Text>
      {sourceExtract && (
        <Text size="xs" c="dimmed">
          原文から選んだ抜粋です。省略箇所の前後がつながるとは限らず、要点や結末を網羅していません。
        </Text>
      )}
      {sourceTail && (
        <details>
          <Text component="summary" size="xs" c="dimmed" style={{ cursor: "pointer" }}>本文末尾の原文を確認</Text>
          <Text size="xs" c="dimmed">本文末尾の原文は確認用です。重要箇所や結末とは限りません。</Text>
          <Text size="sm" style={{ whiteSpace: "pre-wrap" }}>{sourceTail}</Text>
        </details>
      )}
      <Group gap="xs" justify="space-between">
        <Text size="xs" c="dimmed">
          {nativeStatistics ? "対象作品のタグ集計です" : "AIによるメモです"}{modelId && !nativeStatistics ? ` · ${modelId}` : ""}
          {createdAt ? ` · ${new Date(createdAt).toLocaleDateString("ja-JP")}` : ""}
          {promptVersion ? ` · ${nativeStatistics ? "集計条件" : "指示"} ${promptVersion}` : ""}
          {promptStale ? (nativeStatistics ? " · 集計条件が更新されています" : " · 指示が更新されています") : ""}
          {inputStale ? " · 元の作品情報が変わっています" : ""}
          {configStale === true ? (nativeStatistics ? " · 集計条件が変わっています" : " · モデル設定が変わっています") : ""}
          {configStale === null ? (nativeStatistics ? " · 集計条件の鮮度は未確認" : " · モデル設定の鮮度は未確認") : ""}
        </Text>
        {onDiscard && (
          <Button size="compact-xs" variant="subtle" color="gray" onClick={onDiscard}>消す</Button>
        )}
      </Group>
    </Stack>
  );
}
