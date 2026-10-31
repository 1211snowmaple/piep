import { UpdateWorkMetadata } from "./UpdateWorkMetadata";
import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from "react";
import { transitionContent } from "@/lib/contentTransition";
import {
  ActionIcon,
  Badge,
  Box,
  Button,
  Card,
  Checkbox,
  Divider,
  Group,
  Paper,
  Progress,
  SegmentedControl,
  Select,
  Stack,
  Text,
  ThemeIcon,
  Timeline,
  Title,
  Tooltip,
  UnstyledButton,
} from "@mantine/core";
import { useForm, isNotEmpty } from "@mantine/form";
import { modals } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { reportJobAction } from "@/features/jobs/operationJobs";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Icons, IconSize } from "@/lib/icons";
import { useAppNavigate, useAppSearchParams } from "@/app/router";
import { EmptyState, ErrorState, LoadingState } from "@/components/AsyncState";
import { MotionTabs as Tabs } from "@/components/MotionTabs";
import { PageHeader } from "@/components/PageHeader";
import { updateJobStatusMeta, invalidateAfterUpdateJob, isUpdateJobTerminal, useUpdateJobs, type UpdateJobSnapshot, type UpdateJobSummary } from "@/features/updates/updateJobs";
import { DeferredCandidatesPanel } from "./DeferredCandidatesPanel";
import { UpdateWorkLinks } from "./UpdateWorkLinks";
import { UpdateStatusBadge } from "./UpdateStatusBadge";
import { heldAcquisitionKind, heldReason, summarizeUpdateJobItems, updateJobSubject } from "./updateJobOutcome";
import { acquisitionStatusMeta } from "@/lib/acquisition";
import { errorMessage, formatDate, formatNumber } from "@/lib/format";
import { ProviderMark } from "@/lib/providers";
import { createWatchGroup, deleteUpdateTarget, deleteWatchGroup, isTauriRuntime, listUpdateTargets, listWatchGroups, renameWatchGroup, searchDownloadsV2, setUpdateTargetEnabled, setWatchGroupMembers, upsertUpdateTarget } from "@/services/dbApi";
import { readingWorkIds } from "@/features/library/readingShelf";
import { clearFinishedUpdateJobsCommand, countDismissedUpdateCandidatesCommand, dismissUpdateCandidateCommand, listUpdateJobHeldItemStatesCommand, listUpdateJobItemStatesCommand, restoreDismissedUpdateCandidatesCommand, type UpdateJobItemState } from "@/services/updateJobApi";
import { loadSchedule, saveSchedule, type UpdateScheduleSettings } from "@/features/updates/updateSchedule";
import { UpdateScheduleCard } from "@/features/updates/UpdateScheduleCard";
import { useUpdateJobNotifications } from "@/features/updates/useUpdateScheduler";
import { TargetsPanel, type GroupAction, type TargetAction } from "./WatchTargetsPanel";
import type { UpdateTarget, WatchGroup } from "@/types/library";

export { TargetHealthBadge } from "./WatchTargetsPanel";

const demoSnapshot: UpdateJobSnapshot = {
  jobId: "preview-20260802", status: "completed", scope: "all", mode: "check_only", totals: 43, processed: 43, checkTotal: 43, checkProcessed: 43, saveTotal: 0, saveProcessed: 0, candidateCount: 3, savedCount: 0, errorCount: 0, activeLabel: null,
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

// The update screen unmounts on navigation. Remember manual checks for each
// job so a return visit does not silently select everything again.
const candidateSelectionCache = new Map<string, { seen: Set<number>; selected: number[] }>();

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
const CANDIDATE_DISPLAY_PAGE_SIZE = 60;

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
  const navigate = useAppNavigate();
  const runtime = isTauriRuntime();
  const [searchParams, setSearchParams] = useAppSearchParams();
  const workId = parseWorkId(searchParams.get("work"));
  const queryClient = useQueryClient();
  const updateJobs = useUpdateJobs(undefined, runtime);
  // 確認が終わったら OS の通知で知らせる。画面を見ていないときのための足。
  useUpdateJobNotifications(runtime ? updateJobs.activeSnapshot : null, runtime);
  const [previewSnapshot, setPreviewSnapshot] = useState<UpdateJobSnapshot>(demoSnapshot);
  const activeSnapshot = runtime ? updateJobs.activeSnapshot : previewSnapshot;
  const jobs = runtime ? updateJobs.jobs : [demoSnapshot as UpdateJobSummary];
  const urlTab = searchParams.get("tab");
  const [tab, setTab] = useState<string | null>(urlTab ?? "candidates");
  useEffect(() => setTab(urlTab ?? "candidates"), [urlTab]);
  const selectTab = (next: string | null) => {
    setTab(next);
    const params = new URLSearchParams(searchParams);
    if (next && next !== "candidates") params.set("tab", next);
    else params.delete("tab");
    setSearchParams(params, { replace: true });
  };
  const resultIsTerminal = Boolean(activeSnapshot && isUpdateJobTerminal(activeSnapshot.status));
  // 終わった回の進捗は一行に畳む。走り終えた二本の進捗棒が、毎回その下の
  // 候補を画面の半分まで押し下げていた。開けば元の詳しさで読める。
  const [progressOpen, setProgressOpen] = useState(false);
  useEffect(() => setProgressOpen(false), [activeSnapshot?.jobId]);
  const jobItems = useQuery({
    queryKey: ["update-job-items", activeSnapshot?.jobId, activeSnapshot?.finishedAt],
    queryFn: () => listUpdateJobItemStatesCommand(activeSnapshot!.jobId),
    enabled: runtime && resultIsTerminal,
    staleTime: Infinity,
  });
  const heldItems = useQuery({
    queryKey: ["update-job-held-items", activeSnapshot?.jobId, activeSnapshot?.heldCount],
    queryFn: () => listUpdateJobHeldItemStatesCommand(activeSnapshot!.jobId),
    enabled: runtime && tab === "results" && !resultIsTerminal && (activeSnapshot?.heldCount ?? 0) > 0,
    staleTime: 0,
  });
  const resultItems = resultIsTerminal
    ? jobItems.data ?? null
    : (activeSnapshot?.heldCount ?? 0) > 0 ? heldItems.data ?? null : [];
  const outcome = jobItems.data ? summarizeUpdateJobItems(jobItems.data) : null;
  const [selectedCandidateIds, setSelectedCandidateIds] = useState<number[]>([]);
  // 非表示にした候補は、この画面ではすぐ消す（次のジョブでは元から出てこない）。
  const [dismissedIds, setDismissedIds] = useState<number[]>([]);
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
    { id: 1, targetType: "author", source: "pixiv", sourceKey: "8001234", displayName: "青葉しおり", enabled: true, lastCheckedAt: new Date().toISOString(), lastSeenSourceId: "12002", lastSeenSourceUpdatedAt: null, metadataJson: null, createdAt: new Date().toISOString(), updatedAt: new Date().toISOString(), watchGroups: ["毎日見る"] },
    { id: 2, targetType: "series", source: "pixiv", sourceKey: "778120", displayName: "星を編む人", enabled: true, lastCheckedAt: new Date().toISOString(), lastSeenSourceId: "12001", lastSeenSourceUpdatedAt: null, metadataJson: null, createdAt: new Date().toISOString(), updatedAt: new Date().toISOString(), watchGroups: ["毎日見る"] },
    { id: 3, targetType: "author", source: "fanbox", sourceKey: "mizu-atelier", displayName: "mizu atelier", enabled: false, lastCheckedAt: new Date(Date.now() - 86_400_000).toISOString(), lastSeenSourceId: null, lastSeenSourceUpdatedAt: null, metadataJson: null, createdAt: new Date().toISOString(), updatedAt: new Date().toISOString() },
  ] as UpdateTarget[]) });
  // 利用者が作った監視グループ。確認する範囲にも、自動確認の範囲にも出る。
  const groups = useQuery({ queryKey: ["watch-groups"], queryFn: async (): Promise<WatchGroup[]> => runtime ? listWatchGroups() : [
    { id: 1, name: "毎日見る", sortOrder: 0, createdAt: new Date().toISOString(), updatedAt: new Date().toISOString(), targetIds: [1, 2] },
  ] });
  const groupList = groups.data ?? [];
  /** グループの中で、止めていない対象。確認に送るのはこれだけ。 */
  const enabledMembers = (group: WatchGroup) => {
    const enabled = new Set((targets.data ?? []).filter((target) => target.enabled).map((target) => target.id));
    return group.targetIds.filter((id) => enabled.has(id));
  };
  // The snapshot is polled, so re-seeding the selection from it on every
  // arrival used to re-tick boxes the user had just cleared. Seed once per job,
  // then only add candidates that appear later as the job discovers them.
  const visibleSelectionJob = useRef<string | null>(null);
  useEffect(() => {
    if (!activeSnapshot) return;
    const isNewJob = visibleSelectionJob.current !== activeSnapshot.jobId;
    visibleSelectionJob.current = activeSnapshot.jobId;
    let remembered = candidateSelectionCache.get(activeSnapshot.jobId);
    if (!remembered) {
      remembered = { seen: new Set(), selected: [] };
      candidateSelectionCache.set(activeSnapshot.jobId, remembered);
    }
    const eligible = new Set(activeSnapshot.candidates.filter((item) => isSavableCandidate(item, activeSnapshot.status === "paused")).map((item) => item.id));
    const fresh = activeSnapshot.candidates.filter((item) => item.selected && eligible.has(item.id) && !remembered.seen.has(item.id)).map((item) => item.id);
    eligible.forEach((id) => remembered.seen.add(id));
    if (fresh.length) remembered.selected = [...new Set([...remembered.selected, ...fresh])];
    if (isNewJob || fresh.length) setSelectedCandidateIds(remembered.selected);
  }, [activeSnapshot]);

  const changeCandidateSelection = (update: (current: number[]) => number[]) => {
    const jobId = activeSnapshot?.jobId;
    if (!jobId) return;
    const remembered = candidateSelectionCache.get(jobId);
    if (!remembered) return;
    remembered.selected = update(remembered.selected);
    setSelectedCandidateIds(remembered.selected);
  };

  const allowQueuedSelection = activeSnapshot?.status === "paused";
  const selectableCandidateIds = activeSnapshot?.candidates.filter((item) => isSavableCandidate(item, allowQueuedSelection) && !dismissedIds.includes(item.id)).map((item) => item.id) ?? [];
  const selectableCandidateIdSet = new Set(selectableCandidateIds);
  const selectedSavableIds = selectedCandidateIds.filter((id) => selectableCandidateIdSet.has(id));
  const selectedSavableIdSet = new Set(selectedSavableIds);

  const startMutation = useMutation({
    // `targetIds` は監視対象の一覧やグループから来た確認。その作者・シリーズだけを見る。
    mutationFn: async (values: { scope: string; mode: string; targetIds?: number[] }) => {
      if (!runtime) { await new Promise((resolve) => setTimeout(resolve, 400)); setPreviewSnapshot({ ...demoSnapshot, jobId: `preview-${Date.now()}`, startedAt: new Date().toISOString(), updatedAt: new Date().toISOString() }); return; }
      const group = values.scope.startsWith("group:") ? groupList.find((item) => `group:${item.id}` === values.scope) : null;
      if (values.scope.startsWith("group:") && !group) throw new Error("選んだ監視グループが見つかりません");
      const targetIds = values.targetIds ?? (group ? enabledMembers(group) : null);
      if (targetIds && !targetIds.length) throw new Error(group ? `「${group.name}」に確認できる監視対象がありません（止めている対象は確認しません）` : "確認できる監視対象がありません");
      // 棚を選んだときは、その棚の作品IDを集めて「作品」の確認として投げる。
      // 監視のオン・オフに関わらず、いま棚にあるものを見に行ける。
      const shelfIds = targetIds ? null : await shelfWorkIds(values.scope);
      if (shelfIds && !shelfIds.length) throw new Error("この棚には作品がありません");
      const schedule = await loadSchedule();
      await updateJobs.start({
        scope: (targetIds ? "targets" : shelfIds ? "work" : values.scope) as "all" | "work" | "author" | "series" | "targets",
        mode: values.mode as "check_only" | "auto_save",
        workIds: shelfIds ?? (workId && !targetIds ? [workId] : null),
        targetIds,
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
      changeCandidateSelection((current) => current.filter((id) => id !== candidate.id));
      queryClient.invalidateQueries({ queryKey: ["dismissed-candidates"] });
      invalidateAfterUpdateJob(queryClient);
      notifications.show({
        color: "gray",
        title: "候補から外しました（あとで戻せます）",
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
      notifications.show({ color: "piep", title: "非表示を解除しました", message: `${restored}件を解除しました。監視対象に含まれる作品だけ次の確認に戻ります。閲覧制限は保留を続けます。` });
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
    mutationFn: async (candidateIds: number[]) => { if (runtime && activeSnapshot) await updateJobs.saveCandidates(activeSnapshot.jobId, candidateIds); },
    onSuccess: (_result, candidateIds) => notifications.show({ color: "green", title: "保存を開始しました", message: `${candidateIds.length}件を処理します` }),
    onError: (error) => notifications.show({ color: "red", title: "候補を保存できません", message: errorMessage(error) }),
  });
  const targetMutation = useMutation({
    mutationFn: async (input: TargetAction) => {
      if (!runtime) return;
      if (input.action === "add" && input.values) {
        await upsertUpdateTarget({ ...input.values, enabled: true, metadataJson: null });
        // グループを開いたまま足したなら、そのグループにも入れる。
        if (input.groupId !== undefined) {
          const added = (await listUpdateTargets<UpdateTarget>(input.values.targetType, false))
            .find((target) => target.source === input.values?.source && target.sourceKey === input.values?.sourceKey.trim());
          if (added) await setWatchGroupMembers(input.groupId, [added.id], true);
        }
      }
      if (input.action === "toggle" && input.target) await setUpdateTargetEnabled(input.target.targetType, input.target.source, input.target.sourceKey, Boolean(input.enabled));
      if (input.action === "delete" && input.target) await deleteUpdateTarget(input.target.targetType, input.target.source, input.target.sourceKey);
    },
    onSuccess: () => {
      targetForm.reset();
      queryClient.invalidateQueries({ queryKey: ["update-targets"] });
      queryClient.invalidateQueries({ queryKey: ["watch-groups"] });
    },
    onError: (error) => notifications.show({ color: "red", title: "更新対象を変更できません", message: errorMessage(error) }),
  });
  const bulkEnableMutation = useMutation({
    mutationFn: async ({ targets: chosen, enabled }: { targets: UpdateTarget[]; enabled: boolean }) => {
      if (!runtime) return chosen.length;
      for (const target of chosen) await setUpdateTargetEnabled(target.targetType, target.source, target.sourceKey, enabled);
      return chosen.length;
    },
    onSuccess: (count, { enabled }) => notifications.show({ color: "gray", message: `${formatNumber(count)}件の確認を${enabled ? "再開しました" : "止めました"}` }),
    onError: (error) => notifications.show({ color: "red", title: "更新対象を変更できません", message: errorMessage(error) }),
    onSettled: () => queryClient.invalidateQueries({ queryKey: ["update-targets"] }),
  });
  const groupMutation = useMutation({
    mutationFn: async (input: GroupAction): Promise<number | null> => {
      if (!runtime) return null;
      if (input.action === "create") {
        const id = await createWatchGroup(input.name);
        if (input.addTargetIds?.length) await setWatchGroupMembers(id, input.addTargetIds, true);
        return id;
      }
      if (input.action === "rename") await renameWatchGroup(input.group.id, input.name);
      if (input.action === "delete") {
        await deleteWatchGroup(input.group.id);
        // 自動確認の範囲だったなら、すべてへ戻す。消えたグループを指したままだと、
        // 自動確認が黙って止まる。
        const current = await loadSchedule();
        if (current.groupId === input.group.id) {
          await saveSchedule({ ...current, groupId: null });
          setScheduleRevision((value) => value + 1);
        }
        if (form.values.scope === `group:${input.group.id}`) form.setFieldValue("scope", "all");
      }
      if (input.action === "members") return setWatchGroupMembers(input.group.id, input.targetIds, input.add);
      return null;
    },
    onSuccess: (result, input) => {
      queryClient.invalidateQueries({ queryKey: ["watch-groups"] });
      queryClient.invalidateQueries({ queryKey: ["update-targets"] });
      if (input.action === "members") {
        const changed = result ?? 0;
        const skipped = input.targetIds.length - changed;
        notifications.show({
          color: "gray",
          message: input.add
            ? `「${input.group.name}」に${formatNumber(changed)}件を入れました${skipped > 0 ? `（${formatNumber(skipped)}件は入っていました）` : ""}`
            : `「${input.group.name}」から${formatNumber(changed)}件を外しました`,
        });
      }
      if (input.action === "create" && input.addTargetIds?.length) notifications.show({ color: "gray", message: `「${input.name.trim()}」を作り、${formatNumber(input.addTargetIds.length)}件を入れました` });
    },
    onError: (error, input) => {
      // 名前の入力欄から来たものは、その欄が理由を出す。
      if (input.action === "create" || input.action === "rename") return;
      notifications.show({ color: "red", title: "監視グループを変更できません", message: errorMessage(error) });
    },
  });
  const checkTargets = (targetIds: number[], label: string) => startMutation.mutate(
    { scope: "targets", mode: form.values.mode, targetIds },
    { onSuccess: () => notifications.show({ color: "piep", title: "確認を開始しました", message: `${label}の${formatNumber(targetIds.length)}件を確認します` }) },
  );
  // 自動確認は「いまどうなっているか」だけを一行で名乗る。設定そのものは
  // タブの中にあり、変えたらこの一行も追いつく。
  const [scheduleRevision, setScheduleRevision] = useState(0);
  const schedule = useQuery({
    queryKey: ["update-schedule", scheduleRevision],
    queryFn: () => loadSchedule(),
    staleTime: 0,
  });
  const scheduleSummary = describeSchedule(schedule.data, groups.data);
  // A log records that a limit happened in the past. The active label records
  // whether this worker is waiting for that retry right now.
  const throttledMessage = activeSnapshot?.status === "running" &&
    activeSnapshot.activeLabel?.startsWith("取得が制限されています")
    ? activeSnapshot.activeLabel
    : null;
  // 残りの数。タブの脇に出るのは、これから決めるものの件数でなければならない。
  const openCandidateCount = (activeSnapshot?.candidates ?? [])
    .filter((candidate) => !dismissedIds.includes(candidate.id) && candidate.status !== "held" && !isSettledCandidateStatus(candidate.status)).length;
  const finishedJobCount = jobs.filter((job) => isUpdateJobTerminal(job.status)).length;
  const status = activeSnapshot?.status;
  const running = status === "queued" || status === "running" || status === "canceling";
  // 一時停止と再接続待ちは、どちらも「止まっていて、押せば続きから動く」。
  // 再接続が要るほうにだけボタンが無いと、連携し直しても戻る道が無くなる。
  const stalled = status === "paused" || status === "auth_required";
  const checkTotal = activeSnapshot?.checkTotal ?? (activeSnapshot?.scope === "save" ? 0 : activeSnapshot?.totals ?? 0);
  const checkProcessed = activeSnapshot?.checkProcessed ?? (activeSnapshot?.scope === "save" ? 0 : activeSnapshot?.processed ?? 0);
  const saveTotal = activeSnapshot?.saveTotal ?? (activeSnapshot?.scope === "save" ? activeSnapshot.totals : 0);
  const saveProcessed = activeSnapshot?.saveProcessed ?? (activeSnapshot?.scope === "save" ? activeSnapshot.processed : 0);
  const finishingProfiles = Boolean(running && activeSnapshot?.activeLabel?.startsWith("作者・シリーズ情報を確認しています"));
  const profileProgress = activeSnapshot?.activeLabel?.match(/（(\d+)\/(\d+)件完了）/);

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
            data={[
              { value: "all", label: "すべての監視対象" }, { value: "work", label: "監視中の作品" }, { value: "author", label: "作者・クリエイター" }, { value: "series", label: "シリーズ" },
              { value: "favorite", label: `お気に入りの作品（最大${SHELF_SCOPE_LIMIT}件）` }, { value: "reading", label: `読みかけの作品（最大${SHELF_SCOPE_LIMIT}件）` },
              ...(groupList.length ? [{ group: "監視グループ", items: groupList.map((group) => ({ value: `group:${group.id}`, label: `${group.name}（${formatNumber(enabledMembers(group).length)}件）` })) }] : []),
            ]}
            allowDeselect={false}
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
              label="取得元への確認は1件ずつ2秒以上あけます。取得エラーが続くと間隔を延ばして一時停止します。取得制限では分単位で待って再試行します。Pixiv側の表示設定で一覧から省かれた作品は個別に確認します。"
            >
              <ActionIcon variant="subtle" color="gray" size="md" aria-label="確認の進め方について">
                <Icons.help size={IconSize.menu} />
              </ActionIcon>
            </Tooltip>
          </Group>
          {workId && <Badge variant="light" color="gray">作品 ID {workId} のみ</Badge>}
          {(form.values.scope === "favorite" || form.values.scope === "reading") && (
            <Text size="xs" c="dimmed" role="status">
              この確認では最大{SHELF_SCOPE_LIMIT}件を対象にします。{SHELF_SCOPE_LIMIT + 1}件目以降は含まれません。
            </Text>
          )}
          <Box style={{ flex: 1 }} />
          <Group gap={6} wrap="nowrap">
            <Text size="xs" c="dimmed">自動確認：{scheduleSummary}</Text>
            <Button size="xs" variant="subtle" color="gray" onClick={() => selectTab("schedule")}>変更</Button>
          </Group>
        </Group>
      </Paper>

      {activeSnapshot && resultIsTerminal && !progressOpen && (
        <Card px="md" py="sm" className="update-progress-card update-progress-card--compact" mt="lg">
          <Group gap="sm" wrap="nowrap">
            <StatusBadge job={activeSnapshot} />
            <Text size="sm" fw={650} style={{ flex: "none" }}>{statusTitle(activeSnapshot.status)}</Text>
            <Text size="xs" c="dimmed" className="line-clamp-1" style={{ flex: 1, minWidth: 0 }}>
              {formatDate(activeSnapshot.finishedAt ?? activeSnapshot.updatedAt, true)} · 候補 {formatNumber(activeSnapshot.candidateCount)} · 保存済み {formatNumber(activeSnapshot.savedCount)} · エラー {formatNumber(activeSnapshot.errorCount)} · 保留 {formatNumber(activeSnapshot.heldCount ?? 0)}
            </Text>
            {(activeSnapshot.status === "failed" || activeSnapshot.status === "canceled") && <Button size="xs" leftSection={<Icons.undo size={IconSize.inline} />} onClick={() => runtime && reportJobAction(updateJobs.resume(activeSnapshot.jobId, true), "やり直せません")}>失敗分を再試行</Button>}
            <Button size="xs" variant="subtle" color="gray" rightSection={<Icons.expand size={IconSize.inline} />} aria-expanded={false} onClick={() => setProgressOpen(true)}>詳しく</Button>
          </Group>
        </Card>
      )}

      {activeSnapshot && (!resultIsTerminal || progressOpen) && (
        <Card p="lg" className="update-progress-card" mt="lg">
          <Group justify="space-between" align="center" wrap="wrap">
            <Group gap="xs"><StatusBadge job={activeSnapshot} /><Text size="xs" c="dimmed">{activeSnapshot.jobId}</Text></Group>
            <Group gap="xs">
              {resultIsTerminal && <Button variant="subtle" color="gray" rightSection={<Icons.expand size={IconSize.inline} style={{ transform: "rotate(180deg)" }} />} aria-expanded onClick={() => setProgressOpen(false)}>畳む</Button>}
              {running && <Button variant="default" leftSection={<Icons.pause size={IconSize.menu} />} onClick={() => runtime && reportJobAction(updateJobs.pause(activeSnapshot.jobId), "一時停止できません")}>一時停止</Button>}
              {stalled && <Button leftSection={<Icons.resume size={IconSize.menu} />} onClick={() => runtime && reportJobAction(updateJobs.resume(activeSnapshot.jobId), "再開できません")}>再開</Button>}
              {(running || stalled) && <Button variant="subtle" color="red" leftSection={<Icons.cancel size={IconSize.menu} />} onClick={() => runtime && reportJobAction(updateJobs.cancel(activeSnapshot.jobId), "中止できません")}>中止</Button>}
              {((stalled && activeSnapshot.errorCount > 0) || activeSnapshot.status === "failed" || activeSnapshot.status === "canceled") && <Button leftSection={<Icons.undo size={IconSize.menu} />} onClick={() => runtime && reportJobAction(updateJobs.resume(activeSnapshot.jobId, true), "やり直せません")}>失敗分を再試行</Button>}
            </Group>
          </Group>
          <ScrollingJobTitle label={throttledMessage ? statusTitle(activeSnapshot.status) : activeSnapshot.activeLabel || statusTitle(activeSnapshot.status)} />
          <Text size="sm" c="dimmed" mt={5}>候補 {formatNumber(activeSnapshot.candidateCount)} · 保存済み {formatNumber(activeSnapshot.savedCount)} · エラー {formatNumber(activeSnapshot.errorCount)} · 保留 {formatNumber(activeSnapshot.heldCount ?? 0)}{(activeSnapshot.knownHeldCount ?? 0) > 0 && `（ほかに前回から変わらない保留 ${formatNumber(activeSnapshot.knownHeldCount ?? 0)}件）`}</Text>
          {outcome && activeSnapshot.mode === "check_only" && (
            <Text size="sm" mt="xs">
              変更なし・スキップ {formatNumber(outcome.unchanged)}件 · 公開元で見つからない {formatNumber(outcome.missing.length)}件
              {outcome.access.length > 0 && ` · 閲覧条件待ち ${formatNumber(outcome.access.length)}件`}
              {outcome.partial.length > 0 && ` · 一部保存 ${formatNumber(outcome.partial.length)}件`}
              {outcome.errors.length > 0 && ` · 取得エラー ${formatNumber(outcome.errors.length)}件`}
              {outcome.other.length > 0 && ` · その他の保留 ${formatNumber(outcome.other.length)}件`}
              {outcome.checkedTargets > 0 && ` · 作者・シリーズ確認完了 ${formatNumber(outcome.checkedTargets)}件`}
            </Text>
          )}
          {/* 間隔の説明が本当に要る瞬間はここ。「遅い」と感じたときに、
              なぜ待っているのかが同じ場所に出る。 */}
          {running && throttledMessage && (
            <Group gap={6} mt="md" wrap="nowrap" role="status">
              <ThemeIcon size="sm" radius="xl" color="yellow" variant="light"><Icons.pending size={IconSize.inline} /></ThemeIcon>
              <Text size="xs" c="dimmed" className="line-clamp-1">{throttledMessage}</Text>
            </Group>
          )}
          {checkTotal > 0 && <JobStageProgress label="更新を探す" processed={checkProcessed} total={checkTotal} animated={running && checkProcessed < checkTotal} />}
          {(saveTotal > 0 || activeSnapshot.mode === "auto_save" || activeSnapshot.scope === "save") && <JobStageProgress label="見つけた作品を保存" processed={saveProcessed} total={saveTotal} animated={running && saveProcessed < saveTotal} />}
          {finishingProfiles && profileProgress && <JobStageProgress label="作者・シリーズ情報を仕上げる" processed={Number(profileProgress[1])} total={Number(profileProgress[2])} animated={running} />}
          {finishingProfiles && <Text size="xs" c="dimmed" mt="sm">保存後の作者・シリーズ情報を仕上げています。完了表示は、この作業が終わってからになります。</Text>}
        </Card>
      )}

      <Tabs value={tab} onChange={selectTab} mt="lg">
        {/* 三つの束に分ける。「この回」（候補 → 結果 → ログ）、「回をまたいで
            残るもの」（保留候補 → 履歴）、「約束」（何を追いかけるか → いつ
            走らせるか）。数の帯は、伝える中身があるときだけ出す - 0 は何も
            言っていない。 */}
        <Tabs.List>
          <Tabs.Tab value="candidates" leftSection={<Icons.select size={IconSize.menu} />}>候補 {openCandidateCount > 0 && <Badge size="xs" variant="light" ml={4}>{formatNumber(openCandidateCount)}</Badge>}</Tabs.Tab>
          <Tabs.Tab value="results" leftSection={<Icons.confirm size={IconSize.menu} />}>結果 {(activeSnapshot?.heldCount ?? 0) > 0 && <Badge size="xs" variant="light" color="yellow" ml={4}>{formatNumber(activeSnapshot?.heldCount ?? 0)}</Badge>}</Tabs.Tab>
          <Tabs.Tab value="logs" leftSection={<Icons.pending size={IconSize.menu} />}>ログ</Tabs.Tab>
          <span className="updates-tabs__divider" aria-hidden />
          <Tabs.Tab value="deferred" leftSection={<Icons.empty size={IconSize.menu} />}>保留候補</Tabs.Tab>
          <Tabs.Tab value="history" leftSection={<Icons.versionHistory size={IconSize.menu} />}>履歴 {jobs.length > 0 && <Badge size="xs" variant="light" color="gray" ml={4}>{formatNumber(jobs.length)}</Badge>}</Tabs.Tab>
          <span className="updates-tabs__divider" aria-hidden />
          <Tabs.Tab value="targets" leftSection={<Icons.updates size={IconSize.menu} />}>監視対象</Tabs.Tab>
          <Tabs.Tab value="schedule" leftSection={<Icons.publishedDate size={IconSize.menu} />}>自動確認</Tabs.Tab>
        </Tabs.List>

        <Tabs.Panel value="candidates" pt="lg">
          {(activeSnapshot?.heldCount ?? 0) > 0 && <Group mb="md"><Text size="sm" c="dimmed">この回で保留になった{activeSnapshot?.heldCount}件は、結果で理由を確認できます。</Text><Button variant="subtle" size="xs" onClick={() => selectTab("results")}>この回の保留を見る</Button></Group>}
          {activeSnapshot ? (
            <CandidatesPanel
              key={activeSnapshot.jobId}
              candidates={activeSnapshot.candidates.filter((candidate) => !dismissedIds.includes(candidate.id) && candidate.status !== "held")}
              selectedIds={selectedSavableIdSet}
              selectableIds={selectableCandidateIds}
              running={running}
              saving={saveCandidatesMutation.isPending}
              allowQueuedSelection={allowQueuedSelection}
              hasMore={Boolean(activeSnapshot.nextCandidateCursor) || activeSnapshot.candidateCount > activeSnapshot.candidates.length}
              jobActive={running || stalled}
              onToggle={(id, checked) => changeCandidateSelection((current) => checked ? [...new Set([...current, id])] : current.filter((item) => item !== id))}
              onSelectMany={(ids) => changeCandidateSelection((current) => [...new Set([...current, ...ids])])}
              onClearSelection={() => changeCandidateSelection(() => [])}
              onSave={() => saveCandidatesMutation.mutate(selectedSavableIds)}
              onLoadMore={() => runtime && updateJobs.loadMoreCandidates()}
              onDismiss={(candidate) => dismissMutation.mutate(candidate)}
              onOpenWork={(id) => navigate(`/works/${id}`)}
            />
          ) : (
            <EmptyState icon={Icons.versionHistory} title="更新ジョブはまだありません" description="上の対象と方法を選び、「確認を開始」を押してください。" />
          )}
        </Tabs.Panel>

        <Tabs.Panel value="results" pt="lg">
          {activeSnapshot ? (
            <JobResultsPanel
              key={activeSnapshot.jobId}
              job={activeSnapshot}
              subject={updateJobSubject(jobs.find((job) => job.jobId === activeSnapshot.jobId) ?? activeSnapshot)}
              items={resultItems}
              loading={runtime && (resultIsTerminal ? jobItems.isLoading : heldItems.isLoading)}
              error={resultIsTerminal ? jobItems.error : heldItems.error}
              onRetry={() => void (resultIsTerminal ? jobItems.refetch() : heldItems.refetch())}
              onShowLogs={() => selectTab("logs")}
              onShowDeferred={() => selectTab("deferred")}
              onOpenWork={(id) => navigate(`/works/${id}`)}
            />
          ) : <EmptyState icon={Icons.confirm} title="結果はまだありません" description="確認を開始すると、処理結果をここで確認できます。" />}
        </Tabs.Panel>

        <Tabs.Panel value="deferred" pt="lg">
          <DeferredCandidatesPanel runtime={runtime} onOpenWork={(id) => navigate(`/works/${id}`)} onRechecked={(jobId) => { void updateJobs.selectJob(jobId); selectTab("candidates"); }} />
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
                  <Text size="xs" c="dimmed">監視対象の作品だけ次の確認へ戻します。個別に戻すときは「保留・非表示」へ。</Text>
                </Box>
                <Button size="xs" variant="default" loading={restoreDismissedMutation.isPending} onClick={() => restoreDismissedMutation.mutate()}>すべて解除</Button>
              </Group>
            </Card>
          )}
          <TargetsPanel
            targets={targets.data ?? []}
            groups={groupList}
            loading={targets.isLoading}
            error={targets.error}
            retry={() => targets.refetch()}
            form={targetForm}
            mutation={targetMutation}
            groupMutation={groupMutation}
            onSetEnabled={(chosen, enabled) => bulkEnableMutation.mutate({ targets: chosen, enabled })}
            onCheck={checkTargets}
            checkDisabled={running || startMutation.isPending || Boolean(workId)}
          />
        </Tabs.Panel>

        <Tabs.Panel value="history" pt="lg">
          <JobHistoryPanel
            jobs={jobs}
            activeJobId={activeSnapshot?.jobId ?? null}
            finishedCount={finishedJobCount}
            clearingAll={clearFinishedJobsMutation.isPending}
            clearingJobId={clearJobMutation.isPending ? (clearJobMutation.variables ?? null) : null}
            onSelect={(jobId) => {
              if (!runtime) return;
              void updateJobs.selectJob(jobId)
                .then(() => selectTab("results"))
                .catch((error) => notifications.show({ color: "red", title: "履歴を開けません", message: errorMessage(error) }));
            }}
            onClear={(jobId) => clearJobMutation.mutate(jobId)}
            onClearFinished={confirmClearFinishedJobs}
          />
        </Tabs.Panel>

        <Tabs.Panel value="schedule" pt="lg">
          <UpdateScheduleCard groups={groups.data} onChanged={() => setScheduleRevision((value) => value + 1)} />
        </Tabs.Panel>
      </Tabs>
    </div>
  );
}

/** A selected job's outcome, including holds that exist only in its history. */
export function JobResultsPanel({ job, subject, items, loading, error, onRetry, onShowLogs, onShowDeferred, onOpenWork }: {
  job: UpdateJobSnapshot;
  subject?: string;
  items: UpdateJobItemState[] | null;
  loading: boolean;
  error: unknown;
  onRetry: () => void;
  onShowLogs: () => void;
  onShowDeferred: () => void;
  onOpenWork?: (downloadId: number) => void;
}) {
  const [visibleHeldCount, setVisibleHeldCount] = useState(50);
  const terminal = isUpdateJobTerminal(job.status);
  if (terminal && loading) return <LoadingState />;
  if (terminal && error) return <ErrorState error={error} retry={onRetry} />;
  if (terminal && !items) return <EmptyState icon={Icons.confirm} title="結果の内訳はデスクトップアプリで確認できます" description="候補とログは各タブで確認できます。" />;
  const result = summarizeUpdateJobItems(items ?? []);
  return <Card p="lg"><Stack gap="md">
    <Group justify="space-between" align="flex-start">
      <Box><Text fw={700}>{terminal ? "この回の結果" : "この回の途中結果"}</Text>{subject && <Text size="sm">{subject}</Text>}<Text size="xs" c="dimmed">{formatDate(job.startedAt, true)} · {formatNumber(job.processed)}/{formatNumber(job.totals)}{terminal ? "件の処理が終了" : "件を処理済み · 確認を続けています"}</Text></Box>
      <Button size="xs" variant="default" onClick={onShowLogs}>すべてのログを見る</Button>
    </Group>
    <Text size="sm">
      {terminal && (job.mode === "save" ? `保存 ${formatNumber(job.savedCount)}件 · スキップ ${formatNumber(result.skippedCandidates)}件` : `変更なし・スキップ ${formatNumber(result.unchanged)}件`)}
      {!terminal && `保留 ${formatNumber(job.heldCount ?? 0)}件`}
      {result.missing.length > 0 && ` · 公開元で見つからない ${formatNumber(result.missing.length)}件`}
      {result.access.length > 0 && ` · 閲覧条件待ち ${formatNumber(result.access.length)}件`}
      {result.partial.length > 0 && ` · 一部保存 ${formatNumber(result.partial.length)}件`}
      {result.errors.length > 0 && ` · 取得エラー ${formatNumber(result.errors.length)}件`}
      {result.other.length > 0 && ` · その他の保留 ${formatNumber(result.other.length)}件`}
      {terminal && result.checkedTargets > 0 && ` · 作者・シリーズ確認完了 ${formatNumber(result.checkedTargets)}件`}
      {job.candidateCount > 0 && ` · 候補 ${formatNumber(job.candidateCount)}件`}
      {(!terminal || job.mode !== "save") && job.savedCount > 0 && ` · 保存 ${formatNumber(job.savedCount)}件`}
      {job.errorCount > 0 && ` · エラー ${formatNumber(job.errorCount)}件`}
    </Text>
    {result.missing.length > 0 && <Text size="sm" c="dimmed">公開元で見つからなかった投稿の保存済みの内容は、手元に残っています。公開状態が変わった後、作者または作品の確認から再確認できます。</Text>}
    {result.access.length > 0 && <Group justify="space-between"><Text size="sm" c="dimmed">閲覧条件待ちの作品は、条件が変わった後に再確認できます。</Text><Button size="xs" variant="light" onClick={onShowDeferred}>保留・非表示の作品を開く</Button></Group>}
    {!terminal && loading ? <LoadingState /> : !terminal && error ? <ErrorState error={error} retry={onRetry} /> : result.held.length > 0 ? <Stack gap="xs">
      <Text fw={650}>この回で保留になった作品（{formatNumber(result.held.length)}件）</Text>
      {result.held.slice(0, visibleHeldCount).map((item, index) => {
        const reason = heldReason(item);
        return <Paper key={`${item.source}:${item.sourceId}:${index}`} p="sm" withBorder>
          <Group gap="xs" align="flex-start" wrap="nowrap">
            {item.source && <ProviderMark provider={item.source} compact />}
            <Stack gap={3} miw={0} style={{ flex: 1 }}>
              <Text size="sm" fw={600}>{item.title || item.sourceId || "作品"}</Text>
              <UpdateWorkMetadata metadata={item.metadata} />
            </Stack>
            <UpdateStatusBadge title={item.title || item.sourceId || "作品"} {...acquisitionStatusMeta(heldAcquisitionKind(reason))} detail={item.error || "確認を保留しました。保存済みの内容は保持されています。"} />
            {item.source && item.sourceId && <UpdateWorkLinks source={item.source} sourceId={item.sourceId} downloadId={item.downloadId} title={item.title || item.sourceId} onOpenWork={onOpenWork} />}
          </Group>
        </Paper>;
      })}
      {result.held.length > visibleHeldCount && <Button variant="default" onClick={() => setVisibleHeldCount((count) => count + 50)}>残り{formatNumber(result.held.length - visibleHeldCount)}件を表示</Button>}
    </Stack> : <Text size="sm" c="dimmed">この回で保留になった作品はありません。</Text>}
  </Stack></Card>;
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
          <Text size="xs" c="dimmed">選ぶと、その回の結果を開きます</Text>
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
                <StatusBadge job={job} />
                <Stack gap={2} miw={0}>
                  <Group gap="sm" wrap="nowrap"><Text size="sm" fw={job.jobId === activeJobId ? 700 : 500}>{formatDate(job.startedAt, true)}</Text><Text size="sm" className="line-clamp-1">{updateJobSubject(job)}</Text></Group>
                  <Text size="xs" c="dimmed" className="line-clamp-1">
                    {formatNumber(job.processed)}/{formatNumber(job.totals)}件 · 候補 {formatNumber(job.candidateCount)} · 保存 {formatNumber(job.savedCount)}{job.errorCount > 0 ? ` · エラー ${formatNumber(job.errorCount)}` : ""}{job.heldCount ? ` · 保留 ${formatNumber(job.heldCount)}` : ""}{job.knownHeldCount ? ` · 変わらない保留 ${formatNumber(job.knownHeldCount)}` : ""}
                  </Text>
                </Stack>
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
export function ScrollingJobTitle({ label }: { label: string }) {
  const viewport = useRef<HTMLDivElement>(null);
  const content = useRef<HTMLHeadingElement>(null);
  const [overflow, setOverflow] = useState(0);
  useLayoutEffect(() => {
    const measure = () => setOverflow(Math.max(0, (content.current?.scrollWidth ?? 0) - (viewport.current?.clientWidth ?? 0)));
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    if (viewport.current) observer.observe(viewport.current);
    if (content.current) observer.observe(content.current);
    return () => observer.disconnect();
  }, [label]);
  // 見出しを作り直すと、次のラベルは前のアニメーション位置を引き継がない。
  return <Box ref={viewport} className={`update-job-title-viewport${overflow > 0 ? " update-job-title-viewport--scrolling" : ""}`} title={label}>
    <Title key={label} ref={content} order={2} className="update-job-title" style={{ "--title-overflow": `${overflow}px`, "--title-duration": `${Math.max(9, overflow / 28 + 4)}s` } as CSSProperties}>{label}</Title>
  </Box>;
}

function JobStageProgress({ label, processed, total, animated }: { label: string; processed: number; total: number; animated: boolean }) {
  const value = total > 0 ? Math.min(100, processed / total * 100) : 0;
  return <Box mt="md">
    <Group justify="space-between" gap="sm" mb={5}>
      <Text size="sm" fw={600}>{label}</Text>
      <Text size="sm" c="dimmed">{formatNumber(processed)} / {formatNumber(total)}件</Text>
    </Group>
    <Progress value={value} animated={animated} size="md" aria-label={`${label} ${Math.round(value)}%`} />
  </Box>;
}

export function CandidatesPanel({ candidates, selectedIds, selectableIds, running, saving, allowQueuedSelection, hasMore, jobActive, onToggle, onSelectMany, onClearSelection, onSave, onLoadMore, onDismiss, onOpenWork }: {
  candidates: UpdateJobSnapshot["candidates"];
  selectedIds: Set<number>;
  selectableIds: number[];
  running: boolean;
  saving: boolean;
  allowQueuedSelection: boolean;
  hasMore: boolean;
  jobActive: boolean;
  onToggle: (id: number, checked: boolean) => void;
  onSelectMany: (ids: number[]) => void;
  onClearSelection: () => void;
  onSave: () => void;
  onLoadMore: () => void;
  onDismiss: (candidate: UpdateJobSnapshot["candidates"][number]) => void;
  onOpenWork?: (downloadId: number) => void;
}) {
  const [kind, setKind] = useState<string>("all");
  // 済んだものは畳む。数は残す - 「無かったこと」にはしない。
  const [showSettled, setShowSettled] = useState(false);
  const [visibleCount, setVisibleCount] = useState(CANDIDATE_DISPLAY_PAGE_SIZE);
  if (!candidates.length) {
    return jobActive
      ? <EmptyState icon={Icons.pending} title="更新確認を続けています" description="候補が見つかると、ここに順次表示します。" />
      : <EmptyState icon={Icons.confirm} title="保存できる候補はありません" description="この回の保留と理由は「結果」で確認できます。" />;
  }
  const settled = candidates.filter((candidate) => isSettledCandidateStatus(candidate.status));
  const open = candidates.filter((candidate) => !isSettledCandidateStatus(candidate.status));
  if (!open.length && !showSettled && jobActive) {
    return <EmptyState icon={Icons.pending} title="更新確認を続けています" description="いま表示できる候補は処理済みです。確認や保存が終わるまで、新しい候補が追加されることがあります。" />;
  }
  if (!open.length && !showSettled && !hasMore) {
    return (
      <EmptyState
        icon={Icons.confirm}
        title="この確認の候補は、ぜんぶ片付きました"
        description={`見つかった${settled.length}件は保存済み・処理済みです。次の確認では、ここに新しいぶんだけ並びます。`}
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
  const visible = shown.slice(0, visibleCount);
  const shownSelectable = shown.filter((candidate) => isSavableCandidate(candidate, allowQueuedSelection)).map((candidate) => candidate.id);
  const selectedCount = selectableIds.filter((id) => selectedIds.has(id)).length;
  const selectedOutsideShown = selectedCount - visible.filter((candidate) => isSavableCandidate(candidate, allowQueuedSelection) && selectedIds.has(candidate.id)).length;

  return (
    <Card p={0}>
      <Group justify="space-between" p="md" align="flex-start" wrap="wrap">
        <Box>
          <Text fw={700}>保存する候補</Text>
          <Text size="xs" c="dimmed">新作・続編・改稿をまとめて処理 · 全候補から{selectedCount}件を選択中 · 表示 {formatNumber(visible.length)}/{formatNumber(shown.length)}件{selectedOutsideShown > 0 ? `（表示外で${selectedOutsideShown}件を選択中）` : ""}</Text>
        </Box>
        <Group gap="xs">
          {/* 右の保存と同じ高さ。低い押しボタンが並ぶと、行の下端がそろわない。 */}
          <Button size="sm" variant="subtle" disabled={!shownSelectable.length} onClick={() => onSelectMany(shownSelectable)}>該当分を選択</Button>
          <Button size="sm" variant="subtle" color="gray" disabled={!selectedCount} onClick={onClearSelection}>選択をすべて解除</Button>
          <Button size="sm" leftSection={<Icons.export size={IconSize.menu} />} disabled={!selectedCount || running} loading={saving} onClick={onSave}>選択候補{selectedCount}件をまとめて保存</Button>
        </Group>
      </Group>
      <Box px="md" pb="md">
        <SegmentedControl
          size="xs"
          aria-label="候補の種類"
          value={kind}
          onChange={(value) => transitionContent(() => { setKind(value); setVisibleCount(CANDIDATE_DISPLAY_PAGE_SIZE); }, {
            content: () => document.querySelector<HTMLElement>(".update-candidates"),
          })}
          data={CANDIDATE_KINDS.filter((item) => item.value === "all" || counts[item.value]).map((item) => ({
            value: item.value,
            label: item.value === "all" ? `${item.label} ${listed.length}` : `${item.label} ${counts[item.value] ?? 0}`,
          }))}
        />
      </Box>
      <Divider />
      <Stack gap={0} className="update-candidates">
        {visible.map((candidate) => (
          <Paper key={candidate.id} p="md" radius={0} className="update-candidate">
            <Group wrap="nowrap" align="flex-start">
              <Checkbox
                mt={2}
                checked={selectedIds.has(candidate.id)}
                onChange={(event) => onToggle(candidate.id, event.currentTarget.checked)}
                aria-label={`${candidate.title}を選択`}
                disabled={!isSavableCandidate(candidate, allowQueuedSelection)}
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
                <UpdateWorkMetadata metadata={candidate.metadata} fallback={[candidate.targetLabel, candidate.subtitle].filter(Boolean).join(" · ")} />
              </Stack>
              <UpdateStatusBadge title={candidate.title} {...candidateStatusMeta(candidate)} detail={candidate.error} />
              <UpdateWorkLinks source={candidate.source} sourceId={candidate.sourceId} downloadId={candidate.downloadId} title={candidate.title} onOpenWork={onOpenWork} />
              {isSavableCandidate(candidate) && (
                <Tooltip label="候補から外す（保留・非表示の一覧から戻せます）">
                  <ActionIcon variant="subtle" color="gray" aria-label={`${candidate.title}を候補から外す`} onClick={() => onDismiss(candidate)}>
                    <Icons.hide size={IconSize.action} />
                  </ActionIcon>
                </Tooltip>
              )}
            </Group>
          </Paper>
        ))}
      </Stack>
      {shown.length > visible.length && <Box p="md"><Button fullWidth variant="subtle" onClick={() => setVisibleCount((count) => count + CANDIDATE_DISPLAY_PAGE_SIZE)}>さらに表示（残り{formatNumber(shown.length - visible.length)}件）</Button></Box>}
      {settled.length > 0 && (
        <>
          <Divider />
          <Group justify="space-between" p="md" wrap="nowrap">
            <Text size="xs" c="dimmed">保存済み {settled.length}件{showSettled ? "も表示しています" : "は畳んでいます"}</Text>
            <Button size="compact-xs" variant="subtle" color="gray" onClick={() => { setShowSettled((current) => !current); setVisibleCount(CANDIDATE_DISPLAY_PAGE_SIZE); }}>{showSettled ? "隠す" : "表示"}</Button>
          </Group>
        </>
      )}
      {hasMore && <Box p="md"><Button fullWidth variant="default" onClick={onLoadMore}>次の候補を読み込む</Button></Box>}
    </Card>
  );
}

function isSavableCandidate(candidate: UpdateJobSnapshot["candidates"][number], allowQueued = false) {
  return isSavableCandidateStatus(candidate.status) || (allowQueued && candidate.status === "queued");
}

function candidateStatusLabel(status: UpdateJobSnapshot["candidates"][number]["status"]): string {
  return ({ candidate: "候補", queued: "待機中", running: "保存中", saved: "保存済み", failed: "失敗", held: "保留", skipped: "スキップ", done: "処理済み" } as Record<string, string>)[status] ?? status;
}

function candidateStatusMeta(candidate: UpdateJobSnapshot["candidates"][number]): { label: string; color: string } {
  if (candidate.status === "held" || candidate.status === "failed") {
    const kind = heldAcquisitionKind(heldReason({ error: candidate.error ?? null }));
    return acquisitionStatusMeta(kind === "held" && candidate.status === "failed" ? "error" : kind);
  }
  return { label: candidateStatusLabel(candidate.status), color: candidate.status === "saved" ? "green" : "gray" };
}

/** 自動確認の状態を一行で。設定画面を開かなくても、いまの約束が読める。 */
function describeSchedule(schedule: UpdateScheduleSettings | undefined, groups: WatchGroup[] | undefined): string {
  if (!schedule) return "…";
  const mode = schedule.mode === "auto_save" ? "確認して保存" : "確認のみ";
  const when: string[] = [];
  if (schedule.onStartup) when.push("起動時");
  if (schedule.intervalHours > 0) when.push(`${schedule.intervalHours}時間ごと`);
  if (!when.length) return "オフ";
  if (schedule.groupId === null) return `${when.join(" · ")}・${mode}`;
  if (!groups) return "…";
  const group = groups.find((item) => item.id === schedule.groupId);
  return group ? `${when.join(" · ")}・「${group.name}」を${mode}` : "グループが見つからず停止中";
}

function StatusBadge({ job }: { job: UpdateJobSummary }) {
  const item = updateJobStatusMeta(job);
  return <Badge color={item.color} variant="light" leftSection={(job.status === "running" || job.status === "queued") ? <span className="status-dot" /> : undefined}>{item.label}</Badge>;
}

function statusTitle(status: string): string { return ({ queued: "開始を待っています", running: "更新を確認しています", paused: "一時停止中", auth_required: "再接続が必要です", canceling: "停止しています", canceled: "更新確認を中止しました", completed: "更新確認が完了しました", failed: "更新確認で問題が発生しました" } as Record<string, string>)[status] ?? status; }
