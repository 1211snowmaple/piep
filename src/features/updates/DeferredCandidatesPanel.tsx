import { useState } from "react";
import { Badge, Button, Card, Checkbox, Group, Stack, Text } from "@mantine/core";
import { useMutation, useQuery } from "@tanstack/react-query";
import { notifications } from "@mantine/notifications";
import { listDeferredUpdateCandidatesCommand, recheckDeferredUpdateCandidatesCommand } from "@/services/updateJobApi";
import { errorMessage } from "@/lib/format";
import { ErrorState, LoadingState } from "@/components/AsyncState";

export function DeferredCandidatesPanel({ runtime, onRechecked }: { runtime: boolean; onRechecked: (jobId: string) => void }) {
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const query = useQuery({ queryKey: ["deferred-candidates"], queryFn: () => runtime ? listDeferredUpdateCandidatesCommand() : Promise.resolve([]) });
  const mutation = useMutation({
    mutationFn: ({ keys, refreshPostAccess }: { keys: string[]; refreshPostAccess: boolean }) => recheckDeferredUpdateCandidatesCommand(keys, refreshPostAccess),
    onSuccess: (job) => { setSelected(new Set()); onRechecked(job.jobId); },
    onError: (error) => notifications.show({ color: "red", title: "再確認を開始できません", message: errorMessage(error) }),
  });
  if (query.isLoading) return <LoadingState />;
  if (query.error) return <ErrorState error={query.error} retry={() => query.refetch()} />;
  const rows = query.data ?? [];
  const keys = rows.map((row) => `${row.source}:${row.sourceId}`);
  const selectedKeys = keys.filter((key) => selected.has(key));
  return <Card p="lg"><Stack>
    <Text fw={700}>保留・非表示の作品（{rows.length}件）</Text>
    <Text size="sm" c="dimmed">ここにある作品は通常の更新確認・再試行では取得しません。削除はされず、期限もありません。支援プランや連携アカウントの変更後に再確認できます。閲覧可能な作品だけ候補へ戻し、保存や監視登録は行いません。</Text>
    <Group>
      <Button variant="subtle" size="xs" disabled={!keys.length || mutation.isPending} onClick={() => setSelected(new Set(keys))}>すべて選択</Button>
      <Button variant="subtle" size="xs" disabled={!selectedKeys.length || mutation.isPending} onClick={() => setSelected(new Set())}>選択解除</Button>
      <Button disabled={!selectedKeys.length} loading={mutation.isPending} onClick={() => mutation.mutate({ keys: selectedKeys, refreshPostAccess: false })}>権限変更後に再確認（{selectedKeys.length}件）</Button>
    </Group>
    <Group><Text size="xs" c="dimmed">無料公開への変更など、投稿側の条件が変わった場合はこちら。選んだ投稿を1件ずつ取得して確認します（20件まで）。</Text><Button variant="default" size="xs" disabled={!selectedKeys.length || selectedKeys.length > 20 || mutation.isPending} onClick={() => mutation.mutate({ keys: selectedKeys, refreshPostAccess: true })}>投稿の公開条件も再確認</Button></Group>
    {!rows.length && <Text size="sm" c="dimmed">保留・非表示の作品はありません。</Text>}
    {rows.map((row) => {
      const key = `${row.source}:${row.sourceId}`;
      let reason = "あとで候補に戻せます";
      try { reason = JSON.parse(row.payloadJson).holdReason || reason; } catch { /* Old malformed metadata must not break the shelf. */ }
      return <Group key={key} wrap="nowrap" align="flex-start">
        <Checkbox aria-label={`${row.title}を再確認`} checked={selected.has(key)} disabled={mutation.isPending} onChange={(event) => {
          const checked = event.currentTarget.checked;
          setSelected((previous) => { const next = new Set(previous); if (checked) next.add(key); else next.delete(key); return next; });
        }} />
        <Stack gap={3} style={{ flex: 1, minWidth: 0 }}><Text size="sm" fw={600}>{row.title}</Text><Text size="xs" c="dimmed">{row.source} · {row.sourceId} · {reason}</Text></Stack>
        <Badge color="yellow">{row.status === "held" ? "閲覧条件待ち" : "非表示"}</Badge>
      </Group>;
    })}
  </Stack></Card>;
}
