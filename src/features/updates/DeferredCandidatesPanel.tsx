import { useState } from "react";
import { Badge, Button, Card, Checkbox, Group, Select, Stack, Text, TextInput } from "@mantine/core";
import { useMutation, useQuery } from "@tanstack/react-query";
import { notifications } from "@mantine/notifications";
import { listDeferredUpdateCandidatesCommand, recheckDeferredUpdateCandidatesCommand, type DeferredUpdateCandidate } from "@/services/updateJobApi";
import { errorMessage } from "@/lib/format";
import { ErrorState, LoadingState } from "@/components/AsyncState";
import { UpdateWorkLinks } from "./UpdateWorkLinks";

function candidateReason(row: DeferredUpdateCandidate): { reason: string; category: string } {
  let reason = "あとで候補に戻せます";
  let category = row.status === "dismissed" ? "dismissed" : "held";
  try {
    const payload = JSON.parse(row.payloadJson);
    reason = payload.holdReason || reason;
    if (row.status === "held") category = payload.holdKind || "held";
  } catch { /* Older metadata may not be JSON. */ }
  return { reason, category };
}

export function DeferredCandidatesPanel({ runtime, onRechecked, onOpenWork }: { runtime: boolean; onRechecked: (jobId: string) => void; onOpenWork?: (downloadId: number) => void }) {
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [search, setSearch] = useState("");
  const [statusFilter, setStatusFilter] = useState<string | null>("all");
  const query = useQuery({ queryKey: ["deferred-candidates"], queryFn: () => runtime ? listDeferredUpdateCandidatesCommand() : Promise.resolve([]) });
  const mutation = useMutation({
    mutationFn: ({ keys, refreshPostAccess }: { keys: string[]; refreshPostAccess: boolean }) => recheckDeferredUpdateCandidatesCommand(keys, refreshPostAccess),
    onSuccess: (job) => { setSelected(new Set()); onRechecked(job.jobId); },
    onError: (error) => notifications.show({ color: "red", title: "再確認を開始できません", message: errorMessage(error) }),
  });
  if (query.isLoading) return <LoadingState />;
  if (query.error) return <ErrorState error={query.error} retry={() => query.refetch()} />;
  const rows = query.data ?? [];
  const filteredRows = rows.filter((row) => {
    const { reason, category } = candidateReason(row);
    if (statusFilter !== "all" && category !== statusFilter) return false;
    const needle = search.trim().toLocaleLowerCase();
    if (!needle) return true;
    return `${row.title} ${row.source} ${row.sourceId} ${reason}`.toLocaleLowerCase().includes(needle);
  });
  const keys = filteredRows.map((row) => `${row.source}:${row.sourceId}`);
  const selectedKeys = keys.filter((key) => selected.has(key));
  return <Card p="lg"><Stack>
    <Text fw={700}>要確認（{rows.length}件）</Text>
    <Text size="sm" c="dimmed">取得できなかった理由を残しています。必要な作品を選んで再確認できます。</Text>
    <Group grow>
      <TextInput aria-label="保留作品を検索" placeholder="作品名・理由・取得元IDで検索" value={search} onChange={(event) => setSearch(event.currentTarget.value)} />
      <Select aria-label="状態で絞り込み" value={statusFilter} onChange={setStatusFilter} data={[{ value: "all", label: "すべて" }, { value: "incomplete", label: "一部保存" }, { value: "unavailable", label: "見つからない" }, { value: "restricted", label: "閲覧条件待ち" }, { value: "error", label: "取得エラー" }, { value: "dismissed", label: "非表示" }, { value: "held", label: "その他の保留" }]} />
    </Group>
    <Group>
      <Button variant="subtle" size="xs" disabled={!keys.length || mutation.isPending} onClick={() => setSelected(new Set(keys))}>表示分を選択</Button>
      <Button variant="subtle" size="xs" disabled={!selectedKeys.length || mutation.isPending} onClick={() => setSelected(new Set())}>選択解除</Button>
      <Button disabled={!selectedKeys.length} loading={mutation.isPending} onClick={() => mutation.mutate({ keys: selectedKeys, refreshPostAccess: false })}>候補に戻す（{selectedKeys.length}）</Button>
    </Group>
    <Group><Button variant="subtle" size="xs" disabled={!selectedKeys.length || selectedKeys.length > 20 || mutation.isPending} onClick={() => mutation.mutate({ keys: selectedKeys, refreshPostAccess: true })}>公開条件も確認</Button><Text size="xs" c="dimmed">公開条件の確認は20件ずつ</Text></Group>
    {!rows.length && <Text size="sm" c="dimmed">要確認の作品はありません。</Text>}
    {filteredRows.map((row) => {
      const key = `${row.source}:${row.sourceId}`;
      const { reason, category } = candidateReason(row);
      const label = ({ dismissed: "非表示", restricted: "閲覧条件待ち", incomplete: "一部保存", unavailable: "見つからない", error: "取得エラー", held: "保留" } as Record<string, string>)[category] ?? "保留";
      return <Group key={key} wrap="nowrap" align="flex-start">
        <Checkbox aria-label={`${row.title}を再確認`} checked={selected.has(key)} disabled={mutation.isPending} onChange={(event) => {
          const checked = event.currentTarget.checked;
          setSelected((previous) => { const next = new Set(previous); if (checked) next.add(key); else next.delete(key); return next; });
        }} />
        <Stack gap={3} style={{ flex: 1, minWidth: 0 }}><Text size="sm" fw={600}>{row.title}</Text><Text size="xs" c="dimmed">{row.source} · {row.sourceId} · {reason}</Text></Stack>
        <Badge color={category === "dismissed" ? "gray" : category === "restricted" ? "yellow" : "orange"}>{label}</Badge>
        <UpdateWorkLinks source={row.source} sourceId={row.sourceId} downloadId={row.downloadId} title={row.title} onOpenWork={onOpenWork} />
      </Group>;
    })}
    {rows.length > 0 && filteredRows.length === 0 && <Text size="sm" c="dimmed">条件に合う作品はありません。</Text>}
  </Stack></Card>;
}
