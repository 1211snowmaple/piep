import { useState } from "react";
import { Alert, Box, Button, Card, Code, Group, Modal, Stack, Text } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ActionRow } from "@/components/ActionRow";
import { DangerButton } from "@/components/DangerButton";
import { Icons, IconSize } from "@/lib/icons";
import { errorMessage, formatBytes, formatDate, formatNumber } from "@/lib/format";
import { openSingleDialog } from "@/services/dialogApi";
import { startOperation } from "@/features/jobs/operationJobs";
import {
  libraryV2Backup,
  libraryV2CancelRestore,
  libraryV2ImportArchive,
  libraryV2InspectBackup,
  libraryV2RestoreState,
  libraryV2ScheduleRestore,
  type ShelfArchiveImport,
  type ShelfBackupInspection,
} from "@/services/shelfBackupApi";

/** 取り込みの結果を一文にする。数の無い項目は言わない。 */
export function archiveImportMessage(result: ShelfArchiveImport): string {
  const parts = [
    result.added ? `${formatNumber(result.added)}作品を足しました` : "",
    result.restored ? `${formatNumber(result.restored)}作品を棚に戻しました` : "",
    result.already ? `${formatNumber(result.already)}作品はすでに棚にあります` : "",
    result.conflicts ? `${formatNumber(result.conflicts)}作品は同じ作品が別に保存し直されているため足していません` : "",
  ].filter(Boolean);
  return parts.length ? `${parts.join("。")}。` : "取り込む作品はありませんでした。";
}

function backupDate(date: Date): string {
  return [
    date.getFullYear(),
    String(date.getMonth() + 1).padStart(2, "0"),
    String(date.getDate()).padStart(2, "0"),
  ].join("-");
}

/** 区切り文字はフォルダーの書き方に合わせる（Windows は `\`）。 */
function join(directory: string, name: string): string {
  const separator = directory.includes("\\") && !directory.includes("/") ? "\\" : "/";
  return `${directory.replace(/[\\/]+$/, "")}${separator}${name}`;
}

/**
 * 新しい棚のバックアップと復元。
 *
 * バックアップは棚そのものの写し（フォルダー一つ）で、作ったときに全体を検査する。復元は開いている棚を
 * 入れ替えることになるので、検査を通った写しを予約し、次に起動したときに入れ替える。今の棚は消さずに
 * 隣へ退け、選んだ写しも手元に残る。
 *
 * 作者・シリーズのアーカイブ（棚の部分写し）を選んだときは、入れ替えずに今の棚へ取り込む。今の棚に
 * 無い作品だけを足し、ある作品・記録は変えない。切り替えた棚にだけ取り込める。
 */
export function ShelfBackupCard({ runtime, busy }: { runtime: boolean; busy: boolean }) {
  const queryClient = useQueryClient();
  const [review, setReview] = useState<ShelfBackupInspection | null>(null);
  const state = useQuery({ queryKey: ["library-v2-restore-state"], queryFn: libraryV2RestoreState, enabled: runtime });
  const refresh = () => queryClient.invalidateQueries({ queryKey: ["library-v2-restore-state"] });

  const backup = useMutation({
    mutationFn: async () => {
      const directory = await openSingleDialog({ directory: true, title: "バックアップを置くフォルダー" });
      if (!directory) return null;
      const destination = join(directory, `piep-library-backup-${backupDate(new Date())}-${Date.now().toString(36)}`);
      const operation = startOperation({ kind: "backup", label: "棚のバックアップ", detail: destination, total: null, onRetry: () => backup.mutate() });
      operation.progress(0, null, "棚を写して検査しています");
      try {
        const path = await libraryV2Backup(destination);
        operation.complete("棚のバックアップを作りました");
        return path;
      } catch (error) {
        operation.fail(error);
        throw error;
      }
    },
    onSuccess: (path) => { if (path) notifications.show({ color: "green", title: "棚のバックアップを作りました", message: path }); },
    onError: (error) => notifications.show({ color: "red", title: "バックアップを作れません", message: errorMessage(error) }),
  });

  const inspect = useMutation({
    mutationFn: async () => {
      const path = await openSingleDialog({ directory: true, title: "復元に使う棚のバックアップ・取り込むアーカイブ" });
      if (!path) return null;
      return libraryV2InspectBackup(path);
    },
    onSuccess: (inspection) => { if (inspection) setReview(inspection); },
    onError: (error) => notifications.show({ color: "red", title: "バックアップを検査できません", message: errorMessage(error) }),
  });

  const schedule = useMutation({
    mutationFn: (path: string) => libraryV2ScheduleRestore(path),
    onSuccess: async () => {
      setReview(null);
      await refresh();
      notifications.show({ color: "green", title: "復元を予約しました", message: "次に起動したとき、棚を入れ替えます。" });
    },
    onError: (error) => notifications.show({ color: "red", title: "復元を予約できません", message: errorMessage(error) }),
  });

  const importArchive = useMutation({
    mutationFn: async (path: string) => {
      const operation = startOperation({ kind: "restore", label: "アーカイブの取り込み", detail: path, total: null });
      operation.progress(0, null, "アーカイブを検査して、棚に無い作品を足しています");
      try {
        const result = await libraryV2ImportArchive(path);
        operation.complete(archiveImportMessage(result));
        return result;
      } catch (error) {
        operation.fail(error);
        throw error;
      }
    },
    onSuccess: async (result) => {
      setReview(null);
      // 棚の中身が変わったので、一覧・数・作者などを読み直す。
      await queryClient.invalidateQueries();
      notifications.show({ color: result.conflicts ? "yellow" : "green", title: "アーカイブを取り込みました", message: archiveImportMessage(result) });
    },
    onError: (error) => notifications.show({ color: "red", title: "アーカイブを取り込めません", message: errorMessage(error) }),
  });

  const cancel = useMutation({
    mutationFn: libraryV2CancelRestore,
    onSuccess: async () => { await refresh(); },
    onError: (error) => notifications.show({ color: "red", title: "予約を取り消せません", message: errorMessage(error) }),
  });

  const pending = state.data?.pending ?? null;
  const last = state.data?.last ?? null;
  const working = busy || backup.isPending || inspect.isPending || schedule.isPending || importArchive.isPending;
  const applying = schedule.isPending || importArchive.isPending;
  return (
    <Card p="lg">
      <Stack gap="md">
        <Box>
          <Text fw={700}>バックアップと復元</Text>
          <Text size="sm" c="dimmed">棚そのものをフォルダー一つに写し、作ったときに中身をすべて検査します。復元は、検査を通った写しを予約し、次に起動したときに棚を入れ替えます。今の棚は消さずに隣へ退けます。作者・シリーズのアーカイブを選んだときは、入れ替えずに、棚に無い作品だけを足します。</Text>
        </Box>
        {pending && (
          <Alert color="blue" title="復元を予約しています">
            <Text size="sm">次に起動したとき、この写しで棚を入れ替えます。</Text>
            <Code block mt={6}>{pending.backup}</Code>
            <Button mt="sm" size="xs" variant="default" loading={cancel.isPending} onClick={() => cancel.mutate()}>予約を取り消す</Button>
          </Alert>
        )}
        {!pending && last && (last.error
          ? <Alert color="red" title="前回の復元は行えませんでした">{last.error}</Alert>
          : <Alert color="green" title={`${formatDate(new Date(last.finishedAt * 1000).toISOString(), true)}に棚を復元しました`}>{last.previous ? <><Text size="sm">前の棚は次の場所に残しています。</Text><Code block mt={6}>{last.previous}</Code></> : null}</Alert>)}
        <ActionRow>
          <Button disabled={!runtime || working || Boolean(pending)} loading={inspect.isPending} variant="default" leftSection={<Icons.import size={IconSize.menu} />} onClick={() => inspect.mutate()}>バックアップから復元</Button>
          <Button disabled={!runtime || working} loading={backup.isPending} variant="light" leftSection={<Icons.export size={IconSize.menu} />} onClick={() => backup.mutate()}>棚のバックアップを作る</Button>
        </ActionRow>
      </Stack>
      <Modal opened={Boolean(review)} onClose={() => setReview(null)} withCloseButton={!applying} closeOnClickOutside={!applying} closeOnEscape={!applying} title={review?.partial ? "アーカイブの取り込み" : "棚の復元"} size="lg">
        {review?.partial && (
          <Stack gap="sm">
            <Text size="sm">作者・シリーズのアーカイブです。検査を通りました。今の棚に無い作品だけを足し、棚にある作品・編集・読書位置はそのままにします。</Text>
            <Code block>{review.path}</Code>
            <Group gap="lg">
              <Text size="sm">作品 {formatNumber(review.works)}件</Text>
              <Text size="sm">ファイル {formatNumber(review.files)}個</Text>
              <Text size="sm">{formatBytes(review.bytes)}</Text>
            </Group>
            {!review.sameLibrary && <Alert color="yellow">今の棚とは別の棚から作ったアーカイブなので、取り込めません。</Alert>}
            <ActionRow>
              <Button variant="default" disabled={importArchive.isPending} onClick={() => setReview(null)}>キャンセル</Button>
              <Button disabled={!review.sameLibrary} loading={importArchive.isPending} leftSection={<Icons.import size={IconSize.menu} />} onClick={() => importArchive.mutate(review.path)}>この棚に取り込む</Button>
            </ActionRow>
          </Stack>
        )}
        {review && !review.partial && (
          <Stack gap="sm">
            <Text size="sm">検査を通りました。次に起動したとき、今の棚をこの写しで入れ替えます。</Text>
            <Code block>{review.path}</Code>
            <Group gap="lg">
              <Text size="sm">作品 {formatNumber(review.works)}件</Text>
              <Text size="sm">ファイル {formatNumber(review.files)}個</Text>
              <Text size="sm">{formatBytes(review.bytes)}</Text>
            </Group>
            {!review.sameLibrary && <Alert color="yellow">今の棚とは別の棚の写しです。入れ替えると、今の棚にある作品・編集・読書位置は、この写しの内容になります。</Alert>}
            {!review.switched && <Alert color="yellow">正本に切り替える前の写しです。この写しでは棚を入れ替えられません。</Alert>}
            <Text size="xs" c="dimmed">今の棚は消さずに隣へ退けます。選んだ写しもそのまま残ります。</Text>
            <ActionRow>
              <Button variant="default" disabled={schedule.isPending} onClick={() => setReview(null)}>キャンセル</Button>
              <DangerButton disabled={!review.switched} loading={schedule.isPending} leftSection={<Icons.import size={IconSize.menu} />} onClick={() => schedule.mutate(review.path)}>次の起動で入れ替える</DangerButton>
            </ActionRow>
          </Stack>
        )}
      </Modal>
    </Card>
  );
}
