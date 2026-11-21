import { useEffect, useState, type ReactNode } from "react";
import { ActionIcon, Badge, Box, Button, Card, Divider, Group, Modal, Stack, Tabs, Text, Tooltip, UnstyledButton } from "@mantine/core";
import { modals } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { useInfiniteQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { ErrorState, LoadingState } from "@/components/AsyncState";
import { Icons, IconSize } from "@/lib/icons";
import { errorMessage, formatDate, formatNumber } from "@/lib/format";
import { useAppNavigate } from "@/app/router";
import { useShelfStatus } from "@/features/library/shelf";
import { HISTORY_DETAIL_PAGE, HISTORY_PAGE, setStoredJobHidden, storedHistoryKey, storedJobHistory, storedJobItems, storedJobLogs, type StoredJobHistory } from "@/services/jobHistoryApi";

function date(value: number): string { return formatDate(new Date(value).toISOString(), true); }
function scope(value: string): string { return ({ all: "すべて", work: "作品", author: "作者", series: "シリーズ", targets: "選んだ対象", save: "まとめ保存", favorite: "お気に入り", reading: "読みかけ" } as Record<string, string>)[value] ?? value; }
function mode(value: string): string { return ({ check_only: "確認のみ", auto_save: "自動保存", save: "保存", manual: "手動" } as Record<string, string>)[value] ?? value; }
function counts(job: StoredJobHistory): string {
  return `${formatNumber(job.processed)}/${formatNumber(job.totals)}件 · 候補 ${formatNumber(job.candidate_count)} · 保存 ${formatNumber(job.saved_count)} · エラー ${formatNumber(job.error_count)}`;
}

/** 再開できる実行中の記録と、棚に封をした以前の記録を、同じ履歴タブで開く。 */
export function StoredJobHistoryPanel({ runtime, current, hasCurrent, onShelfReady }: { runtime: boolean; current: ReactNode; hasCurrent: boolean; onShelfReady: () => Promise<void> }) {
  const status = useShelfStatus(runtime);
  const active = runtime && status.data?.state === "ready";
  const queryClient = useQueryClient();
  const [selected, setSelected] = useState<StoredJobHistory | null>(null);
  const history = useInfiniteQuery({
    queryKey: storedHistoryKey,
    enabled: active,
    initialPageParam: null as string | null,
    queryFn: ({ pageParam }) => storedJobHistory(pageParam),
    getNextPageParam: (page) => page.length === HISTORY_PAGE ? page[page.length - 1]?.id : undefined,
  });
  useEffect(() => {
    if (active) void onShelfReady().catch((error) => console.error("現在の実行記録を読み直せません", error));
  }, [active, onShelfReady]);
  const hide = useMutation({
    mutationFn: (job: string | null) => setStoredJobHidden(job, true),
    onSuccess: async (_, job) => {
      setSelected(null);
      await queryClient.invalidateQueries({ queryKey: storedHistoryKey });
      notifications.show({ color: "gray", title: "履歴を一覧から外しました", message: job ? <Button size="compact-xs" variant="default" onClick={() => {
        void setStoredJobHidden(job, false).then(() => queryClient.invalidateQueries({ queryKey: storedHistoryKey })).catch((error) => notifications.show({ color: "red", title: "履歴を戻せません", message: errorMessage(error) }));
      }}>元に戻す</Button> : "記録の原本は棚に保管されています。" });
    },
    onError: (error) => notifications.show({ color: "red", title: "履歴を一覧から外せません", message: errorMessage(error) }),
  });
  const confirmHide = (job: StoredJobHistory | null) => modals.openConfirmModal({
    title: job ? "この履歴を一覧から外しますか？" : "以前の実行記録をすべて一覧から外しますか？",
    children: <Text size="sm">{job ? date(job.started_at) : "現在ある以前の実行記録"}を一覧から外します。記録の原本と保存した作品は棚に保管されます。</Text>,
    labels: { confirm: "一覧から外す", cancel: "やめる" },
    onConfirm: () => hide.mutate(job?.id ?? null),
  });
  if (!active) return current;
  const rows = history.data?.pages.flat() ?? [];
  return <Stack>
    {(hasCurrent || (!history.isPending && !history.error && rows.length === 0)) && current}
    {history.isPending ? <LoadingState /> : history.error ? <ErrorState error={history.error} retry={() => void history.refetch()} /> : rows.length > 0 && <Card p={0}>
      <Group p="md" justify="space-between" wrap="nowrap">
        <Box><Text fw={700}>以前の実行記録</Text><Text size="xs" c="dimmed">実行当時の件数・項目・ログを開きます</Text></Box>
        <Button size="xs" variant="default" loading={hide.isPending} onClick={() => confirmHide(null)}>すべて一覧から外す</Button>
      </Group>
      <Divider />
      <Stack gap={0}>{rows.map((job) => <Group key={job.id} px="md" py="sm" gap="sm" wrap="nowrap" className="update-history__row">
        <UnstyledButton style={{ flex: 1, minWidth: 0 }} aria-label={`${date(job.started_at)}の以前の実行記録を開く`} onClick={() => setSelected(job)}>
          <Group gap="sm" wrap="nowrap"><Badge color="green" variant="light">完了</Badge><Stack gap={2} miw={0}>
            <Text size="sm">{date(job.started_at)} · {scope(job.scope)} · {mode(job.mode)}</Text>
            <Text size="xs" c="dimmed">{counts(job)}</Text>
          </Stack></Group>
        </UnstyledButton>
        <Tooltip label="この履歴を一覧から外す"><ActionIcon variant="subtle" color="gray" size="sm" aria-label={`${date(job.started_at)}の以前の実行記録を一覧から外す`} disabled={hide.isPending} onClick={() => confirmHide(job)}><Icons.cancel size={IconSize.menu} /></ActionIcon></Tooltip>
      </Group>)}</Stack>
      {history.hasNextPage && <Button m="md" variant="default" loading={history.isFetchingNextPage} onClick={() => void history.fetchNextPage()}>以前の記録を読み込む</Button>}
    </Card>}
    <Modal opened={selected !== null} onClose={() => setSelected(null)} title="以前の実行記録" size="lg">
      {selected && <StoredHistoryDetails job={selected} />}
    </Modal>
  </Stack>;
}

function StoredHistoryDetails({ job }: { job: StoredJobHistory }) {
  const navigate = useAppNavigate();
  const [tab, setTab] = useState<string | null>("items");
  const items = useInfiniteQuery({
    queryKey: [...storedHistoryKey, job.id, "items"], enabled: tab === "items",
    initialPageParam: null as number | null, queryFn: ({ pageParam }) => storedJobItems(job.id, pageParam),
    getNextPageParam: (page) => page.length === HISTORY_DETAIL_PAGE ? page[page.length - 1]?.sequence : undefined,
  });
  const logs = useInfiniteQuery({
    queryKey: [...storedHistoryKey, job.id, "logs"], enabled: tab === "logs",
    initialPageParam: null as number | null, queryFn: ({ pageParam }) => storedJobLogs(job.id, pageParam),
    getNextPageParam: (page) => page.length === HISTORY_DETAIL_PAGE ? page[page.length - 1]?.sequence : undefined,
  });
  const labels: Record<string, string> = { done: "確認済み", saved: "保存", skipped: "対象外", held: "保留", failed: "失敗" };
  return <Stack gap="sm">
    <Text size="sm">{date(job.started_at)} · {scope(job.scope)} · {mode(job.mode)}</Text>
    <Text size="sm">{counts(job)}</Text>
    <Text size="xs" c="dimmed">終了：{job.finished_at === null ? "日時の記録なし" : date(job.finished_at)}</Text>
    <Text size="xs" c="dimmed">件数・題名・ログは実行当時の記録です。作品へのリンクは現在の詳細を開きます。</Text>
    <Tabs value={tab} onChange={setTab}>
      <Tabs.List><Tabs.Tab value="items">項目</Tabs.Tab><Tabs.Tab value="logs">ログ</Tabs.Tab></Tabs.List>
      <Tabs.Panel value="items" pt="md"><Stack gap="sm">
        {items.isPending ? <LoadingState /> : items.error ? <ErrorState error={items.error} retry={() => void items.refetch()} /> : <>
          {!items.data?.pages.flat().length && <Text size="sm" c="dimmed">項目の記録はありません。</Text>}
          {items.data?.pages.flat().map((item) => <Box key={item.id}><Group justify="space-between" wrap="nowrap">
            <Text size="sm" style={{ flex: 1, minWidth: 0 }}>{item.title}</Text><Badge color={item.status === "failed" ? "red" : item.status === "held" ? "yellow" : "gray"} variant="light">{labels[item.status] ?? item.status}</Badge>
            {item.result_work && <Button variant="subtle" size="compact-xs" onClick={() => navigate(`/works/${item.result_work}`)}>保存した作品</Button>}
          </Group>{item.error && <Text size="xs" c="dimmed">{item.error}</Text>}</Box>)}
          {items.hasNextPage && <Button variant="default" loading={items.isFetchingNextPage} onClick={() => void items.fetchNextPage()}>次の項目を読み込む</Button>}
        </>}
      </Stack></Tabs.Panel>
      <Tabs.Panel value="logs" pt="md"><Stack gap="sm">
        {logs.isPending ? <LoadingState /> : logs.error ? <ErrorState error={logs.error} retry={() => void logs.refetch()} /> : <>
          {!logs.data?.pages.flat().length && <Text size="sm" c="dimmed">ログの記録はありません。</Text>}
          {logs.data?.pages.flat().map((log) => <Box key={log.id}><Text size="sm" c={log.log_type === "error" ? "red" : undefined}>{log.message}</Text><Text size="xs" c="dimmed">{date(log.created_at)}</Text></Box>)}
          {logs.hasNextPage && <Button variant="default" loading={logs.isFetchingNextPage} onClick={() => void logs.fetchNextPage()}>次のログを読み込む</Button>}
        </>}
      </Stack></Tabs.Panel>
    </Tabs>
  </Stack>;
}
