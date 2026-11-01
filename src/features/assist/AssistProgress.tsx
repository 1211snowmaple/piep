import { useEffect, useState } from "react";
import { Group, Loader, Stack, Text } from "@mantine/core";
import { formatDuration } from "@/lib/format";
import type { AssistFeatureId, AssistProgressEvent } from "@/services/assistApi";
import { subscribeTauriEvent } from "@/services/eventBus";

const STAGE_LABELS: Record<AssistProgressEvent["stage"], string> = {
  reading: "長い本文を区切って読んでいます",
  combining: "区切りごとのメモから、全体をまとめています",
  drafting: "本文を読んで、下書きを作っています",
  checking: "下書きの文を、一つずつ原文と照らしています",
  revising: "原文と合わなかった文を直しています",
  rechecking: "直した文を、もう一度原文と照らしています",
  writing: "本文から要点の文を選んでいます",
};

/** いまの段階を一文にする。 */
export function assistStageText(progress: AssistProgressEvent | null): string {
  if (!progress) return "準備しています";
  const section = progress.section && progress.sections ? `（${progress.section}/${progress.sections}）` : "";
  const retry = progress.retry ? "。答えの形が崩れたので、頼み直しています" : "";
  return `${STAGE_LABELS[progress.stage]}${section}${retry}`;
}

/**
 * あらすじ・前回のあらすじを作っているあいだの、段階と経過時間。
 *
 * 長い本文では一作に数分かかる。読み込み中の印だけでは、止まったのか
 * 進んでいるのか分からないので、モデルにいま何を頼んでいるかを出す。
 */
export function AssistProgressLine({
  feature,
  downloadId,
  active,
}: {
  feature: AssistFeatureId;
  downloadId: number;
  active: boolean;
}) {
  const [progress, setProgress] = useState<AssistProgressEvent | null>(null);
  const [startedAt, setStartedAt] = useState<number | null>(null);
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    if (!active) return undefined;
    setProgress(null);
    setStartedAt(Date.now());
    setNow(Date.now());
    const unlisten = subscribeTauriEvent<AssistProgressEvent>("assist-progress", ({ payload }) => {
      if (payload.feature === feature && payload.downloadId === downloadId) setProgress(payload);
    });
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => {
      unlisten();
      window.clearInterval(timer);
    };
  }, [active, feature, downloadId]);

  if (!active) return null;
  const elapsed = startedAt === null ? 0 : Math.max(0, (now - startedAt) / 1000);
  return (
    <Stack gap={2} role="status" aria-live="polite">
      <Group gap="xs" wrap="nowrap">
        <Loader size="xs" />
        <Text size="sm">{assistStageText(progress)}</Text>
      </Group>
      <Text size="xs" c="dimmed">
        {formatDuration(elapsed)}経過。長い作品では数分かかります。
      </Text>
    </Stack>
  );
}
