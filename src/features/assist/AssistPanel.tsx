import type { ReactNode } from "react";
import { Alert, Button, Card, Group, Loader, Stack, Text } from "@mantine/core";
import { Icons, IconSize } from "@/lib/icons";
import { AssistLauncher } from "@/features/assist/AssistLauncher";
import type { AssistFeatureId, GroundedClaim } from "@/services/assistApi";
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
  claims,
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
  claims?: GroundedClaim[] | null;
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
      {claims && claims.length > 0 ? <GroundedClaims claims={claims} /> : <Text size="sm" style={{ whiteSpace: "pre-wrap" }}>{text}</Text>}
      {sourceExtract && !claims && (
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

const verdictLabel: Record<GroundedClaim["verdict"], string> = {
  supported: "原文で確認",
  partial: "一部だけ確認",
  unsupported: "確認できず",
};

/**
 * 書き下ろしのあらすじ。文ごとに根拠の原文を開いて確かめられる。
 *
 * 照合で支持されなかった文は本文から外すが、消さずに別枠で残す。黙って消すと
 * モデルが何を言ったのかを利用者が確かめられなくなる。
 */
function GroundedClaims({ claims }: { claims: GroundedClaim[] }) {
  const shown = claims.filter((claim) => claim.verdict !== "unsupported");
  const doubtful = claims.filter((claim) => claim.verdict === "unsupported");
  const partial = shown.filter((claim) => claim.verdict === "partial").length;
  return (
    <Stack gap={4}>
      <Text size="sm">
        {shown.map((claim, index) => (
          <span key={index} className={claim.verdict === "partial" ? "assist-claim-partial" : undefined} title={claim.verdict === "partial" ? claim.concern ?? verdictLabel.partial : undefined}>
            {claim.text}
            {claim.verdict === "partial" && <Text component="sup" size="xs" c="dimmed">※</Text>}
          </span>
        ))}
      </Text>
      <Text size="xs" c="dimmed">
        AIが自分の言葉でまとめたあらすじです。文ごとに原文の根拠と照らしています
        {partial > 0 ? `（※の${partial}文は一部だけ確認）` : ""}。照合もAIの判断です。
      </Text>
      <details>
        <Text component="summary" size="xs" c="dimmed" style={{ cursor: "pointer" }}>根拠の原文を見る</Text>
        <Stack gap={6} mt={4}>
          {shown.map((claim, index) => <ClaimEvidence key={index} claim={claim} />)}
        </Stack>
      </details>
      {doubtful.length > 0 && (
        <details>
          <Text component="summary" size="xs" c="dimmed" style={{ cursor: "pointer" }}>原文で確かめられなかった文（{doubtful.length}）</Text>
          <Stack gap={6} mt={4}>
            {doubtful.map((claim, index) => <ClaimEvidence key={index} claim={claim} />)}
          </Stack>
        </details>
      )}
    </Stack>
  );
}

function ClaimEvidence({ claim }: { claim: GroundedClaim }) {
  return (
    <Stack gap={2}>
      <Text size="sm">{claim.text}<Text component="span" size="xs" c="dimmed">　{verdictLabel[claim.verdict]}</Text></Text>
      {claim.concern && <Text size="xs" c="dimmed">指摘: {claim.concern}</Text>}
      {claim.evidence.map((line, index) => (
        <Text key={index} size="xs" c="dimmed" className="assist-claim-evidence" style={{ whiteSpace: "pre-wrap" }}>{line}</Text>
      ))}
    </Stack>
  );
}
