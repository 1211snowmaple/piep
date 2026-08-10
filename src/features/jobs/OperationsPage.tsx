import { useMemo, useState } from "react";
import {
  Accordion,
  ActionIcon,
  Badge,
  Box,
  Button,
  Card,
  Group,
  Progress,
  SegmentedControl,
  SimpleGrid,
  Stack,
  Text,
  ThemeIcon,
  Timeline,
  Tooltip,
} from "@mantine/core";
import { Icons, IconSize, type LucideIcon } from "@/lib/icons";
import { PageHeader } from "@/components/PageHeader";
import { RuntimeNotice } from "@/components/RuntimeNotice";
import { isTauriRuntime } from "@/services/dbApi";
import { errorMessage } from "@/lib/format";
import { notifications } from "@mantine/notifications";
import {
  clearCompletedOperations,
  requestOperationCancel,
  retryOperation,
  type OperationJob,
  type OperationKind,
  type OperationStatus,
  useOperationJobs,
} from "./operationJobs";
import { isUpdateJobTerminal, useUpdateJobs, type UpdateJobStatus } from "@/features/updates/updateJobs";

type Filter = "all" | "active" | "failed";

const kindMeta: Record<OperationKind, { label: string; icon: LucideIcon; color: string }> = {
  save: { label: "保存", icon: Icons.collect, color: "blue" },
  update: { label: "更新", icon: Icons.watch, color: "cyan" },
  epub: { label: "EPUB", icon: Icons.read, color: "grape" },
  backup: { label: "バックアップ", icon: Icons.database, color: "indigo" },
  restore: { label: "復元", icon: Icons.restore, color: "violet" },
  search: { label: "検索索引", icon: Icons.search, color: "teal" },
  maintenance: { label: "保守", icon: Icons.database, color: "orange" },
};

const statusMeta: Record<OperationStatus | UpdateJobStatus, { label: string; color: string }> = {
  queued: { label: "待機中", color: "gray" },
  running: { label: "実行中", color: "blue" },
  paused: { label: "一時停止", color: "yellow" },
  auth_required: { label: "再接続が必要", color: "orange" },
  canceling: { label: "中止中", color: "orange" },
  canceled: { label: "中止", color: "gray" },
  completed: { label: "完了", color: "green" },
  failed: { label: "失敗", color: "red" },
  interrupted: { label: "中断", color: "orange" },
};

function timestamp(value: string | null) {
  if (!value) return "—";
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString("ja-JP", { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" });
}

function OperationCard({ job }: { job: OperationJob }) {
  const meta = kindMeta[job.kind];
  const Icon = meta.icon;
  const status = statusMeta[job.status];
  const progress = job.total ? Math.min(100, job.current / job.total * 100) : job.status === "completed" ? 100 : 0;
  return (
    <Card p="lg" data-testid={`operation-${job.kind}`}>
      <Stack gap="md">
        <Group justify="space-between" align="flex-start" wrap="nowrap">
          <Group wrap="nowrap" align="flex-start">
            <ThemeIcon variant="light" color={meta.color} size={42}><Icon size={19} /></ThemeIcon>
            <Box miw={0}>
              <Group gap="xs"><Text fw={700} className="line-clamp-1">{job.label}</Text><Badge size="sm" variant="light" color={status.color}>{status.label}</Badge></Group>
              <Text size="xs" c="dimmed" mt={4}>{meta.label} · {timestamp(job.updatedAt)}</Text>
              {job.detail && <Text size="sm" c="dimmed" mt={4} className="line-clamp-2">{job.detail}</Text>}
            </Box>
          </Group>
          <Group gap={4} wrap="nowrap">
            {job.canRetry && <Tooltip label="再試行"><ActionIcon variant="subtle" aria-label={`${job.label}を再試行`} onClick={() => retryOperation(job.id).catch((error) => notifications.show({ color: "red", message: errorMessage(error) }))}><Icons.retry size={IconSize.action} /></ActionIcon></Tooltip>}
            {job.canCancel && <Tooltip label="キャンセル"><ActionIcon color="red" variant="subtle" aria-label={`${job.label}をキャンセル`} onClick={() => requestOperationCancel(job.id)}><Icons.stop size={IconSize.action} /></ActionIcon></Tooltip>}
          </Group>
        </Group>
        {(job.total !== null || job.status === "running") && <Box><Progress value={progress} animated={job.status === "running" && job.total === null} /><Text size="xs" c="dimmed" mt={5}>{job.total !== null ? `${job.current} / ${job.total}` : "処理中"}</Text></Box>}
        {job.logs.length > 0 && <Accordion variant="contained"><Accordion.Item value="logs"><Accordion.Control>ログ {job.logs.length}件</Accordion.Control><Accordion.Panel><Timeline bulletSize={18} lineWidth={2}>{job.logs.slice().reverse().map((log) => <Timeline.Item key={log.id} color={log.level === "error" ? "red" : log.level === "warn" ? "yellow" : log.level === "success" ? "green" : "blue"} title={<Text size="sm">{log.message}</Text>}><Text size="xs" c="dimmed">{timestamp(log.createdAt)}</Text></Timeline.Item>)}</Timeline></Accordion.Panel></Accordion.Item></Accordion>}
      </Stack>
    </Card>
  );
}

export default function OperationsPage() {
  const runtime = isTauriRuntime();
  const localJobs = useOperationJobs();
  const updateJobs = useUpdateJobs(undefined, runtime);
  const [filter, setFilter] = useState<Filter>("all");
  const visibleLocal = useMemo(() => localJobs.filter((job) => filter === "all" || (filter === "active" ? ["queued", "running", "canceling"].includes(job.status) : ["failed", "interrupted"].includes(job.status))), [filter, localJobs]);
  const visibleUpdates = useMemo(() => updateJobs.jobs.filter((job) => filter === "all" || (filter === "active" ? !isUpdateJobTerminal(job.status) : job.status === "failed")), [filter, updateJobs.jobs]);
  const activeCount = localJobs.filter((job) => ["queued", "running", "canceling"].includes(job.status)).length + updateJobs.jobs.filter((job) => !isUpdateJobTerminal(job.status)).length;
  const failedCount = localJobs.filter((job) => ["failed", "interrupted"].includes(job.status)).length + updateJobs.jobs.filter((job) => job.status === "failed").length;

  return (
    <div className="page page--contained operations-page">
      <PageHeader title="操作履歴" description="保存、更新、EPUB、バックアップ、検索保守の進行状況とログを一か所で確認します。" actions={<Button variant="default" leftSection={<Icons.delete size={IconSize.menu} />} onClick={clearCompletedOperations}>完了履歴を消去</Button>} />
      <RuntimeNotice />
      <SimpleGrid cols={{ base: 1, sm: 3 }} mt="lg">
        <Card p="lg"><Group><ThemeIcon variant="light"><Icons.pending size={IconSize.nav} /></ThemeIcon><Box><Text size="xs" c="dimmed">実行中</Text><Text size="xl" fw={750}>{activeCount}</Text></Box></Group></Card>
        <Card p="lg"><Group><ThemeIcon variant="light" color="green"><Icons.success size={IconSize.nav} /></ThemeIcon><Box><Text size="xs" c="dimmed">記録済み</Text><Text size="xl" fw={750}>{localJobs.length + updateJobs.jobs.length}</Text></Box></Group></Card>
        <Card p="lg"><Group><ThemeIcon variant="light" color={failedCount ? "red" : "gray"}><Icons.failure size={IconSize.nav} /></ThemeIcon><Box><Text size="xs" c="dimmed">要確認</Text><Text size="xl" fw={750}>{failedCount}</Text></Box></Group></Card>
      </SimpleGrid>
      <Group justify="space-between" mt="xl"><SegmentedControl aria-label="操作履歴の絞り込み" value={filter} onChange={(value) => setFilter(value as Filter)} data={[{ value: "all", label: "すべて" }, { value: "active", label: "実行中" }, { value: "failed", label: "要確認" }]} /><Text size="sm" c="dimmed">新しい順</Text></Group>
      <Stack gap="md" mt="md">
        {visibleUpdates.map((job) => {
          const status = statusMeta[job.status];
          const progress = job.totals ? Math.min(100, job.processed / job.totals * 100) : 0;
          return <Card key={`update-${job.jobId}`} p="lg"><Stack gap="md"><Group justify="space-between" align="flex-start"><Group><ThemeIcon variant="light" color="cyan" size={42}><Icons.watch size={IconSize.nav} /></ThemeIcon><Box><Group gap="xs"><Text fw={700}>更新確認 · {job.scope}</Text><Badge color={status.color} variant="light">{status.label}</Badge></Group><Text size="xs" c="dimmed" mt={4}>{timestamp(job.updatedAt)} · 保存 {job.savedCount} · エラー {job.errorCount}</Text></Box></Group><Group gap={4}>{job.status === "running" && <Tooltip label="一時停止"><ActionIcon variant="subtle" aria-label="更新確認を一時停止" onClick={() => updateJobs.pause(job.jobId).then(() => updateJobs.loadJobs())}><Icons.pause size={IconSize.action} /></ActionIcon></Tooltip>}{["paused", "auth_required"].includes(job.status) && <Tooltip label="再開"><ActionIcon variant="subtle" aria-label="更新確認を再開" onClick={() => updateJobs.resume(job.jobId).then(() => updateJobs.loadJobs())}><Icons.resume size={IconSize.action} /></ActionIcon></Tooltip>}{job.status === "failed" && <Tooltip label="失敗項目を再試行"><ActionIcon variant="subtle" aria-label="更新確認を再試行" onClick={() => updateJobs.resume(job.jobId, true).then(() => updateJobs.loadJobs())}><Icons.retry size={IconSize.action} /></ActionIcon></Tooltip>}{!isUpdateJobTerminal(job.status) && <Tooltip label="キャンセル"><ActionIcon variant="subtle" color="red" aria-label="更新確認をキャンセル" onClick={() => updateJobs.cancel(job.jobId).then(() => updateJobs.loadJobs())}><Icons.stop size={IconSize.action} /></ActionIcon></Tooltip>}</Group></Group><Progress value={progress} animated={job.status === "running"} /><Text size="xs" c="dimmed">{job.processed} / {job.totals}{job.activeLabel ? ` · ${job.activeLabel}` : ""}</Text></Stack></Card>;
        })}
        {visibleLocal.map((job) => <OperationCard key={job.id} job={job} />)}
        {!visibleUpdates.length && !visibleLocal.length && <Card p="xl"><Stack align="center" gap="xs"><ThemeIcon size={48} variant="light" color="gray"><Icons.success size={IconSize.feature} /></ThemeIcon><Text fw={700}>表示する操作はありません</Text><Text size="sm" c="dimmed">処理を始めると、進行状況と失敗理由がここに残ります。</Text></Stack></Card>}
      </Stack>
    </div>
  );
}
