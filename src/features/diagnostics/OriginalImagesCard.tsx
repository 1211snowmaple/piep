import { useEffect, useRef, useState } from "react";
import { Badge, Box, Button, Card, Group, Progress, Text, ThemeIcon } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { useMutation, useQuery } from "@tanstack/react-query";
import { DangerButton } from "@/components/DangerButton";
import { Icons, IconSize } from "@/lib/icons";
import { useAppNavigate } from "@/app/router";
import { requestOperationCancel, startOperation, type OperationController } from "@/features/jobs/operationJobs";
import { errorMessage, formatNumber } from "@/lib/format";
import {
  cancelOriginalImageUpgrade,
  getOriginalImageStatus,
  upgradePixivImagesToOriginal,
  type OriginalImageProgress,
} from "@/services/dbApi";
import { onTauriEvent, subscribeTauriEvent } from "@/services/eventBus";

/**
 * 縮小版で保存された pixiv の表紙・挿絵を、原寸に取り直す。
 *
 * 以前の保存は表紙を 240x480 の縮小版で、本文から参照したイラストを
 * 1200px の版で持っていた。取りに行くのは画像サーバーだけで、
 * アカウントの通信は増えない。
 */
export function OriginalImagesCard({ runtime, disabled }: { runtime: boolean; disabled: boolean }) {
  const navigate = useAppNavigate();
  const operationRef = useRef<OperationController | null>(null);
  const retryRef = useRef<() => void>(() => undefined);
  const [liveProgress, setLiveProgress] = useState<OriginalImageProgress | null>(null);
  const status = useQuery({
    queryKey: ["original-image-status"],
    queryFn: getOriginalImageStatus,
    enabled: runtime,
    staleTime: 0,
    refetchInterval: (query) => query.state.data?.running ? 2_000 : false,
  });
  const refetchStatus = status.refetch;
  useEffect(() => {
    if (!runtime) return;
    return subscribeTauriEvent<OriginalImageProgress>("original-image-upgrade-progress", ({ payload }) => {
      setLiveProgress(payload);
      if (payload.phase === "complete" || payload.phase === "canceled" || payload.phase === "paused") void refetchStatus();
    });
  }, [runtime, refetchStatus]);

  const cancel = () => {
    const operation = operationRef.current;
    if (operation) { void requestOperationCancel(operation.id); return; }
    void cancelOriginalImageUpgrade().catch((error) =>
      notifications.show({ color: "red", title: "中止できません", message: errorMessage(error) }));
  };

  const upgrade = useMutation({
    mutationFn: async () => {
      const total = status.data ? status.data.covers + status.data.illustrations : null;
      operationRef.current = startOperation({
        kind: "maintenance",
        label: "pixiv の画像を原寸に取り直す",
        detail: "縮小版で保存された表紙と挿絵を、原寸に置き換えます",
        total,
        onCancel: async () => { await cancelOriginalImageUpgrade(); },
        onRetry: () => retryRef.current(),
      });
      const dispose = await onTauriEvent<OriginalImageProgress>("original-image-upgrade-progress", ({ payload }) => {
        setLiveProgress(payload);
        operationRef.current?.progress(
          payload.completed,
          payload.total,
          payload.activeLabel ? `確認中: ${payload.activeLabel}` : undefined,
        );
        if (payload.error) operationRef.current?.log(`${payload.activeLabel ?? "画像"}: ${payload.error}`, "warn");
      });
      try {
        return await upgradePixivImagesToOriginal();
      } catch (error) {
        operationRef.current?.fail(error);
        throw error;
      } finally {
        dispose();
      }
    },
    onSuccess: async (result) => {
      const summary = `原寸 ${formatNumber(result.upgraded)}枚`
        + (result.unavailable ? `、公開元に無く縮小版のまま ${formatNumber(result.unavailable)}枚` : "")
        + (result.failed ? `、失敗 ${formatNumber(result.failed)}枚` : "");
      if (result.stoppedReason) operationRef.current?.fail(result.stoppedReason);
      else if (result.canceled) operationRef.current?.cancel(`${formatNumber(result.attempted)}枚を確認した時点で中止しました（${summary}）`);
      else operationRef.current?.complete(summary);
      operationRef.current = null;
      setLiveProgress(null);
      await status.refetch();
      notifications.show({
        color: result.stoppedReason ? "yellow" : result.canceled ? "gray" : "green",
        title: result.stoppedReason ? "取り直しを一時停止しました" : result.canceled ? "取り直しを中止しました" : "画像の取り直しが終わりました",
        message: result.stoppedReason ?? `${summary}。`,
      });
    },
    onError: (error) => {
      operationRef.current = null;
      setLiveProgress(null);
      notifications.show({ color: "red", title: "画像を取り直せません", message: errorMessage(error) });
    },
  });
  retryRef.current = () => upgrade.mutate();

  const data = status.data;
  const progress = liveProgress ?? data?.progress ?? null;
  const running = upgrade.isPending || Boolean(data?.running);
  const remaining = data ? data.covers + data.illustrations : 0;
  if (!data || (remaining === 0 && !progress)) return null;
  return <Card p="lg" mt="lg" className="maintenance-integrity-card">
    <Group justify="space-between" align="flex-start" wrap="nowrap">
      <Group align="flex-start" wrap="nowrap" miw={0}>
        <ThemeIcon variant="light" color={remaining ? "yellow" : "green"}>{remaining ? <Icons.warning size={IconSize.menu} /> : <Icons.success size={IconSize.menu} />}</ThemeIcon>
        <Box miw={0}>
          <Group gap="xs">
            <Text fw={720}>pixiv の画像の画質</Text>
            <Badge color={remaining ? "yellow" : "green"} variant="light">{remaining ? `${formatNumber(remaining)}枚 縮小版` : "原寸"}</Badge>
          </Group>
          <Text size="xs" c="dimmed" mt={5}>{remaining
            ? "縮小版で保存された表紙と挿絵を、原寸に取り直して置き換えます。版は増えません。公開元で消えた画像は縮小版のまま残ります。"
            : "保存済みの表紙と挿絵はすべて原寸です。"}</Text>
          {!running && remaining > 0 && <Text size="xs" c="dimmed" mt="xs">表紙 {formatNumber(data.covers)} · 挿絵 {formatNumber(data.illustrations)}</Text>}
        </Box>
      </Group>
      {remaining > 0 && (running
        ? <DangerButton style={{ flexShrink: 0 }} leftSection={<Icons.stop size={IconSize.menu} />} onClick={cancel}>中止</DangerButton>
        : <Button style={{ flexShrink: 0 }} leftSection={<Icons.retry size={IconSize.menu} />} disabled={!runtime || disabled} onClick={() => upgrade.mutate()}>原寸に取り直す</Button>)}
    </Group>
    {progress && <Box mt="md">
      <Group justify="space-between" mb={6} wrap="nowrap">
        <Text size="xs" c="dimmed" className="line-clamp-1" title={progress.activeLabel ?? undefined}>{progress.activeLabel
          ? `${progress.phase === "checked" ? "確認済み" : "確認中"}: ${progress.activeLabel}`
          : running ? "取り直しを準備しています" : progress.phase === "canceled" ? "停止しました" : progress.phase === "paused" ? "一時停止しました" : "今回の取り直しが完了しました"}</Text>
        <Text size="xs" c="dimmed" style={{ flexShrink: 0 }}>{formatNumber(progress.completed)} / {formatNumber(progress.total)}</Text>
      </Group>
      <Progress value={progress.total ? progress.completed / progress.total * 100 : 0} animated={running} />
      <Text size="xs" c="dimmed" mt={6}>原寸 {formatNumber(progress.upgraded)}枚 · 公開元に無し {formatNumber(progress.unavailable)}枚 · 失敗 {formatNumber(progress.failed)}枚</Text>
      {progress.error && <Text size="xs" c="red" mt={6}>{progress.error}</Text>}
      {progress.failed > 0 && <Button size="xs" variant="subtle" mt={6} onClick={() => navigate("/operations")}>失敗の詳細</Button>}
    </Box>}
  </Card>;
}
