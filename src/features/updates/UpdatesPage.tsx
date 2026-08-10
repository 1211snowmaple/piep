import { useEffect, useRef, useState } from "react";
import {
  ActionIcon,
  Alert,
  Badge,
  Box,
  Button,
  Card,
  Checkbox,
  Divider,
  Grid,
  Group,
  NumberInput,
  Paper,
  Progress,
  SegmentedControl,
  Select,
  Stack,
  Switch,
  Tabs,
  Text,
  TextInput,
  ThemeIcon,
  Timeline,
  Title,
  Tooltip,
} from "@mantine/core";
import { useForm, isNotEmpty, type UseFormReturnType } from "@mantine/form";
import { modals } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Icons, IconSize } from "@/lib/icons";
import { useAppSearchParams } from "@/app/router";
import { EmptyState, ErrorState, LoadingState } from "@/components/AsyncState";
import { PageHeader } from "@/components/PageHeader";
import { useUpdateJobs, type UpdateJobSnapshot, type UpdateJobSummary } from "@/features/updates/updateJobs";
import { errorMessage, formatDate, formatNumber } from "@/lib/format";
import { ProviderMark } from "@/lib/providers";
import { deleteUpdateTarget, isTauriRuntime, listUpdateTargets, setUpdateTargetEnabled, upsertUpdateTarget } from "@/services/dbApi";
import type { UpdateTarget } from "@/types/library";

const demoSnapshot: UpdateJobSnapshot = {
  jobId: "preview-20260802", status: "completed", scope: "all", mode: "check_only", totals: 43, processed: 43, candidateCount: 3, savedCount: 0, errorCount: 0, activeLabel: null,
  startedAt: new Date(Date.now() - 90_000).toISOString(), updatedAt: new Date().toISOString(), finishedAt: new Date().toISOString(),
  logs: [
    { id: 1, logType: "info", message: "43件の更新対象を確認しました", createdAt: new Date(Date.now() - 80_000).toISOString() },
    { id: 2, logType: "success", message: "3件の新しい候補が見つかりました", createdAt: new Date().toISOString() },
  ],
  candidates: [
    { id: 1, key: "pixiv:12001", source: "pixiv", sourceId: "12001", title: "星を編む人 第十三話", subtitle: "32,800字", targetLabel: "星を編む人", targetType: "series", selected: true, status: "candidate" },
    { id: 2, key: "fanbox:10920", source: "fanbox", sourceId: "10920", title: "制作ノート #25", subtitle: "画像8点", targetLabel: "mizu atelier", targetType: "author", selected: true, status: "candidate" },
    { id: 3, key: "pixiv:12002", source: "pixiv", sourceId: "12002", title: "夏灯りの帰り道", subtitle: "短編", targetLabel: "青葉しおり", targetType: "author", selected: false, status: "candidate" },
  ],
};

export function isSavableCandidateStatus(status: string): boolean {
  return status === "candidate" || status === "failed";
}

export function parseWorkId(value: string | null): number | null {
  if (!value || !/^\d+$/.test(value)) return null;
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) && parsed > 0 ? parsed : null;
}

export function normalizeConcurrency(value: unknown, min: number, max: number): number {
  const parsed = typeof value === "number" ? value : Number(value);
  if (!Number.isFinite(parsed)) return min;
  return Math.max(min, Math.min(max, Math.trunc(parsed)));
}

function validateConcurrency(value: unknown, min: number, max: number): string | null {
  return typeof value === "number" && Number.isInteger(value) && value >= min && value <= max
    ? null
    : `${min}〜${max}の整数を入力してください`;
}

export default function UpdatesPage() {
  const runtime = isTauriRuntime();
  const [searchParams] = useAppSearchParams();
  const workId = parseWorkId(searchParams.get("work"));
  const queryClient = useQueryClient();
  const updateJobs = useUpdateJobs(undefined, runtime);
  const [previewSnapshot, setPreviewSnapshot] = useState<UpdateJobSnapshot>(demoSnapshot);
  const activeSnapshot = runtime ? updateJobs.activeSnapshot : previewSnapshot;
  const jobs = runtime ? updateJobs.jobs : [demoSnapshot as UpdateJobSummary];
  const [selectedCandidateIds, setSelectedCandidateIds] = useState<number[]>([]);
  const [tab, setTab] = useState<string | null>("candidates");
  const form = useForm({
    initialValues: { scope: workId ? "work" : "all", mode: "check_only", fetchConcurrency: 3, saveConcurrency: 2, collectionConcurrency: 1 },
    validate: {
      fetchConcurrency: (value) => validateConcurrency(value, 1, 8),
      saveConcurrency: (value) => validateConcurrency(value, 1, 4),
      collectionConcurrency: (value) => validateConcurrency(value, 1, 3),
    },
  });
  const targetForm = useForm({
    initialValues: { targetType: "author", source: "pixiv", sourceKey: "", displayName: "" },
    validate: { sourceKey: isNotEmpty("IDを入力してください"), displayName: isNotEmpty("表示名を入力してください") },
  });
  const targets = useQuery({ queryKey: ["update-targets"], queryFn: () => runtime ? listUpdateTargets<UpdateTarget>(null, false) : Promise.resolve<UpdateTarget[]>([
    { id: 1, targetType: "author", source: "pixiv", sourceKey: "8001234", displayName: "青葉しおり", enabled: true, lastCheckedAt: new Date().toISOString(), lastSeenSourceId: "12002", lastSeenSourceUpdatedAt: null, metadataJson: null, createdAt: new Date().toISOString(), updatedAt: new Date().toISOString() },
    { id: 2, targetType: "series", source: "pixiv", sourceKey: "778120", displayName: "星を編む人", enabled: true, lastCheckedAt: new Date().toISOString(), lastSeenSourceId: "12001", lastSeenSourceUpdatedAt: null, metadataJson: null, createdAt: new Date().toISOString(), updatedAt: new Date().toISOString() },
  ]) });
  // The snapshot is polled, so re-seeding the selection from it on every
  // arrival used to re-tick boxes the user had just cleared. Seed once per job,
  // then only add candidates that appear later as the job discovers them.
  const seenRef = useRef<{ jobId: string | null; ids: Set<number> }>({ jobId: null, ids: new Set() });
  useEffect(() => {
    if (!activeSnapshot) return;
    const isNewJob = seenRef.current.jobId !== activeSnapshot.jobId;
    if (isNewJob) seenRef.current = { jobId: activeSnapshot.jobId, ids: new Set() };
    const eligible = new Set(activeSnapshot.candidates.filter(isSavableCandidate).map((item) => item.id));
    const fresh = activeSnapshot.candidates.filter((item) => item.selected && eligible.has(item.id) && !seenRef.current.ids.has(item.id)).map((item) => item.id);
    activeSnapshot.candidates.forEach((item) => seenRef.current.ids.add(item.id));
    if (isNewJob) setSelectedCandidateIds(fresh);
    else setSelectedCandidateIds((current) => [...new Set([...current, ...fresh])].filter((id) => eligible.has(id)));
  }, [activeSnapshot]);

  const selectableCandidateIds = activeSnapshot?.candidates.filter(isSavableCandidate).map((item) => item.id) ?? [];
  const selectableCandidateIdSet = new Set(selectableCandidateIds);
  const selectedSavableIds = selectedCandidateIds.filter((id) => selectableCandidateIdSet.has(id));
  const selectedSavableIdSet = new Set(selectedSavableIds);

  const startMutation = useMutation({
    mutationFn: async (values: typeof form.values) => {
      if (!runtime) { await new Promise((resolve) => setTimeout(resolve, 400)); setPreviewSnapshot({ ...demoSnapshot, jobId: `preview-${Date.now()}`, startedAt: new Date().toISOString(), updatedAt: new Date().toISOString() }); return; }
      await updateJobs.start({ scope: values.scope as "all" | "work" | "author" | "series", mode: values.mode as "check_only" | "auto_save", workIds: workId ? [workId] : null, concurrency: { fetch: normalizeConcurrency(values.fetchConcurrency, 1, 8), save: normalizeConcurrency(values.saveConcurrency, 1, 4), collection: normalizeConcurrency(values.collectionConcurrency, 1, 3) } });
      await updateJobs.loadJobs();
    },
    onError: (error) => notifications.show({ color: "red", title: "更新確認を開始できません", message: errorMessage(error) }),
  });
  const saveCandidatesMutation = useMutation({
    mutationFn: async () => { if (runtime && activeSnapshot) await updateJobs.saveCandidates(activeSnapshot.jobId, selectedSavableIds); },
    onSuccess: () => notifications.show({ color: "green", title: "保存を開始しました", message: `${selectedSavableIds.length}件を処理します` }),
    onError: (error) => notifications.show({ color: "red", title: "候補を保存できません", message: errorMessage(error) }),
  });
  const targetMutation = useMutation({
    mutationFn: async (input: { action: "add" | "toggle" | "delete"; target?: UpdateTarget; enabled?: boolean; values?: typeof targetForm.values }) => {
      if (!runtime) return;
      if (input.action === "add" && input.values) await upsertUpdateTarget({ ...input.values, enabled: true, metadataJson: null });
      if (input.action === "toggle" && input.target) await setUpdateTargetEnabled(input.target.targetType, input.target.source, input.target.sourceKey, Boolean(input.enabled));
      if (input.action === "delete" && input.target) await deleteUpdateTarget(input.target.targetType, input.target.source, input.target.sourceKey);
    },
    onSuccess: () => { targetForm.reset(); queryClient.invalidateQueries({ queryKey: ["update-targets"] }); },
    onError: (error) => notifications.show({ color: "red", title: "更新対象を変更できません", message: errorMessage(error) }),
  });
  const status = activeSnapshot?.status;
  const running = status === "queued" || status === "running" || status === "canceling";
  const progressValue = activeSnapshot?.totals ? activeSnapshot.processed / activeSnapshot.totals * 100 : 0;

  return (
    <div className="page page--contained updates-page">
      <PageHeader title="更新センター" description="作品の変更、作者の新作、シリーズの続編を一つのジョブとして安全に確認・保存します。" actions={<Button leftSection={<Icons.updates size={IconSize.action} />} loading={startMutation.isPending} disabled={running} onClick={() => form.onSubmit((values) => startMutation.mutate(values))()}>更新を確認</Button>} />
      <Grid gap="lg" align="flex-start">
        <Grid.Col span={{ base: 12, lg: 4, xl: 3 }}>
          <Stack gap="lg">
            <Card p="lg">
              <Stack gap="md"><Title order={3}>新しい更新ジョブ</Title>{workId && <Alert color="blue">作品 ID {workId} だけを確認します。</Alert>}<SegmentedControl fullWidth aria-label="更新時の処理方法" data={[{ value: "check_only", label: "確認のみ" }, { value: "auto_save", label: "自動保存" }]} {...form.getInputProps("mode")} /><Select label="対象" data={[{ value: "all", label: "すべての監視対象" }, { value: "work", label: "作品" }, { value: "author", label: "作者・クリエイター" }, { value: "series", label: "シリーズ" }]} disabled={Boolean(workId)} {...form.getInputProps("scope")} /><Divider /><Text size="sm" fw={700}>同時実行数</Text><Group grow><NumberInput label="取得" required hideControls allowDecimal={false} allowNegative={false} clampBehavior="strict" min={1} max={8} {...form.getInputProps("fetchConcurrency")} /><NumberInput label="保存" required hideControls allowDecimal={false} allowNegative={false} clampBehavior="strict" min={1} max={4} {...form.getInputProps("saveConcurrency")} /></Group><NumberInput label="一覧取得" required hideControls allowDecimal={false} allowNegative={false} clampBehavior="strict" min={1} max={3} {...form.getInputProps("collectionConcurrency")} /><Button variant="light" leftSection={<Icons.updates size={IconSize.menu} />} disabled={running} loading={startMutation.isPending} onClick={() => form.onSubmit((values) => startMutation.mutate(values))()}>確認を開始</Button></Stack>
            </Card>
            <Card p="lg"><Group justify="space-between" mb="sm"><Text fw={700}>履歴</Text><Badge variant="light" color="gray">{jobs.length}</Badge></Group><Stack gap={4}>{jobs.slice(0, 8).map((job) => <Button key={job.jobId} variant={activeSnapshot?.jobId === job.jobId ? "light" : "subtle"} color="gray" justify="space-between" size="compact-sm" onClick={() => runtime && updateJobs.selectJob(job.jobId)}><Text size="xs">{formatDate(job.startedAt, true)}</Text><StatusBadge status={job.status} /></Button>)}</Stack></Card>
          </Stack>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 8, xl: 9 }}>
          {!activeSnapshot ? <EmptyState icon={Icons.versionHistory} title="更新ジョブはまだありません" description="左側で対象と方法を選び、更新確認を開始してください。" /> : (
            <Stack gap="lg">
              <Card p="lg" className="update-progress-card">
                <Group justify="space-between" align="flex-start"><Box><Group gap="xs"><StatusBadge status={activeSnapshot.status} /><Text size="xs" c="dimmed">{activeSnapshot.jobId}</Text></Group><Title order={2} mt="sm">{activeSnapshot.activeLabel || statusTitle(activeSnapshot.status)}</Title><Text size="sm" c="dimmed" mt={5}>{formatNumber(activeSnapshot.processed)} / {formatNumber(activeSnapshot.totals)}件を処理 · 候補 {formatNumber(activeSnapshot.candidateCount)} · エラー {formatNumber(activeSnapshot.errorCount)}</Text></Box><Group gap="xs">{running && <Button variant="default" leftSection={<Icons.pause size={IconSize.menu} />} onClick={() => runtime && updateJobs.pause(activeSnapshot.jobId)}>一時停止</Button>}{activeSnapshot.status === "paused" && <Button leftSection={<Icons.resume size={IconSize.menu} />} onClick={() => runtime && updateJobs.resume(activeSnapshot.jobId)}>再開</Button>}{running && <Button variant="subtle" color="red" leftSection={<Icons.cancel size={IconSize.menu} />} onClick={() => runtime && updateJobs.cancel(activeSnapshot.jobId)}>中止</Button>}{(activeSnapshot.status === "failed" || activeSnapshot.status === "canceled") && <Button leftSection={<Icons.undo size={IconSize.menu} />} onClick={() => runtime && updateJobs.resume(activeSnapshot.jobId, true)}>失敗分を再試行</Button>}</Group></Group>
                <Progress value={progressValue} animated={running} mt="lg" size="lg" aria-label={`更新進捗 ${Math.round(progressValue)}%`} />
              </Card>

              <Tabs value={tab} onChange={setTab}>
                <Tabs.List><Tabs.Tab value="candidates" leftSection={<Icons.select size={IconSize.menu} />}>候補 <Badge size="xs" variant="light" ml={4}>{activeSnapshot.candidates.length}</Badge></Tabs.Tab><Tabs.Tab value="logs" leftSection={<Icons.pending size={IconSize.menu} />}>ログ</Tabs.Tab><Tabs.Tab value="targets" leftSection={<Icons.updates size={IconSize.menu} />}>監視対象</Tabs.Tab></Tabs.List>
                <Tabs.Panel value="candidates" pt="lg">
                  {!activeSnapshot.candidates.length ? <EmptyState icon={Icons.confirm} title="新しい候補はありません" description="監視対象はすべて最新です。" /> : <Card p={0}><Group justify="space-between" p="md"><Box><Text fw={700}>保存する候補</Text><Text size="xs" c="dimmed">{selectedSavableIds.length}件を選択中</Text></Box><Group gap="xs"><Button size="xs" variant="subtle" disabled={!selectableCandidateIds.length} onClick={() => setSelectedCandidateIds(selectableCandidateIds)}>すべて選択</Button><Button size="xs" variant="subtle" color="gray" disabled={!selectedSavableIds.length} onClick={() => setSelectedCandidateIds([])}>解除</Button><Button size="sm" leftSection={<Icons.export size={IconSize.menu} />} disabled={!selectedSavableIds.length || running} loading={saveCandidatesMutation.isPending} onClick={() => saveCandidatesMutation.mutate()}>選択候補を保存</Button></Group></Group><Divider /><Stack gap={0}>{activeSnapshot.candidates.map((candidate) => <Paper key={candidate.id} p="md" radius={0} className="update-candidate"><Group wrap="nowrap"><Checkbox checked={selectedSavableIdSet.has(candidate.id)} onChange={(event) => setSelectedCandidateIds((current) => event.currentTarget.checked ? [...new Set([...current, candidate.id])] : current.filter((id) => id !== candidate.id))} aria-label={`${candidate.title}を選択`} disabled={!isSavableCandidate(candidate)} /><ProviderMark provider={candidate.source} compact /><Stack gap={2} flex={1} miw={0}><Text size="sm" fw={650} className="line-clamp-1">{candidate.title}</Text><Text size="xs" c="dimmed">{candidate.targetLabel} · {candidate.subtitle}</Text></Stack><Badge color={candidate.status === "failed" ? "red" : candidate.status === "saved" ? "green" : "gray"} variant="light">{candidateStatusLabel(candidate.status)}</Badge></Group></Paper>)}</Stack></Card>}
                </Tabs.Panel>
                <Tabs.Panel value="logs" pt="lg"><Card p="lg"><Timeline active={activeSnapshot.logs.length}>{activeSnapshot.logs.map((log) => <Timeline.Item key={log.id} color={log.logType === "error" ? "red" : log.logType === "success" ? "green" : log.logType === "warn" ? "yellow" : "blue"} title={log.message}><Text size="xs" c="dimmed">{formatDate(log.createdAt, true)}</Text></Timeline.Item>)}</Timeline></Card></Tabs.Panel>
                <Tabs.Panel value="targets" pt="lg"><TargetsPanel targets={targets.data ?? []} loading={targets.isLoading} error={targets.error} retry={() => targets.refetch()} form={targetForm} mutation={targetMutation} /></Tabs.Panel>
              </Tabs>
            </Stack>
          )}
        </Grid.Col>
      </Grid>
    </div>
  );
}

type TargetValues = { targetType: string; source: string; sourceKey: string; displayName: string };
type TargetAction = { action: "add" | "toggle" | "delete"; target?: UpdateTarget; enabled?: boolean; values?: TargetValues };

function TargetsPanel({ targets, loading, error, retry, form, mutation }: { targets: UpdateTarget[]; loading: boolean; error: unknown; retry: () => void; form: UseFormReturnType<TargetValues, TargetValues, any>; mutation: { mutate: (input: TargetAction) => void; isPending: boolean } }) {
  if (loading) return <LoadingState />;
  if (error) return <ErrorState error={error} retry={retry} />;
  const confirmDelete = (target: UpdateTarget) => modals.openConfirmModal({
    title: "監視対象を削除しますか？",
    children: <Text size="sm">「{target.displayName}」を更新監視から削除します。保存済みの作品は削除されません。</Text>,
    labels: { confirm: "削除する", cancel: "キャンセル" },
    confirmProps: { color: "red" },
    onConfirm: () => mutation.mutate({ action: "delete", target }),
  });
  return <Stack gap="lg"><Card p="lg"><form onSubmit={form.onSubmit((values) => mutation.mutate({ action: "add", values }))}><Stack><Title order={3}>監視対象を追加</Title><Grid><Grid.Col span={{ base: 12, sm: 3 }}><Select label="種類" data={[{ value: "author", label: "作者" }, { value: "series", label: "シリーズ" }]} {...form.getInputProps("targetType")} /></Grid.Col><Grid.Col span={{ base: 12, sm: 3 }}><Select label="ソース" data={[{ value: "pixiv", label: "pixiv" }, { value: "fanbox", label: "FANBOX" }]} {...form.getInputProps("source")} /></Grid.Col><Grid.Col span={{ base: 12, sm: 3 }}><TextInput label="ID" {...form.getInputProps("sourceKey")} /></Grid.Col><Grid.Col span={{ base: 12, sm: 3 }}><TextInput label="表示名" {...form.getInputProps("displayName")} /></Grid.Col></Grid><Button type="submit" w="fit-content" leftSection={<Icons.add size={IconSize.menu} />} loading={mutation.isPending}>追加</Button></Stack></form></Card><Card p={0}>{targets.length ? targets.map((target, index) => <Box key={target.id}>{index > 0 && <Divider />}<Group p="md" wrap="nowrap"><ProviderMark provider={target.source} compact /><ThemeIcon variant="light" color="gray" aria-hidden>{target.targetType === "series" ? "S" : "A"}</ThemeIcon><Stack gap={2} flex={1} miw={0}><Text size="sm" fw={650} className="line-clamp-1">{target.displayName}</Text><Text size="xs" c="dimmed" className="line-clamp-1">{target.targetType === "series" ? "シリーズ" : "作者"} · {target.sourceKey} · 最終確認 {formatDate(target.lastCheckedAt, true)}</Text></Stack><Switch checked={target.enabled} disabled={mutation.isPending} onChange={(event) => mutation.mutate({ action: "toggle", target, enabled: event.currentTarget.checked })} aria-label={`${target.displayName}の監視`} /><Tooltip label="削除"><ActionIcon variant="subtle" color="red" disabled={mutation.isPending} aria-label={`${target.displayName}を削除`} onClick={() => confirmDelete(target)}><Icons.delete size={IconSize.menu} /></ActionIcon></Tooltip></Group></Box>) : <Text p="lg" c="dimmed" size="sm">監視対象はありません。</Text>}</Card></Stack>;
}

function isSavableCandidate(candidate: UpdateJobSnapshot["candidates"][number]) {
  return isSavableCandidateStatus(candidate.status);
}

function candidateStatusLabel(status: UpdateJobSnapshot["candidates"][number]["status"]): string {
  return ({ candidate: "候補", queued: "待機中", running: "保存中", saved: "保存済み", failed: "失敗", skipped: "スキップ", done: "処理済み" } as Record<string, string>)[status] ?? status;
}

function StatusBadge({ status }: { status: string }) {
  const config: Record<string, { color: string; label: string }> = { queued: { color: "gray", label: "待機中" }, running: { color: "blue", label: "実行中" }, paused: { color: "yellow", label: "一時停止" }, auth_required: { color: "orange", label: "再接続が必要" }, canceling: { color: "yellow", label: "停止中" }, canceled: { color: "gray", label: "中止" }, completed: { color: "green", label: "完了" }, failed: { color: "red", label: "失敗" } };
  const item = config[status] ?? { color: "gray", label: status };
  return <Badge color={item.color} variant="light" leftSection={(status === "running" || status === "queued") ? <span className="status-dot" /> : undefined}>{item.label}</Badge>;
}

function statusTitle(status: string): string { return ({ queued: "開始を待っています", running: "更新を確認しています", paused: "一時停止中", auth_required: "再接続が必要です", canceling: "停止しています", canceled: "更新確認を中止しました", completed: "更新確認が完了しました", failed: "更新確認で問題が発生しました" } as Record<string, string>)[status] ?? status; }
