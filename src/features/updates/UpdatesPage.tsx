import { useEffect, useMemo, useRef, useState } from "react";
import {
  ActionIcon,
  Badge,
  Box,
  Button,
  Card,
  Checkbox,
  Divider,
  Grid,
  Group,
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
  UnstyledButton,
} from "@mantine/core";
import { useForm, isNotEmpty, type UseFormReturnType } from "@mantine/form";
import { modals } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Icons, IconSize } from "@/lib/icons";
import { useAppSearchParams } from "@/app/router";
import { EmptyState, ErrorState, LoadingState } from "@/components/AsyncState";
import { PageHeader } from "@/components/PageHeader";
import { UPDATE_JOB_STATUS_META, invalidateAfterUpdateJob, isUpdateJobTerminal, useUpdateJobs, type UpdateJobSnapshot, type UpdateJobSummary } from "@/features/updates/updateJobs";
import { errorMessage, formatDate, formatNumber } from "@/lib/format";
import { ProviderMark } from "@/lib/providers";
import { deleteUpdateTarget, isTauriRuntime, listUpdateTargets, searchDownloadsV2, setUpdateTargetEnabled, upsertUpdateTarget } from "@/services/dbApi";
import { readingWorkIds } from "@/features/library/readingShelf";
import { clearFinishedUpdateJobsCommand, countDismissedUpdateCandidatesCommand, dismissUpdateCandidateCommand, restoreDismissedUpdateCandidatesCommand } from "@/services/updateJobApi";
import { loadSchedule, type UpdateScheduleSettings } from "@/features/updates/updateSchedule";
import { UpdateScheduleCard } from "@/features/updates/UpdateScheduleCard";
import { useUpdateJobNotifications } from "@/features/updates/useUpdateScheduler";
import type { UpdateTarget } from "@/types/library";

const demoSnapshot: UpdateJobSnapshot = {
  jobId: "preview-20260802", status: "completed", scope: "all", mode: "check_only", totals: 43, processed: 43, candidateCount: 3, savedCount: 0, errorCount: 0, activeLabel: null,
  startedAt: new Date(Date.now() - 90_000).toISOString(), updatedAt: new Date().toISOString(), finishedAt: new Date().toISOString(),
  logs: [
    { id: 1, logType: "info", message: "43件の更新対象を確認しました", createdAt: new Date(Date.now() - 80_000).toISOString() },
    { id: 2, logType: "success", message: "3件の新しい候補が見つかりました", createdAt: new Date().toISOString() },
  ],
  candidates: [
    { id: 1, key: "pixiv:12001", source: "pixiv", sourceId: "12001", title: "星を編む人 第十三話", subtitle: "青葉しおり ・ 2026-08-14", targetLabel: "星を編む人", targetType: "series", selected: true, status: "candidate", kind: "sequel" },
    { id: 2, key: "fanbox:10920", source: "fanbox", sourceId: "10920", title: "制作ノート #25", subtitle: "2026-08-12", targetLabel: "mizu atelier", targetType: "author", selected: true, status: "candidate", kind: "new" },
    { id: 3, key: "fanbox:10880", source: "fanbox", sourceId: "10880", title: "4月のまとめ（加筆）", subtitle: "手元は v1", targetLabel: "mizu atelier", targetType: "author", selected: true, status: "candidate", kind: "revision" },
  ],
  nextCandidateCursor: null,
  previousLogCursor: null,
};

export function isSavableCandidateStatus(status: string): boolean {
  return status === "candidate" || status === "failed";
}

/**
 * もう手を動かす余地のない候補か。
 *
 * 「保存する候補」は、これから決めるものの一覧である。済んだものを同じ顔で
 * 並べ続けると、44件保存し終えた次の日も44件が待っているように見え、どれが
 * 残りなのか読めなくなる。消しはしない - 数えて畳み、開けば出てくる。
 */
export function isSettledCandidateStatus(status: string): boolean {
  return status === "saved" || status === "skipped" || status === "done";
}

/** How many works one shelf check may cover before it stops being sensible. */
const SHELF_SCOPE_LIMIT = 500;

/**
 * The works behind a shelf scope, or null when the scope is not a shelf.
 *
 * Shelves reuse the plain work check: the ids are collected here and sent as
 * `workIds`, so the job needs no new scope of its own.
 */
export async function shelfWorkIds(scope: string): Promise<number[] | null> {
  if (scope !== "favorite" && scope !== "reading") return null;
  const params = scope === "favorite"
    ? { favorite: true, limit: SHELF_SCOPE_LIMIT, projection: "bulk" as const }
    : { idsInclude: readingWorkIds().slice(0, SHELF_SCOPE_LIMIT), limit: SHELF_SCOPE_LIMIT, projection: "bulk" as const };
  if (scope === "reading" && !params.idsInclude?.length) return [];
  const result = await searchDownloadsV2(params);
  return result.items.map((item) => item.id);
}

export function parseWorkId(value: string | null): number | null {
  if (!value || !/^\d+$/.test(value)) return null;
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) && parsed > 0 ? parsed : null;
}

export default function UpdatesPage() {
  const runtime = isTauriRuntime();
  const [searchParams] = useAppSearchParams();
  const workId = parseWorkId(searchParams.get("work"));
  const queryClient = useQueryClient();
  const updateJobs = useUpdateJobs(undefined, runtime);
  // 確認が終わったら OS の通知で知らせる。画面を見ていないときのための足。
  useUpdateJobNotifications(runtime ? updateJobs.activeSnapshot : null, runtime);
  const [previewSnapshot, setPreviewSnapshot] = useState<UpdateJobSnapshot>(demoSnapshot);
  const activeSnapshot = runtime ? updateJobs.activeSnapshot : previewSnapshot;
  const jobs = runtime ? updateJobs.jobs : [demoSnapshot as UpdateJobSummary];
  const [selectedCandidateIds, setSelectedCandidateIds] = useState<number[]>([]);
  // 非表示にした候補は、この画面ではすぐ消す（次のジョブでは元から出てこない）。
  const [dismissedIds, setDismissedIds] = useState<number[]>([]);
  const [tab, setTab] = useState<string | null>("candidates");
  const form = useForm({
    initialValues: { scope: workId ? "work" : "all", mode: "check_only" },
  });
  const targetForm = useForm({
    initialValues: { targetType: "author", source: "pixiv", sourceKey: "", displayName: "" },
    validate: { sourceKey: isNotEmpty("IDを入力してください"), displayName: isNotEmpty("表示名を入力してください") },
  });
  // 一覧に出すのは「自分で選んだ作者・シリーズ」だけ。作品の監視は
  // 作品カードのトグルが持っていて、ここに混ぜると名前が並んで分かりにくい。
  const targets = useQuery({ queryKey: ["update-targets"], queryFn: async () => runtime ? (await listUpdateTargets<UpdateTarget>(null, false)).filter((target) => target.targetType === "author" || target.targetType === "series") : ([
    { id: 1, targetType: "author", source: "pixiv", sourceKey: "8001234", displayName: "青葉しおり", enabled: true, lastCheckedAt: new Date().toISOString(), lastSeenSourceId: "12002", lastSeenSourceUpdatedAt: null, metadataJson: null, createdAt: new Date().toISOString(), updatedAt: new Date().toISOString() },
    { id: 2, targetType: "series", source: "pixiv", sourceKey: "778120", displayName: "星を編む人", enabled: true, lastCheckedAt: new Date().toISOString(), lastSeenSourceId: "12001", lastSeenSourceUpdatedAt: null, metadataJson: null, createdAt: new Date().toISOString(), updatedAt: new Date().toISOString() },
    { id: 3, targetType: "author", source: "fanbox", sourceKey: "mizu-atelier", displayName: "mizu atelier", enabled: false, lastCheckedAt: new Date(Date.now() - 86_400_000).toISOString(), lastSeenSourceId: null, lastSeenSourceUpdatedAt: null, metadataJson: null, createdAt: new Date().toISOString(), updatedAt: new Date().toISOString() },
  ] as UpdateTarget[]) });
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
      // 棚を選んだときは、その棚の作品IDを集めて「作品」の確認として投げる。
      // 監視のオン・オフに関わらず、いま棚にあるものを見に行ける。
      const shelfIds = await shelfWorkIds(values.scope);
      if (shelfIds && !shelfIds.length) throw new Error("この棚には作品がありません");
      const schedule = await loadSchedule();
      await updateJobs.start({
        scope: (shelfIds ? "work" : values.scope) as "all" | "work" | "author" | "series",
        mode: values.mode as "check_only" | "auto_save",
        workIds: shelfIds ?? (workId ? [workId] : null),
        watchSaved: schedule.watchSaved,
      });
      await updateJobs.loadJobs();
    },
    onError: (error) => notifications.show({ color: "red", title: "更新確認を開始できません", message: errorMessage(error) }),
  });
  // チェックを外すのは「今は選ばない」。二度と出したくない、は別の操作にする。
  const dismissMutation = useMutation({
    mutationFn: async (candidate: UpdateJobSnapshot["candidates"][number]) => {
      if (runtime) await dismissUpdateCandidateCommand(candidate.source, candidate.sourceId, true);
      return candidate;
    },
    onSuccess: (candidate) => {
      setDismissedIds((current) => [...new Set([...current, candidate.id])]);
      queryClient.invalidateQueries({ queryKey: ["dismissed-candidates"] });
      invalidateAfterUpdateJob(queryClient);
      notifications.show({
        color: "gray",
        title: "今後は表示しません",
        message: (
          <Group gap="sm" wrap="nowrap">
            <Text size="sm" className="line-clamp-1" style={{ flex: 1, minWidth: 0 }}>{candidate.title}</Text>
            {/* 取り消しは題より優先する。長い題に押されると「元に戻…」になってしまう。 */}
            <Button size="compact-xs" variant="default" style={{ flex: "none" }} onClick={() => undismissMutation.mutate(candidate)}>元に戻す</Button>
          </Group>
        ),
      });
    },
    onError: (error) => notifications.show({ color: "red", title: "候補を非表示にできません", message: errorMessage(error) }),
  });
  const undismissMutation = useMutation({
    mutationFn: async (candidate: UpdateJobSnapshot["candidates"][number]) => {
      if (runtime) await dismissUpdateCandidateCommand(candidate.source, candidate.sourceId, false);
      return candidate;
    },
    onSuccess: (candidate) => {
      setDismissedIds((current) => current.filter((id) => id !== candidate.id));
      queryClient.invalidateQueries({ queryKey: ["dismissed-candidates"] });
      invalidateAfterUpdateJob(queryClient);
    },
  });
  const dismissedCount = useQuery({
    queryKey: ["dismissed-candidates"],
    queryFn: () => (runtime ? countDismissedUpdateCandidatesCommand() : Promise.resolve(0)),
  });
  const restoreDismissedMutation = useMutation({
    mutationFn: () => (runtime ? restoreDismissedUpdateCandidatesCommand() : Promise.resolve(0)),
    onSuccess: (restored) => {
      setDismissedIds([]);
      queryClient.invalidateQueries({ queryKey: ["dismissed-candidates"] });
      invalidateAfterUpdateJob(queryClient);
      notifications.show({ color: "piep", title: "非表示を解除しました", message: `${restored}件が次の確認でまた候補に出ます` });
    },
  });

  /**
   * 履歴は残すためのものだが、消せないままでは溜まる一方になる。
   * 走っているジョブは消さない - 消してしまえば、進んでいるものの行き先が
   * 画面から無くなる。
   */
  const clearJobMutation = useMutation({
    mutationFn: async (jobId: string) => {
      if (!runtime) return;
      await updateJobs.clear(jobId);
      await updateJobs.loadJobs();
    },
    onError: (error) => notifications.show({ color: "red", title: "履歴を消せません", message: errorMessage(error) }),
  });
  const clearFinishedJobsMutation = useMutation({
    mutationFn: async () => {
      if (!runtime) return 0;
      const removed = await clearFinishedUpdateJobsCommand();
      await updateJobs.loadJobs();
      return removed;
    },
    onSuccess: (removed) => notifications.show({ color: "gray", title: "履歴を消しました", message: `${formatNumber(removed ?? 0)}件を消しました。走っているジョブはそのままです` }),
    onError: (error) => notifications.show({ color: "red", title: "履歴を消せません", message: errorMessage(error) }),
  });
  const confirmClearFinishedJobs = () => modals.openConfirmModal({
    title: "終わった履歴をまとめて消しますか？",
    children: <Text size="sm">終わったジョブの記録・ログ・候補の一覧を消します。保存した作品には触れません。走っているジョブは残ります。</Text>,
    labels: { confirm: "消す", cancel: "やめる" },
    confirmProps: { color: "red" },
    onConfirm: () => clearFinishedJobsMutation.mutate(),
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
  // 自動確認は「いまどうなっているか」だけを一行で名乗る。設定そのものは
  // タブの中にあり、変えたらこの一行も追いつく。
  const [scheduleRevision, setScheduleRevision] = useState(0);
  const schedule = useQuery({
    queryKey: ["update-schedule", scheduleRevision],
    queryFn: () => loadSchedule(),
    staleTime: 0,
  });
  const scheduleSummary = describeSchedule(schedule.data);
  /**
   * 取得制限に当たっているか。
   *
   * ジョブは制限を受けると「取得が制限されています。N秒あけて…」と記録する。
   * 平常時の注意書きを畳んだぶん、実際に効いた瞬間はここが名乗る。
   */
  const throttledMessage = useMemo(() => {
    const logs = activeSnapshot?.logs ?? [];
    for (let index = logs.length - 1; index >= 0; index -= 1) {
      const log = logs[index];
      if (log.logType !== "warn") continue;
      if (log.message.includes("取得が制限されています")) return log.message;
      break;
    }
    return null;
  }, [activeSnapshot?.logs]);
  // 残りの数。タブの脇に出るのは、これから決めるものの件数でなければならない。
  const openCandidateCount = (activeSnapshot?.candidates ?? [])
    .filter((candidate) => !dismissedIds.includes(candidate.id) && !isSettledCandidateStatus(candidate.status)).length;
  const finishedJobCount = jobs.filter((job) => isUpdateJobTerminal(job.status)).length;
  const status = activeSnapshot?.status;
  const running = status === "queued" || status === "running" || status === "canceling";
  // 一時停止と再接続待ちは、どちらも「止まっていて、押せば続きから動く」。
  // 再接続が要るほうにだけボタンが無いと、連携し直しても戻る道が無くなる。
  const stalled = status === "paused" || status === "auth_required";
  const progressValue = activeSnapshot?.totals ? activeSnapshot.processed / activeSnapshot.totals * 100 : 0;

  return (
    <div className="page page--contained updates-page">
      <PageHeader title="更新センター" description="作品の変更、作者の新作、シリーズの続編を一つのジョブとして安全に確認・保存します。" />

      {/* 毎回触るものだけを一列に置く。自動確認は数か月に一度しか触らない
          設定なので、いまどうなっているかだけ名乗って、変更は開いたときに。
          触る頻度が3桁違うものを同じ重さで並べていたころは、左の柱が画面
          より長くなっていた。 */}
      <Paper className="updates-toolbar" p="sm" withBorder mt="md">
        <Group gap="sm" wrap="wrap">
          <Select
            aria-label="確認する対象"
            leftSection={<Icons.select size={IconSize.menu} />}
            data={[{ value: "all", label: "すべての監視対象" }, { value: "work", label: "監視中の作品" }, { value: "author", label: "作者・クリエイター" }, { value: "series", label: "シリーズ" }, { value: "favorite", label: "お気に入りの作品" }, { value: "reading", label: "読みかけの作品" }]}
            disabled={Boolean(workId)}
            w={210}
            {...form.getInputProps("scope")}
          />
          <SegmentedControl aria-label="更新時の処理方法" data={[{ value: "check_only", label: "確認のみ" }, { value: "auto_save", label: "自動保存" }]} {...form.getInputProps("mode")} />
          <Button leftSection={<Icons.updates size={IconSize.menu} />} disabled={running} loading={startMutation.isPending} onClick={() => form.onSubmit((values) => startMutation.mutate(values))()}>確認を開始</Button>
          {/* 平常時に読ませたいのは一行だけ。詳しい話は、知りたくなった
              ときに開く。実際に効いた瞬間は下の進捗カードが名乗る。 */}
          <Group gap={4} wrap="nowrap">
            <Text size="xs" c="dimmed">1件ずつ間隔をあけて確認します</Text>
            <Tooltip
              multiline
              w={280}
              label="取得元に負担をかけないよう、1件ずつ間隔をあけて確認します。制限を受けたときは自動で間隔を広げ、通るようになったら戻します。"
            >
              <ActionIcon variant="subtle" color="gray" size="sm" aria-label="確認の進め方について">
                <Icons.help size={IconSize.menu} />
              </ActionIcon>
            </Tooltip>
          </Group>
          {workId && <Badge variant="light" color="gray">作品 ID {workId} のみ</Badge>}
          <Box style={{ flex: 1 }} />
          <Group gap={6} wrap="nowrap">
            <Text size="xs" c="dimmed">自動確認：{scheduleSummary}</Text>
            <Button size="compact-xs" variant="subtle" color="gray" onClick={() => setTab("schedule")}>変更</Button>
          </Group>
        </Group>
      </Paper>

      {activeSnapshot && (
        <Card p="lg" className="update-progress-card" mt="lg">
          <Group justify="space-between" align="flex-start" wrap="wrap">
            <Box miw={0}>
              <Group gap="xs"><StatusBadge status={activeSnapshot.status} /><Text size="xs" c="dimmed">{activeSnapshot.jobId}</Text></Group>
              <Title order={2} mt="sm">{activeSnapshot.activeLabel || statusTitle(activeSnapshot.status)}</Title>
              <Text size="sm" c="dimmed" mt={5}>{formatNumber(activeSnapshot.processed)} / {formatNumber(activeSnapshot.totals)}件を処理 · 候補 {formatNumber(activeSnapshot.candidateCount)} · エラー {formatNumber(activeSnapshot.errorCount)}</Text>
            </Box>
            <Group gap="xs">
              {running && <Button variant="default" leftSection={<Icons.pause size={IconSize.menu} />} onClick={() => runtime && updateJobs.pause(activeSnapshot.jobId)}>一時停止</Button>}
              {stalled && <Button leftSection={<Icons.resume size={IconSize.menu} />} onClick={() => runtime && updateJobs.resume(activeSnapshot.jobId)}>再開</Button>}
              {(running || stalled) && <Button variant="subtle" color="red" leftSection={<Icons.cancel size={IconSize.menu} />} onClick={() => runtime && updateJobs.cancel(activeSnapshot.jobId)}>中止</Button>}
              {(activeSnapshot.status === "failed" || activeSnapshot.status === "canceled") && <Button leftSection={<Icons.undo size={IconSize.menu} />} onClick={() => runtime && updateJobs.resume(activeSnapshot.jobId, true)}>失敗分を再試行</Button>}
            </Group>
          </Group>
          {/* 間隔の説明が本当に要る瞬間はここ。「遅い」と感じたときに、
              なぜ待っているのかが同じ場所に出る。 */}
          {running && throttledMessage && (
            <Group gap={6} mt="md" wrap="nowrap" role="status">
              <ThemeIcon size="sm" radius="xl" color="yellow" variant="light"><Icons.pending size={IconSize.inline} /></ThemeIcon>
              <Text size="xs" c="dimmed" className="line-clamp-1">{throttledMessage}</Text>
            </Group>
          )}
          <Progress value={progressValue} animated={running} mt="lg" size="lg" aria-label={`更新進捗 ${Math.round(progressValue)}%`} />
        </Card>
      )}

      <Tabs value={tab} onChange={setTab} mt="lg">
        {/* 左から右へ「いま何が待っているか → どう進んだか → 前はどうだったか」、
            区切りを挟んで「何を追いかける約束か → いつ走らせる約束か」。
            数の帯は、伝える中身があるときだけ出す - 0 は何も言っていない。 */}
        <Tabs.List>
          <Tabs.Tab value="candidates" leftSection={<Icons.select size={IconSize.menu} />}>候補 {openCandidateCount > 0 && <Badge size="xs" variant="light" ml={4}>{formatNumber(openCandidateCount)}</Badge>}</Tabs.Tab>
          <Tabs.Tab value="logs" leftSection={<Icons.pending size={IconSize.menu} />}>ログ</Tabs.Tab>
          <Tabs.Tab value="history" leftSection={<Icons.versionHistory size={IconSize.menu} />}>履歴 {jobs.length > 0 && <Badge size="xs" variant="light" color="gray" ml={4}>{formatNumber(jobs.length)}</Badge>}</Tabs.Tab>
          <span className="updates-tabs__divider" aria-hidden />
          <Tabs.Tab value="targets" leftSection={<Icons.updates size={IconSize.menu} />}>監視対象</Tabs.Tab>
          <Tabs.Tab value="schedule" leftSection={<Icons.publishedDate size={IconSize.menu} />}>自動確認</Tabs.Tab>
        </Tabs.List>

        <Tabs.Panel value="candidates" pt="lg">
          {activeSnapshot ? (
            <CandidatesPanel
              candidates={activeSnapshot.candidates.filter((candidate) => !dismissedIds.includes(candidate.id))}
              selectedIds={selectedSavableIdSet}
              selectableIds={selectableCandidateIds}
              running={running}
              saving={saveCandidatesMutation.isPending}
              hasMore={Boolean(activeSnapshot.nextCandidateCursor)}
              onToggle={(id, checked) => setSelectedCandidateIds((current) => checked ? [...new Set([...current, id])] : current.filter((item) => item !== id))}
              onSelectMany={(ids) => setSelectedCandidateIds(ids)}
              onSave={() => saveCandidatesMutation.mutate()}
              onLoadMore={() => runtime && updateJobs.loadMoreCandidates()}
              onDismiss={(candidate) => dismissMutation.mutate(candidate)}
            />
          ) : (
            <EmptyState icon={Icons.versionHistory} title="更新ジョブはまだありません" description="上の対象と方法を選び、「確認を開始」を押してください。" />
          )}
        </Tabs.Panel>

        <Tabs.Panel value="logs" pt="lg">
          {activeSnapshot ? (
            <Card p="lg">
              {activeSnapshot.previousLogCursor && <Button mb="md" variant="default" onClick={() => runtime && updateJobs.loadOlderLogs()}>以前のログを読み込む</Button>}
              <Timeline active={activeSnapshot.logs.length}>{activeSnapshot.logs.map((log) => <Timeline.Item key={log.id} color={log.logType === "error" ? "red" : log.logType === "success" ? "green" : log.logType === "warn" ? "yellow" : "piep"} title={log.logType === "error" ? errorMessage(log.message) : log.message}><Text size="xs" c="dimmed">{formatDate(log.createdAt, true)}</Text></Timeline.Item>)}</Timeline>
            </Card>
          ) : (
            <EmptyState icon={Icons.pending} title="ログはまだありません" description="確認を開始すると、ここに1件ずつの結果が並びます。" />
          )}
        </Tabs.Panel>

        <Tabs.Panel value="targets" pt="lg">
          {(dismissedCount.data ?? 0) > 0 && (
            <Card p="md" mb="lg">
              <Group justify="space-between" wrap="nowrap">
                <Box>
                  <Text size="sm" fw={650}>非表示にした作品が{dismissedCount.data}件あります</Text>
                  <Text size="xs" c="dimmed">解除すると、次の確認でまた候補に並びます。</Text>
                </Box>
                <Button size="xs" variant="default" loading={restoreDismissedMutation.isPending} onClick={() => restoreDismissedMutation.mutate()}>すべて解除</Button>
              </Group>
            </Card>
          )}
          <TargetsPanel targets={targets.data ?? []} loading={targets.isLoading} error={targets.error} retry={() => targets.refetch()} form={targetForm} mutation={targetMutation} />
        </Tabs.Panel>

        <Tabs.Panel value="history" pt="lg">
          <JobHistoryPanel
            jobs={jobs}
            activeJobId={activeSnapshot?.jobId ?? null}
            finishedCount={finishedJobCount}
            clearingAll={clearFinishedJobsMutation.isPending}
            clearingJobId={clearJobMutation.isPending ? (clearJobMutation.variables ?? null) : null}
            onSelect={(jobId) => runtime && updateJobs.selectJob(jobId)}
            onClear={(jobId) => clearJobMutation.mutate(jobId)}
            onClearFinished={confirmClearFinishedJobs}
          />
        </Tabs.Panel>

        <Tabs.Panel value="schedule" pt="lg">
          <UpdateScheduleCard onChanged={() => setScheduleRevision((value) => value + 1)} />
        </Tabs.Panel>
      </Tabs>
    </div>
  );
}

/**
 * 走らせた記録。
 *
 * 残すためのものだが、消せないままでは溜まる一方になる。走っているものには
 * 消す口を出さない - 消せば、進んでいるものの行き先が画面から無くなる。
 */
function JobHistoryPanel({ jobs, activeJobId, finishedCount, clearingAll, clearingJobId, onSelect, onClear, onClearFinished }: {
  jobs: UpdateJobSummary[];
  activeJobId: string | null;
  finishedCount: number;
  clearingAll: boolean;
  clearingJobId: string | null;
  onSelect: (jobId: string) => void;
  onClear: (jobId: string) => void;
  onClearFinished: () => void;
}) {
  if (!jobs.length) {
    return <EmptyState icon={Icons.versionHistory} title="履歴はまだありません" description="確認を開始すると、走らせた記録がここに残ります。" />;
  }
  return (
    <Card p={0}>
      <Group justify="space-between" p="md" wrap="nowrap">
        <Box>
          <Text fw={700}>走らせた記録</Text>
          <Text size="xs" c="dimmed">選ぶと、その回の候補とログを開きます</Text>
        </Box>
        {finishedCount > 0 && (
          <Button size="xs" variant="default" color="gray" leftSection={<Icons.delete size={IconSize.menu} />} loading={clearingAll} onClick={onClearFinished}>終わった{formatNumber(finishedCount)}件を消す</Button>
        )}
      </Group>
      <Divider />
      <Stack gap={0}>
        {jobs.map((job) => (
          <Group key={job.jobId} gap="sm" wrap="nowrap" px="md" py="sm" className="update-history__row" data-active={job.jobId === activeJobId || undefined}>
            <UnstyledButton style={{ flex: 1, minWidth: 0 }} onClick={() => onSelect(job.jobId)}>
              <Group gap="sm" wrap="nowrap">
                <StatusBadge status={job.status} />
                <Text size="sm" fw={job.jobId === activeJobId ? 700 : 500}>{formatDate(job.startedAt, true)}</Text>
                {/* どの回が実りある回だったかは、開かなくても分かるほうがいい。 */}
                <Text size="xs" c="dimmed" className="line-clamp-1">
                  {formatNumber(job.processed)}/{formatNumber(job.totals)}件 · 候補 {formatNumber(job.candidateCount)} · 保存 {formatNumber(job.savedCount)}{job.errorCount > 0 ? ` · エラー ${formatNumber(job.errorCount)}` : ""}
                </Text>
              </Group>
            </UnstyledButton>
            {isUpdateJobTerminal(job.status) && (
              <Tooltip label="この履歴を消す">
                <ActionIcon className="update-history__delete" variant="subtle" color="gray" size="sm" aria-label={`${formatDate(job.startedAt, true)}の履歴を消す`} loading={clearingJobId === job.jobId} onClick={() => onClear(job.jobId)}>
                  <Icons.cancel size={IconSize.menu} />
                </ActionIcon>
              </Tooltip>
            )}
          </Group>
        ))}
      </Stack>
    </Card>
  );
}

type TargetValues = { targetType: string; source: string; sourceKey: string; displayName: string };
type TargetAction = { action: "add" | "toggle" | "delete"; target?: UpdateTarget; enabled?: boolean; values?: TargetValues };

function TargetsPanel({ targets, loading, error, retry, form, mutation }: { targets: UpdateTarget[]; loading: boolean; error: unknown; retry: () => void; form: UseFormReturnType<TargetValues, TargetValues, any>; mutation: { mutate: (input: TargetAction) => void; isPending: boolean } }) {
  const [showPaused, setShowPaused] = useState(false);
  if (loading) return <LoadingState />;
  if (error) return <ErrorState error={error} retry={retry} />;
  const confirmDelete = (target: UpdateTarget) => modals.openConfirmModal({
    title: "監視対象を削除しますか？",
    children: <Text size="sm">「{target.displayName}」を更新監視から削除します。保存済みの作品は削除されません。</Text>,
    labels: { confirm: "削除する", cancel: "キャンセル" },
    confirmProps: { color: "red" },
    onConfirm: () => mutation.mutate({ action: "delete", target }),
  });
  const active = targets.filter((target) => target.enabled);
  const paused = targets.filter((target) => !target.enabled);

  const row = (target: UpdateTarget) => (
    <Group p="md" wrap="nowrap" key={target.id}>
      <ProviderMark provider={target.source} compact />
      <ThemeIcon variant="light" color="gray" aria-hidden>{target.targetType === "series" ? "S" : "A"}</ThemeIcon>
      <Stack gap={3} flex={1} miw={0}>
        <Group gap={6} wrap="nowrap">
          <Text size="sm" fw={650} className="line-clamp-1">{target.displayName}</Text>
          <TargetHealthBadge target={target} />
        </Group>
        <Text size="xs" c="dimmed" className="line-clamp-1">
          {target.targetType === "series" ? "シリーズ" : "作者"} · {target.sourceKey} · 最終確認 {formatDate(target.lastCheckedAt, true)} · 最後のヒット {target.lastHitAt ? formatDate(target.lastHitAt, true) : "なし"}
        </Text>
      </Stack>
      <Tooltip label={target.enabled ? "確認を止める（一覧では下に畳まれます）" : "確認を再開する"}>
        <Switch
          checked={target.enabled}
          disabled={mutation.isPending}
          onChange={(event) => mutation.mutate({ action: "toggle", target, enabled: event.currentTarget.checked })}
          aria-label={`${target.displayName}の監視`}
        />
      </Tooltip>
      <Tooltip label="削除">
        <ActionIcon variant="subtle" color="red" disabled={mutation.isPending} aria-label={`${target.displayName}を削除`} onClick={() => confirmDelete(target)}>
          <Icons.delete size={IconSize.menu} />
        </ActionIcon>
      </Tooltip>
    </Group>
  );

  return (
    <Stack gap="lg">
      <Card p="lg">
        <form onSubmit={form.onSubmit((values) => mutation.mutate({ action: "add", values }))}>
          <Stack>
            <Title order={3}>監視対象を追加</Title>
            <Text size="sm" c="dimmed" mt={-8}>ここに入るのは、自分で追加した作者とシリーズだけです。保存や更新確認で勝手に増えることはありません。</Text>
            <Grid>
              <Grid.Col span={{ base: 12, sm: 3 }}><Select label="種類" data={[{ value: "author", label: "作者" }, { value: "series", label: "シリーズ" }]} {...form.getInputProps("targetType")} /></Grid.Col>
              <Grid.Col span={{ base: 12, sm: 3 }}><Select label="ソース" data={[{ value: "pixiv", label: "pixiv" }, { value: "fanbox", label: "FANBOX" }]} {...form.getInputProps("source")} /></Grid.Col>
              <Grid.Col span={{ base: 12, sm: 3 }}><TextInput label="ID" {...form.getInputProps("sourceKey")} /></Grid.Col>
              <Grid.Col span={{ base: 12, sm: 3 }}><TextInput label="表示名" {...form.getInputProps("displayName")} /></Grid.Col>
            </Grid>
            <Button type="submit" w="fit-content" leftSection={<Icons.add size={IconSize.menu} />} loading={mutation.isPending}>追加</Button>
          </Stack>
        </form>
      </Card>

      <Card p={0}>
        {active.length
          ? active.map((target, index) => <Box key={target.id}>{index > 0 && <Divider />}{row(target)}</Box>)
          : <Text p="lg" c="dimmed" size="sm">確認中の対象はありません。</Text>}
      </Card>

      {/* 止めた対象はここへ畳む。トグルひとつで一覧から下がり、消したいときだけ
          削除すればいい。切り替えた瞬間に消えるより戻しやすい。 */}
      {paused.length > 0 && (
        <Card p={0}>
          <UnstyledButton className="update-paused__head" onClick={() => setShowPaused((open) => !open)} aria-expanded={showPaused}>
            <Group p="md" justify="space-between" wrap="nowrap">
              <Group gap="xs" wrap="nowrap">
                <Icons.next size={IconSize.menu} style={{ transform: showPaused ? "rotate(90deg)" : undefined }} aria-hidden />
                <Text size="sm" fw={650}>停止中</Text>
                <Badge size="sm" variant="light" color="gray">{paused.length}</Badge>
              </Group>
              <Text size="xs" c="dimmed">確認しません。再開も削除もここから</Text>
            </Group>
          </UnstyledButton>
          {showPaused && paused.map((target) => <Box key={target.id}><Divider />{row(target)}</Box>)}
        </Card>
      )}
    </Stack>
  );
}

/** 候補の種類ごとの見た目。色は意味に対応させ、数は増やさない。 */
const CANDIDATE_KINDS = [
  { value: "all", label: "すべて" },
  { value: "new", label: "新作" },
  { value: "sequel", label: "続編" },
  { value: "revision", label: "改稿" },
] as const;

const KIND_COLOR: Record<string, string> = { new: "piep", sequel: "leaf", revision: "orange" };

function kindLabel(kind: string): string {
  return CANDIDATE_KINDS.find((item) => item.value === kind)?.label ?? "候補";
}

/**
 * 見つかったものを選り分ける画面。
 *
 * 「新作」「続編」「改稿」は判断の重さが違う — 改稿は手元の版を置き換えるので、
 * 新作をまとめて取り込むついでに混ぜたくない。だから既定は分けて見せる。
 */
function CandidatesPanel({ candidates, selectedIds, selectableIds, running, saving, hasMore, onToggle, onSelectMany, onSave, onLoadMore, onDismiss }: {
  candidates: UpdateJobSnapshot["candidates"];
  selectedIds: Set<number>;
  selectableIds: number[];
  running: boolean;
  saving: boolean;
  hasMore: boolean;
  onToggle: (id: number, checked: boolean) => void;
  onSelectMany: (ids: number[]) => void;
  onSave: () => void;
  onLoadMore: () => void;
  onDismiss: (candidate: UpdateJobSnapshot["candidates"][number]) => void;
}) {
  const [kind, setKind] = useState<string>("all");
  // 済んだものは畳む。数は残す - 「無かったこと」にはしない。
  const [showSettled, setShowSettled] = useState(false);
  if (!candidates.length) {
    return <EmptyState icon={Icons.confirm} title="新しい候補はありません" description="監視対象はすべて最新です。" />;
  }
  const settled = candidates.filter((candidate) => isSettledCandidateStatus(candidate.status));
  const open = candidates.filter((candidate) => !isSettledCandidateStatus(candidate.status));
  if (!open.length && !showSettled) {
    return (
      <EmptyState
        icon={Icons.confirm}
        title="この確認の候補は、ぜんぶ片付きました"
        description={`見つかった${settled.length}件は保存済みです。次の確認では、ここに新しいぶんだけ並びます。`}
        action={<Button variant="default" onClick={() => setShowSettled(true)}>保存済みを表示</Button>}
      />
    );
  }
  const listed = showSettled ? candidates : open;
  const counts = listed.reduce<Record<string, number>>((totals, candidate) => {
    totals[candidate.kind] = (totals[candidate.kind] ?? 0) + 1;
    return totals;
  }, {});
  const shown = kind === "all" ? listed : listed.filter((candidate) => candidate.kind === kind);
  const shownSelectable = shown.filter(isSavableCandidate).map((candidate) => candidate.id);
  const selectedCount = selectableIds.filter((id) => selectedIds.has(id)).length;

  return (
    <Card p={0}>
      <Group justify="space-between" p="md" align="flex-start" wrap="wrap">
        <Box>
          <Text fw={700}>保存する候補</Text>
          <Text size="xs" c="dimmed">新作・続編・改稿をまとめて処理 · {selectedCount}件を選択中 · 表示 {shown.length}件</Text>
        </Box>
        <Group gap="xs">
          <Button size="xs" variant="subtle" disabled={!shownSelectable.length} onClick={() => onSelectMany(shownSelectable)}>表示分を選択</Button>
          <Button size="xs" variant="subtle" color="gray" disabled={!selectedCount} onClick={() => onSelectMany([])}>解除</Button>
          <Button size="sm" leftSection={<Icons.export size={IconSize.menu} />} disabled={!selectedCount || running} loading={saving} onClick={onSave}>選択候補をまとめて保存</Button>
        </Group>
      </Group>
      <Box px="md" pb="md">
        <SegmentedControl
          size="xs"
          aria-label="候補の種類"
          value={kind}
          onChange={setKind}
          data={CANDIDATE_KINDS.filter((item) => item.value === "all" || counts[item.value]).map((item) => ({
            value: item.value,
            label: item.value === "all" ? `${item.label} ${listed.length}` : `${item.label} ${counts[item.value] ?? 0}`,
          }))}
        />
      </Box>
      <Divider />
      <Stack gap={0}>
        {shown.map((candidate) => (
          <Paper key={candidate.id} p="md" radius={0} className="update-candidate">
            <Group wrap="nowrap" align="flex-start">
              <Checkbox
                mt={2}
                checked={selectedIds.has(candidate.id)}
                onChange={(event) => onToggle(candidate.id, event.currentTarget.checked)}
                aria-label={`${candidate.title}を選択`}
                disabled={!isSavableCandidate(candidate)}
              />
              <ProviderMark provider={candidate.source} compact />
              <Stack gap={2} flex={1} miw={0}>
                <Group gap={6} wrap="nowrap">
                  {/* 縮むのは題の側。帯が縮むと「新作」が「新…」になり、種類が読めなくなる。 */}
                  <Badge size="xs" variant="light" color={KIND_COLOR[candidate.kind] ?? "gray"} style={{ flex: "none" }}>{kindLabel(candidate.kind)}</Badge>
                  {/* 題名は折らない。候補は同じ連載の続きが並ぶので、1行で
                      切ると全部が同じ文字列になり、どれを保存するのか
                      決められない。 */}
                  <Text size="sm" fw={650}>{candidate.title}</Text>
                </Group>
                <Text size="xs" c="dimmed" className="line-clamp-1">{candidate.targetLabel} · {candidate.subtitle}</Text>
                {candidate.error && <Text size="xs" c="red" className="line-clamp-2">{errorMessage(candidate.error)}</Text>}
              </Stack>
              <Badge color={candidate.status === "failed" ? "red" : candidate.status === "saved" ? "green" : "gray"} variant="light" style={{ flex: "none" }}>{candidateStatusLabel(candidate.status)}</Badge>
              {isSavableCandidate(candidate) && (
                <Tooltip label="今後この作品を候補に出さない">
                  <ActionIcon variant="subtle" color="gray" aria-label={`${candidate.title}を今後表示しない`} onClick={() => onDismiss(candidate)}>
                    <Icons.hide size={IconSize.action} />
                  </ActionIcon>
                </Tooltip>
              )}
            </Group>
          </Paper>
        ))}
      </Stack>
      {settled.length > 0 && (
        <>
          <Divider />
          <Group justify="space-between" p="md" wrap="nowrap">
            <Text size="xs" c="dimmed">保存済み {settled.length}件{showSettled ? "も表示しています" : "は畳んでいます"}</Text>
            <Button size="compact-xs" variant="subtle" color="gray" onClick={() => setShowSettled((current) => !current)}>{showSettled ? "隠す" : "表示"}</Button>
          </Group>
        </>
      )}
      {hasMore && <Box p="md"><Button fullWidth variant="default" onClick={onLoadMore}>次の候補を読み込む</Button></Box>}
    </Card>
  );
}

/** 対象が何日ヒットしなければ「休眠」と見なすか。 */
const DORMANT_DAYS = 180;
/** 何回続けて失敗したら知らせるか。 */
const FAILING_STREAK = 3;

/**
 * 対象の様子を一目で。
 *
 * 監視対象は増える一方で、放っておくと「もう更新の来ない作者」と「ずっと
 * 失敗している対象」が同じ顔で並ぶ。どちらも確認のたびに時間を使うので、
 * 畳むか外すかを決められるようにする。
 */
export function TargetHealthBadge({ target }: { target: UpdateTarget }) {
  const errors = target.consecutiveErrors ?? 0;
  if (errors >= FAILING_STREAK) {
    return <Tooltip label={`${errors}回続けて確認に失敗しています`}><Badge size="xs" color="red" variant="light" style={{ flex: "none" }}>確認できていません</Badge></Tooltip>;
  }
  if (!target.enabled) return <Badge size="xs" color="gray" variant="light" style={{ flex: "none" }}>停止中</Badge>;
  const reference = target.lastHitAt ?? target.createdAt;
  const days = (Date.now() - new Date(reference).getTime()) / 86_400_000;
  if (Number.isFinite(days) && days >= DORMANT_DAYS) {
    return <Tooltip label={`${Math.round(days)}日、新しいものが見つかっていません`}><Badge size="xs" color="gray" variant="light" style={{ flex: "none" }}>休眠</Badge></Tooltip>;
  }
  return null;
}

function isSavableCandidate(candidate: UpdateJobSnapshot["candidates"][number]) {
  return isSavableCandidateStatus(candidate.status);
}

function candidateStatusLabel(status: UpdateJobSnapshot["candidates"][number]["status"]): string {
  return ({ candidate: "候補", queued: "待機中", running: "保存中", saved: "保存済み", failed: "失敗", skipped: "スキップ", done: "処理済み" } as Record<string, string>)[status] ?? status;
}

/** 自動確認の状態を一行で。設定画面を開かなくても、いまの約束が読める。 */
function describeSchedule(schedule: UpdateScheduleSettings | undefined): string {
  if (!schedule) return "…";
  const mode = schedule.mode === "auto_save" ? "確認して保存" : "確認のみ";
  const when: string[] = [];
  if (schedule.onStartup) when.push("起動時");
  if (schedule.intervalHours > 0) when.push(`${schedule.intervalHours}時間ごと`);
  if (!when.length) return "オフ";
  return `${when.join(" · ")}・${mode}`;
}

function StatusBadge({ status }: { status: string }) {
  const item = UPDATE_JOB_STATUS_META[status as keyof typeof UPDATE_JOB_STATUS_META] ?? { color: "gray", label: status };
  return <Badge color={item.color} variant="light" leftSection={(status === "running" || status === "queued") ? <span className="status-dot" /> : undefined}>{item.label}</Badge>;
}

function statusTitle(status: string): string { return ({ queued: "開始を待っています", running: "更新を確認しています", paused: "一時停止中", auth_required: "再接続が必要です", canceling: "停止しています", canceled: "更新確認を中止しました", completed: "更新確認が完了しました", failed: "更新確認で問題が発生しました" } as Record<string, string>)[status] ?? status; }
