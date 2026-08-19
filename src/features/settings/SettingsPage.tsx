import { useEffect, useRef, useState } from "react";
import {
  ActionIcon,
  Alert,
  Avatar,
  Badge,
  Box,
  Button,
  Card,
  Code,
  Divider,
  Grid,
  Group,
  Modal,
  NavLink,
  PasswordInput,
  Progress,
  SegmentedControl,
  Stack,
  Switch,
  Text,
  TextInput,
  ThemeIcon,
  Title,
  useMantineColorScheme,
} from "@mantine/core";
import { useForm, isNotEmpty, type UseFormReturnType } from "@mantine/form";
import { modals } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Icons, IconSize, type LucideIcon } from "@/lib/icons";
import { Note } from "@/components/Note";
import { PiepLockup } from "@/components/PiepLockup";
import { ErrorState, LoadingState } from "@/components/AsyncState";
import { JobProgress } from "@/components/JobProgress";
import { PageHeader } from "@/components/PageHeader";
import { useAppSearchParams } from "@/app/router";
import { RuntimeNotice } from "@/components/RuntimeNotice";
import { applyDensity, isDense } from "@/lib/density";
import { PAGE_SIZE_OPTIONS, usePageSize, usePagingMode, type PagingMode } from "@/components/ListPager";
import { errorMessage, formatBytes, formatNumber } from "@/lib/format";
import { getProvider, providers } from "@/lib/providers";
import {
  exportAllMultipart,
  getStoragePath,
  importBackupFile,
  inspectBackupFile,
  multipartManifestPath,
  type BackupFormat,
  type BackupInspection,
} from "@/services/archiveApi";
import { loginFanboxWebview, loginPixivWebview, verifyFanboxSession, verifyPixivToken } from "@/services/authApi";
import { getSearchIndexStatus, getStats, isTauriRuntime, scanAndReimportDownloads } from "@/services/dbApi";
import { openSingleDialog, saveDialog } from "@/services/dialogApi";
import { openFilesystemPath } from "@/services/openerApi";
import { useSearchIndexProgress } from "@/features/search/searchIndexProgress";
import DiagnosticsPage from "@/features/diagnostics/DiagnosticsPage";
import { cancelSearchRebuildIndex, startSearchRebuildIndex, type SearchRebuildProgress } from "@/services/searchApi";
import { store } from "@/store";
import { requestOperationCancel, startOperation, type OperationController } from "@/features/jobs/operationJobs";
import { APP_VERSION } from "@/lib/version";
import { AppUpdateCard } from "@/features/settings/AppUpdateCard";

type Section = "connections" | "library" | "search" | "diagnostics" | "appearance" | "about";
const SECTIONS: Section[] = ["connections", "library", "search", "diagnostics", "appearance", "about"];
function isSection(value: string | null): value is Section {
  return value !== null && (SECTIONS as string[]).includes(value);
}
interface PixivUser { id: string; name: string; profile_image_urls?: { medium?: string } }
interface FanboxUser { userId: string; name: string; iconUrl?: string | null }
interface ConnectionState { pixiv: PixivUser | null; fanbox: FanboxUser | null }
export interface BackupReview { path: string; format: BackupFormat; inspection: BackupInspection }

export default function SettingsPage() {
  const runtime = isTauriRuntime();
  const [searchParams, setSearchParams] = useAppSearchParams();
  const requestedSection = searchParams.get("section");
  // Derived from the URL rather than mirrored into state, so "piepについて" in
  // the header switches sections even when Settings is already open, and the
  // history buttons move between sections.
  const section: Section = isSection(requestedSection) ? requestedSection : "connections";
  const setSection = (next: Section) => {
    const params = new URLSearchParams(searchParams);
    if (next === "connections") params.delete("section"); else params.set("section", next);
    setSearchParams(params, { replace: true });
  };
  const queryClient = useQueryClient();
  // Read from the shared store rather than only tracking runs this page
  // started: the app catches the index up on its own at launch, and this screen
  // has to show that run too.
  const rebuild = useSearchIndexProgress();
  const rebuildOperationRef = useRef<OperationController | null>(null);
  const manualJobRef = useRef<string | null>(null);
  const [restoreReview, setRestoreReview] = useState<BackupReview | null>(null);
  const { colorScheme, setColorScheme } = useMantineColorScheme();
  const auth = useQuery({
    queryKey: ["settings-auth"],
    queryFn: async (): Promise<ConnectionState> => {
      if (!runtime) return { pixiv: null, fanbox: null };
      const [pixivUser, fanboxUser] = await Promise.all([store.get<PixivUser>("pixiv_user"), store.get<FanboxUser>("fanbox_user")]);
      return { pixiv: pixivUser ?? null, fanbox: fanboxUser ?? null };
    },
  });
  const stats = useQuery({ queryKey: ["stats"], queryFn: () => runtime ? getStats() : Promise.resolve({ totalDownloads: 1284, pixivCount: 936, fanboxCount: 348, totalAssets: 8241, totalSizeBytes: 14_680_000_000 }) });
  const storagePath = useQuery({ queryKey: ["storage-path"], queryFn: () => runtime ? getStoragePath() : Promise.resolve("C:\\Users\\preview\\AppData\\Roaming\\com.hiron.piep\\downloads") });
  const index = useQuery({ queryKey: ["search-index-status"], queryFn: () => runtime ? getSearchIndexStatus() : Promise.resolve({ totalDownloads: 1284, indexedDownloads: 1284, pendingDownloads: 0, isComplete: true, phase: "ready", indexedChunks: 4192, semanticIndexedChunks: 4192, semanticModelReady: true, embeddingProvider: "DirectML (preview)", gpuEnabled: true }) });
  const pixivForm = useForm({ initialValues: { token: "" }, validate: { token: isNotEmpty("リフレッシュトークンを入力してください") } });
  const fanboxForm = useForm({ initialValues: { session: "", userAgent: "Mozilla/5.0" }, validate: { session: isNotEmpty("FANBOXSESSIDを入力してください") } });
  const connectionMutation = useMutation({
    mutationFn: async (input: { source: "pixiv" | "fanbox"; mode: "web" | "manual"; values?: Record<string, string> }) => {
      if (!runtime) throw new Error("接続はデスクトップアプリで利用できます");
      if (input.source === "pixiv") {
        const [token, user] = input.mode === "web" ? await loginPixivWebview<[string, PixivUser]>() : [input.values?.token ?? "", await verifyPixivToken<PixivUser>(input.values?.token ?? "")];
        await store.set("pixiv_refresh_token", token); await store.set("pixiv_user", user); await store.save();
      } else {
        const [session, user, userAgent] = input.mode === "web" ? await loginFanboxWebview<[string, FanboxUser, string]>() : [input.values?.session ?? "", await verifyFanboxSession<FanboxUser>(input.values?.session ?? "", input.values?.userAgent || "Mozilla/5.0"), input.values?.userAgent || "Mozilla/5.0"];
        await store.set("fanbox_session_id", session); await store.set("fanbox_user", user); await store.set("fanbox_user_agent", userAgent); await store.save();
      }
    },
    onSuccess: (_, input) => { notifications.show({ color: "green", title: "接続しました", message: getProvider(input.source).label }); queryClient.invalidateQueries({ queryKey: ["settings-auth"] }); queryClient.invalidateQueries({ queryKey: ["auth-status"] }); pixivForm.reset(); fanboxForm.reset(); },
    onError: (error) => notifications.show({ color: "red", title: "接続できません", message: errorMessage(error) }),
  });
  const disconnect = (source: "pixiv" | "fanbox") => modals.openConfirmModal({
    title: `${getProvider(source).label}との接続を解除しますか？`, children: <Text size="sm">保存済みの作品は削除されません。再度保存・更新するには接続が必要です。</Text>, labels: { confirm: "接続を解除", cancel: "キャンセル" }, confirmProps: { color: "red" },
    onConfirm: async () => { if (source === "pixiv") { await store.delete("pixiv_refresh_token"); await store.delete("pixiv_user"); } else { await store.delete("fanbox_session_id"); await store.delete("fanbox_user"); } await store.save(); queryClient.invalidateQueries({ queryKey: ["settings-auth"] }); queryClient.invalidateQueries({ queryKey: ["auth-status"] }); },
  });
  const maintenanceMutation = useMutation({
    mutationFn: async (action: "backup" | "restore" | "scan") => {
      if (!runtime) throw new Error("デスクトップアプリで利用できます");
      if (action === "restore") {
        const path = await openSingleDialog({ title: "検査するバックアップを選択", filters: [{ name: "Piep backup", extensions: ["json", "zip"] }] });
        if (!path) return "";
        const review = await inspectBackupFile(path);
        setRestoreReview({ path, ...review });
        return "";
      }
      const operation = startOperation({
        kind: action === "backup" ? "backup" : "maintenance",
        label: action === "backup" ? "ライブラリのバックアップ" : "保存フォルダーの再取り込み",
        onRetry: () => maintenanceMutation.mutate(action),
      });
      try {
        if (action === "backup") {
          const selectedPath = await saveDialog({ title: "ライブラリのバックアップ", defaultPath: `piep-backup-${new Date().toISOString().slice(0, 10)}.json`, filters: [{ name: "Piep multipart backup manifest", extensions: ["json"] }] });
          if (!selectedPath) { operation.cancel("保存先の選択をキャンセルしました"); return ""; }
          const manifestPath = multipartManifestPath(selectedPath);
          operation.log(manifestPath);
          await exportAllMultipart(manifestPath);
          operation.complete("分割バックアップを書き出しました");
          return "分割バックアップを書き出しました。マニフェストと同じフォルダーのZIPパートを一緒に保管してください";
        }
        const count = await scanAndReimportDownloads();
        operation.complete(`${count}件を再取り込みしました`);
        return `${count}件を再取り込みしました`;
      } catch (error) {
        operation.fail(error);
        throw error;
      }
    },
    onSuccess: (message) => { if (message) notifications.show({ color: "green", message }); queryClient.invalidateQueries({ queryKey: ["dashboard"] }); queryClient.invalidateQueries({ queryKey: ["library"] }); queryClient.invalidateQueries({ queryKey: ["stats"] }); },
    onError: (error, action) => notifications.show({
      color: "red",
      title: action === "restore" ? "バックアップを検査できませんでした" : "操作に失敗しました",
      message: action === "restore"
        ? `${errorMessage(error)}。JSONマニフェストを選ぶ場合は、同じフォルダーにすべてのZIPパートが必要です。`
        : errorMessage(error),
    }),
  });
  const restoreMutation = useMutation({
    mutationFn: async ({ path, format, inspection }: BackupReview) => {
      const execute = async (): Promise<void> => {
        const operation = startOperation({ kind: "restore", label: "バックアップの復元", detail: path, total: inspection.workCount, onRetry: execute });
        operation.log(`${inspection.workCount}作品・${inspection.assetCount}アセットを検証済み`);
        try {
          const count = await importBackupFile(path, format);
          operation.progress(count, inspection.workCount);
          operation.complete(`${count}件を復元しました`);
          notifications.show({ color: "green", message: `${count}件を復元しました` });
          setRestoreReview(null);
          await Promise.all([
            queryClient.invalidateQueries({ queryKey: ["dashboard"] }),
            queryClient.invalidateQueries({ queryKey: ["library"] }),
            queryClient.invalidateQueries({ queryKey: ["stats"] }),
          ]);
        } catch (error) {
          operation.fail(error);
          throw error;
        }
      };
      await execute();
    },
    onError: (error, review) => notifications.show({
      color: "red",
      title: "復元を完了できませんでした",
      message: review.format === "multipart"
        ? `${errorMessage(error)}。同じJSONマニフェストをもう一度選ぶと、完了済みパートを保ったまま安全に再開できます。`
        : `${errorMessage(error)}。既存ライブラリは復元ジャーナルにより元の状態へ戻されます。`,
    }),
  });
  const rebuildMutation = useMutation({
    mutationFn: async (includeSemantic: boolean) => {
      if (!runtime) throw new Error("デスクトップアプリで利用できます");
      const pending = index.data?.pendingDownloads ?? index.data?.totalDownloads ?? 0;
      const jobId = await startSearchRebuildIndex({ includeSemantic });
      manualJobRef.current = jobId;
      rebuildOperationRef.current = startOperation({
        kind: "search",
        label: includeSemantic ? "検索インデックスを再構築（意味検索を含む）" : "検索インデックスを再構築",
        total: pending,
        onCancel: () => cancelSearchRebuildIndex(jobId),
        onRetry: () => rebuildMutation.mutate(includeSemantic),
      });
      return jobId;
    },
    onError: (error) => notifications.show({ color: "red", title: "再構築を開始できません", message: errorMessage(error) }),
  });
  // Only runs started here get an entry in the operation history and a toast.
  // An automatic catch-up at launch is reported by the header indicator; it
  // needs no announcement and certainly no notification every time.
  useEffect(() => {
    if (!rebuild || rebuild.jobId !== manualJobRef.current) return;
    rebuildOperationRef.current?.progress(rebuild.processed ?? rebuild.indexedDownloads, rebuild.processedTotal ?? rebuild.totalDownloads);
    if (rebuild.status === "running") return;
    manualJobRef.current = null;
    if (rebuild.status === "failed") {
      rebuildOperationRef.current?.fail(rebuild.error || "検索インデックスの再構築に失敗しました");
      notifications.show({ color: "red", title: "再構築に失敗しました", message: rebuild.error || "検索インデックスの再構築に失敗しました" });
    } else if (rebuild.status === "canceled") {
      rebuildOperationRef.current?.cancel(`${formatNumber(rebuild.processed ?? 0)}件を処理した時点で中止しました`);
      notifications.show({ color: "gray", message: "再構築を中止しました。次回は途中から再開します" });
    } else {
      const failed = rebuild.failed ?? 0;
      const summary = `${formatNumber(rebuild.processed ?? 0)}件を索引しました${failed ? `（${formatNumber(failed)}件は読み込めずスキップしました）` : ""}`;
      rebuildOperationRef.current?.complete(summary);
      notifications.show({ color: failed ? "yellow" : "green", title: "検索インデックスを再構築しました", message: summary });
    }
    rebuildOperationRef.current = null;
    queryClient.invalidateQueries({ queryKey: ["search-index-status"] });
    queryClient.invalidateQueries({ queryKey: ["library-diagnostics"] });
  }, [queryClient, rebuild]);
  // An automatic run still changes the numbers this page shows.
  const automaticFinished = rebuild && rebuild.origin === "automatic" && rebuild.status === "completed";
  useEffect(() => {
    if (automaticFinished) queryClient.invalidateQueries({ queryKey: ["search-index-status"] });
  }, [automaticFinished, queryClient]);

  const nav = [
    { id: "connections" as const, label: "サービス接続", description: "pixiv・FANBOX", icon: Icons.credentials },
    { id: "library" as const, label: "ローカルライブラリ", description: "保存先・バックアップ", icon: Icons.database },
    { id: "search" as const, label: "検索", description: "インデックス・意味検索", icon: Icons.search },
    { id: "diagnostics" as const, label: "診断とメンテナンス", description: "容量・性能・最適化", icon: Icons.diagnostics },
    { id: "appearance" as const, label: "外観と操作", description: "テーマ・表示", icon: Icons.appearance },
    { id: "about" as const, label: "piepについて", description: "バージョン・拡張", icon: Icons.info },
  ];

  return (
    <div className="page page--contained settings-page">
      <PageHeader title="設定" description="接続情報、ローカルデータ、検索と表示を管理します。" />
      <RuntimeNotice />
      <Grid gap="xl" mt="lg" align="flex-start">
        <Grid.Col span={{ base: 12, md: 4, lg: 3 }}><Card p="xs" className="settings-nav">{nav.map((item) => { const Icon = item.icon; return <NavLink component="button" type="button" key={item.id} active={section === item.id} aria-current={section === item.id ? "page" : undefined} label={item.label} description={item.description} leftSection={<Icon size={18} />} onClick={() => setSection(item.id)} />; })}</Card></Grid.Col>
        <Grid.Col span={{ base: 12, md: 8, lg: 9 }}>
          {section === "connections" && (auth.isLoading ? <LoadingState label="接続状態を確認しています" /> : auth.error ? <ErrorState error={auth.error} retry={() => auth.refetch()} /> : <ConnectionsSection auth={auth.data ?? { pixiv: null, fanbox: null }} runtime={runtime} pixivForm={pixivForm} fanboxForm={fanboxForm} mutation={connectionMutation} disconnect={disconnect} />)}
          {section === "library" && (stats.isLoading || storagePath.isLoading ? <LoadingState label="ライブラリ情報を読み込んでいます" /> : stats.error || storagePath.error ? <ErrorState error={stats.error ?? storagePath.error} retry={() => { stats.refetch(); storagePath.refetch(); }} /> : <LibrarySection stats={stats.data} path={storagePath.data} runtime={runtime} pending={maintenanceMutation.isPending} run={(action) => maintenanceMutation.mutate(action)} />)}
          {section === "search" && (index.isLoading ? <LoadingState label="検索インデックスを確認しています" /> : index.error ? <ErrorState error={index.error} retry={() => index.refetch()} /> : <SearchSection status={index.data} rebuild={rebuild} runtime={runtime} rebuilding={rebuildMutation.isPending || rebuild?.status === "running"} start={(includeSemantic) => rebuildMutation.mutate(includeSemantic)} cancel={() => rebuildOperationRef.current ? requestOperationCancel(rebuildOperationRef.current.id) : rebuild ? cancelSearchRebuildIndex(rebuild.jobId) : undefined} />)}
          {section === "diagnostics" && <DiagnosticsPage embedded />}
          {section === "appearance" && <AppearanceSection colorScheme={colorScheme} setColorScheme={setColorScheme} />}
          {section === "about" && <AboutSection runtime={runtime} />}
        </Grid.Col>
      </Grid>
      <RestoreWizard
        review={restoreReview}
        loading={restoreMutation.isPending}
        onClose={() => !restoreMutation.isPending && setRestoreReview(null)}
        onConfirm={() => restoreReview && restoreMutation.mutate(restoreReview)}
      />
    </div>
  );
}

function SectionIntro({ title, description }: { title: string; description: string }) { return <Box mb="lg"><Title order={2}>{title}</Title><Text c="dimmed" size="sm" mt={5}>{description}</Text></Box>; }

type ConnectionInput = { source: "pixiv" | "fanbox"; mode: "web" | "manual"; values?: Record<string, string> };

function ConnectionsSection({ auth, runtime, pixivForm, fanboxForm, mutation, disconnect }: { auth: ConnectionState; runtime: boolean; pixivForm: UseFormReturnType<{ token: string }, { token: string }, any>; fanboxForm: UseFormReturnType<{ session: string; userAgent: string }, { session: string; userAgent: string }, any>; mutation: { mutate: (input: ConnectionInput) => void; isPending: boolean }; disconnect: (source: "pixiv" | "fanbox") => void }) {
  return <Stack gap="lg"><SectionIntro title="サービス接続" description="認証情報は端末内のTauri Storeに保存され、作品取得のときだけ使用します。" /><ConnectionCard source="pixiv" user={auth.pixiv} runtime={runtime} loading={mutation.isPending} onWeb={() => mutation.mutate({ source: "pixiv", mode: "web" })} onDisconnect={() => disconnect("pixiv")} manual={<form onSubmit={pixivForm.onSubmit((values) => mutation.mutate({ source: "pixiv", mode: "manual", values }))}><Group align="flex-start"><PasswordInput flex={1} disabled={!runtime} label="リフレッシュトークン" placeholder="手動で入力" {...pixivForm.getInputProps("token")} /><Button type="submit" mt={25} variant="light" disabled={!runtime}>検証して保存</Button></Group></form>} /><ConnectionCard source="fanbox" user={auth.fanbox} runtime={runtime} loading={mutation.isPending} onWeb={() => mutation.mutate({ source: "fanbox", mode: "web" })} onDisconnect={() => disconnect("fanbox")} manual={<form onSubmit={fanboxForm.onSubmit((values) => mutation.mutate({ source: "fanbox", mode: "manual", values }))}><Stack><PasswordInput disabled={!runtime} label="FANBOXSESSID" placeholder="手動で入力" {...fanboxForm.getInputProps("session")} /><TextInput disabled={!runtime} label="User-Agent" {...fanboxForm.getInputProps("userAgent")} /><Button type="submit" variant="light" disabled={!runtime}>検証して保存</Button></Stack></form>} /><Note icon={Icons.secure} title="ローカル保存">piepは接続情報を外部サーバーへ送信しません。各サービスへの通信とローカル保存だけに使用します。</Note></Stack>;
}

function ConnectionCard({ source, user, runtime, loading, onWeb, onDisconnect, manual }: { source: "pixiv" | "fanbox"; user: PixivUser | FanboxUser | null; runtime: boolean; loading: boolean; onWeb: () => void; onDisconnect: () => void; manual: React.ReactNode }) {
  const provider = getProvider(source); const icon = user ? (source === "pixiv" ? (user as PixivUser).profile_image_urls?.medium : (user as FanboxUser).iconUrl ?? undefined) : undefined; return <Card p="lg"><Group justify="space-between" align="flex-start"><Group><Avatar src={icon} size={54} color="piep">{user ? <Icons.person size={IconSize.feature} /> : provider.icon}</Avatar><Box><Group gap="xs"><Text fw={700}>{provider.label}</Text><Badge color={user ? "green" : "gray"} variant="light">{user ? "接続済み" : "未接続"}</Badge></Group><Text size="sm" c="dimmed" mt={4}>{user?.name || provider.description}</Text></Box></Group>{user ? <Button color="red" variant="subtle" size="xs" leftSection={<Icons.delete size={IconSize.menu} />} onClick={onDisconnect}>接続解除</Button> : <Button disabled={!runtime} loading={loading} leftSection={<Icons.credentials size={IconSize.menu} />} onClick={onWeb}>ブラウザで接続</Button>}</Group>{!user && <><Divider my="lg" label="または手動入力" labelPosition="center" />{manual}</>}</Card>;
}

function LibrarySection({ stats, path, runtime, pending, run }: { stats?: { totalDownloads: number; totalAssets: number; totalSizeBytes: number }; path?: string; runtime: boolean; pending: boolean; run: (action: "backup" | "restore" | "scan") => void }) { return <Stack gap="lg"><SectionIntro title="ローカルライブラリ" description="保存ファイルとデータベースの場所、バックアップを管理します。" /><Grid><Grid.Col span={{ base: 12, sm: 4 }}><Metric icon={Icons.read} label="作品" value={formatNumber(stats?.totalDownloads)} /></Grid.Col><Grid.Col span={{ base: 12, sm: 4 }}><Metric icon={Icons.archive} label="アセット" value={formatNumber(stats?.totalAssets)} /></Grid.Col><Grid.Col span={{ base: 12, sm: 4 }}><Metric icon={Icons.storage} label="使用容量" value={formatBytes(stats?.totalSizeBytes ?? 0)} /></Grid.Col></Grid><Card p="lg"><Text fw={700}>保存先</Text><Group mt="sm" wrap="nowrap"><Code block flex={1}>{path || "読み込み中…"}</Code><ActionIcon variant="light" aria-label="保存先を開く" disabled={!runtime || !path} onClick={() => path && openFilesystemPath(path)}><Icons.openFolder size={IconSize.action} /></ActionIcon></Group></Card><Card p="lg"><Stack gap="md"><Box><Text fw={700}>バックアップと復元</Text><Text size="sm" c="dimmed">大きなライブラリはJSONマニフェストと複数のZIPパートに分けて書き出します。復元には同じフォルダー内の一式が必要です。従来の単一ZIPバックアップも引き続き復元できます。</Text></Box><Group><Button disabled={!runtime} loading={pending} variant="light" leftSection={<Icons.export size={IconSize.menu} />} onClick={() => run("backup")}>分割バックアップを書き出す</Button><Button disabled={!runtime} loading={pending} variant="default" leftSection={<Icons.import size={IconSize.menu} />} onClick={() => run("restore")}>バックアップを復元</Button></Group><Divider /><Box><Text fw={700} size="sm">フォルダーから再取り込み</Text><Text size="xs" c="dimmed" mt={4}>DBにない完全な保存フォルダーだけを検出し、1作品ずつ原子的にライブラリへ戻します。既存作品は手動変更で上書きしません。</Text><Button mt="sm" disabled={!runtime} loading={pending} size="xs" variant="default" leftSection={<Icons.retry size={IconSize.menu} />} onClick={() => run("scan")}>スキャンを実行</Button></Box></Stack></Card></Stack>; }

export function RestoreWizard({ review, loading, onClose, onConfirm }: { review: BackupReview | null; loading: boolean; onClose: () => void; onConfirm: () => void }) {
  const inspection = review?.inspection;
  const hasSpace = inspection?.availableFreeBytes == null || inspection.availableFreeBytes >= inspection.requiredFreeBytes;
  const canRestore = Boolean(inspection?.valid && hasSpace);
  const multipart = review?.format === "multipart";
  return <Modal opened={Boolean(review)} onClose={onClose} withCloseButton={!loading} closeOnClickOutside={!loading} closeOnEscape={!loading} title="バックアップ復元ウィザード" size="lg">
    {review && inspection && <Stack gap="lg">
      <Group gap="xs" role="list" aria-label="復元手順"><Badge component="span" role="listitem" variant="filled">1. 検査済み</Badge><Text c="dimmed" aria-hidden>→</Text><Badge component="span" role="listitem" variant="light" color={canRestore ? "piep" : "gray"}>2. 内容確認</Badge><Text c="dimmed" aria-hidden>→</Text><Badge component="span" role="listitem" variant="light" color="gray">3. {multipart ? "再開可能な復元" : "アトミック復元"}</Badge></Group>
      <Box><Text id="selected-backup-label" size="sm" fw={700}>選択した{multipart ? "JSONマニフェスト" : "ZIPバックアップ"}</Text><Code block mt={6} aria-labelledby="selected-backup-label">{review.path}</Code></Box>
      {!inspection.valid && <Alert color="red" title="このバックアップは復元できません">{inspection.error || "バックアップの整合性検査に失敗しました。"}</Alert>}
      {inspection.valid && !hasSpace && <Alert color="red" title="空き容量が不足しています">復元には一時領域を含めて {formatBytes(inspection.requiredFreeBytes)} 必要ですが、利用可能なのは {formatBytes(inspection.availableFreeBytes ?? 0)} です。</Alert>}
      {inspection.valid && <>
        <Grid>
          <Grid.Col span={{ base: 6, sm: 4 }}><Metric icon={Icons.read} label="作品" value={formatNumber(inspection.workCount)} /></Grid.Col>
          <Grid.Col span={{ base: 6, sm: 4 }}><Metric icon={Icons.person} label="作者" value={formatNumber(inspection.personCount)} /></Grid.Col>
          <Grid.Col span={{ base: 6, sm: 4 }}><Metric icon={Icons.archive} label="シリーズ" value={formatNumber(inspection.seriesCount)} /></Grid.Col>
          <Grid.Col span={{ base: 6, sm: 4 }}><Metric icon={Icons.retry} label="履歴・版" value={formatNumber(inspection.versionCount)} /></Grid.Col>
          <Grid.Col span={{ base: 6, sm: 4 }}><Metric icon={Icons.storage} label="アセット" value={formatNumber(inspection.assetCount)} /></Grid.Col>
          <Grid.Col span={{ base: 6, sm: 4 }}><Metric icon={Icons.database} label="展開後" value={formatBytes(inspection.expandedBytes)} /></Grid.Col>
        </Grid>
        <Card p="md" withBorder>
          <Group justify="space-between"><Text size="sm">{multipart ? "ZIPパート合計" : "ZIPサイズ"}</Text><Text size="sm" fw={700}>{formatBytes(inspection.compressedBytes)}</Text></Group>
          <Group justify="space-between" mt="xs"><Text size="sm">一時領域を含む必要容量</Text><Text size="sm" fw={700}>{formatBytes(inspection.requiredFreeBytes)}</Text></Group>
          <Group justify="space-between" mt="xs"><Text size="sm">利用可能容量</Text><Text size="sm" fw={700}>{inspection.availableFreeBytes == null ? "取得できません" : formatBytes(inspection.availableFreeBytes)}</Text></Group>
          <Group justify="space-between" mt="xs"><Text size="sm">バックアップ形式</Text><Text size="sm" fw={700}>{multipart ? "分割" : "単一ZIP"} · v{inspection.backupVersion ?? "不明"}</Text></Group>
        </Card>
      </>}
      {inspection.warnings.map((warning) => <Alert key={warning} color="yellow">{warning}</Alert>)}
      {inspection.valid && <Note icon={Icons.secure} title={multipart ? "中断しても再開できます" : "安全な復元"}>{multipart
        ? "すべてのZIPパートを先に検査し、パートごとに安全に復元します。途中で止まった場合は、同じJSONマニフェストをもう一度選ぶと続きから再開できます。マニフェストとZIPパートは移動・改名せず一緒に保管してください。"
        : "すべてのファイルを一時領域へ展開・検証してから、データベースと保存ファイルを一括で切り替えます。途中で失敗した場合は復元ジャーナルにより元の状態へ戻します。"}</Note>}
      <Group justify="flex-end"><Button variant="default" disabled={loading} onClick={onClose}>キャンセル</Button><Button color="red" disabled={!canRestore} loading={loading} leftSection={<Icons.import size={IconSize.menu} />} onClick={onConfirm}>検証済みバックアップを復元</Button></Group>
    </Stack>}
  </Modal>;
}

function Metric({ icon: Icon, label, value }: { icon: LucideIcon; label: string; value: string }) { return <Card p="md"><Group><ThemeIcon variant="light"><Icon size={17} /></ThemeIcon><Box><Text size="xs" c="dimmed">{label}</Text><Text fw={750}>{value}</Text></Box></Group></Card>; }

const REBUILD_PHASE_LABEL: Record<string, string> = {
  preparing: "対象を数えています",
  indexing: "本文を解析して索引に書き込んでいます",
  committing: "索引を確定しています",
  ready: "完了しました",
  failed: "失敗しました",
};

function SearchSection({ status, rebuild, runtime, rebuilding, start, cancel }: { status?: { totalDownloads: number; indexedDownloads: number; pendingDownloads: number; isComplete: boolean; indexedChunks: number; semanticIndexedChunks: number; semanticModelReady: boolean; embeddingProvider: string; gpuEnabled: boolean }; rebuild: SearchRebuildProgress | null; runtime: boolean; rebuilding: boolean; start: (includeSemantic: boolean) => void; cancel: () => void }) {
  const [includeSemantic, setIncludeSemantic] = useState(false);
  const indexedRatio = status?.totalDownloads ? status.indexedDownloads / status.totalDownloads * 100 : 100;
  const running = rebuilding && rebuild && rebuild.status === "running";
  return <Stack gap="lg">
    <SectionIntro title="検索" description="全文検索と意味検索のインデックス状態を管理します。" />
    <Card p="lg">
      <Group justify="space-between" align="flex-start">
        <Box>
          <Group gap="xs"><Text fw={700}>全文検索インデックス</Text><Badge color={status?.isComplete ? "green" : "yellow"}>{status?.isComplete ? "最新" : `${formatNumber(status?.pendingDownloads)}件が未反映`}</Badge></Group>
          <Text size="sm" c="dimmed" mt={5}>{formatNumber(status?.indexedDownloads)} / {formatNumber(status?.totalDownloads)}作品</Text>
        </Box>
        <ThemeIcon size={48} variant="light"><Icons.search size={IconSize.feature} /></ThemeIcon>
      </Group>
      <Box mt="lg">
        {running
          ? <JobProgress
              phase={REBUILD_PHASE_LABEL[rebuild.phase ?? ""] ?? "処理中"}
              processed={rebuild.processed ?? 0}
              total={rebuild.processedTotal ?? null}
              rate={rebuild.throughputPerSec ?? null}
              etaSeconds={rebuild.etaSeconds ?? null}
              unit="作品"
              note={rebuild.failed ? `${formatNumber(rebuild.failed)}件は元ファイルを読めずスキップしました` : null}
            />
          : <Progress value={indexedRatio} color={status?.isComplete ? "green" : "yellow"} aria-label="索引の完成度" />}
      </Box>
      <Group mt="lg" justify="space-between" align="flex-end">
        <Group>
          <Button disabled={!runtime || rebuilding} leftSection={<Icons.retry size={IconSize.menu} />} onClick={() => start(includeSemantic)}>
            {status?.isComplete ? "インデックスを再構築" : "未反映分を索引する"}
          </Button>
          {rebuilding && <Button variant="subtle" color="red" leftSection={<Icons.cancel size={IconSize.menu} />} onClick={cancel}>中止</Button>}
        </Group>
      </Group>
      <Divider my="md" />
      <Switch
        checked={includeSemantic}
        disabled={rebuilding}
        onChange={(event) => setIncludeSemantic(event.currentTarget.checked)}
        label="意味検索のベクトルも同時に作成する"
        description="GPUを使う処理です。全文検索だけの再構築より大幅に時間がかかります。"
      />
    </Card>
    <Card p="lg">
      <Group justify="space-between">
        <Box><Text fw={700}>意味検索モデル</Text><Text size="sm" c="dimmed" mt={5}>{status?.embeddingProvider || "未初期化"}</Text></Box>
        <Badge color={status?.semanticModelReady ? "green" : "gray"} variant="light">{status?.semanticModelReady ? "利用可能" : "準備中"}</Badge>
      </Group>
      <Divider my="md" />
      <Group gap="xl"><Text size="sm">意味ベクトル <b>{formatNumber(status?.semanticIndexedChunks)}</b></Text><Text size="sm">GPU <b>{status?.gpuEnabled ? "有効" : "無効"}</b></Text></Group>
      <Note mt="md">全文検索の索引づくりはCPUの処理で、GPUは使いません。GPU（DirectML）を使うのは上の「意味検索のベクトル」だけです。</Note>
    </Card>
    <Note>再構築中もライブラリの通常検索は利用できます。中止したり、アプリを終了したりした場合は、確定済みのところまでが保持され、次回は続きから再開します。</Note>
  </Stack>;
}

function AppearanceSection({ colorScheme, setColorScheme }: { colorScheme: string; setColorScheme: (value: "light" | "dark" | "auto") => void }) { const [dense, setDense] = useState(isDense); const [pagingMode, setPagingMode] = usePagingMode(); const [pageSize, setPageSize] = usePageSize(); return <Stack gap="lg"><SectionIntro title="外観と操作" description="デスクトップ環境に合わせて表示を調整します。" /><Card p="lg"><Box><Text fw={700}>一覧の読み込み方</Text><Text size="sm" c="dimmed" mb="sm">ライブラリや作者ページで、続きをどう読み込むかを決めます。各一覧の件数表示の横でも切り替えられます。</Text><SegmentedControl aria-label="一覧の読み込み方" value={pagingMode} onChange={(value) => setPagingMode(value as PagingMode)} data={[{ value: "auto", label: "スクロールで自動" }, { value: "pages", label: "ページ番号" }]} /><Text size="xs" c="dimmed" mt="sm">ページ番号は並び順を選んでいるときだけ使えます。関連度順は一致度の高い順に辿る仕組みで「何ページ目」が定まらないため、自動スクロールになります。</Text><Divider my="md" /><Text fw={700}>1ページの件数</Text><Text size="sm" c="dimmed" mb="sm">ページ番号で表示する1ページ分の件数です。スクロールで読み込む1回分の件数にもなります。</Text><SegmentedControl aria-label="1ページの件数" value={String(pageSize)} onChange={(value) => setPageSize(Number(value))} data={PAGE_SIZE_OPTIONS.map((size) => ({ value: String(size), label: `${size}件` }))} /></Box></Card><Card p="lg"><Stack gap="lg"><Box><Text fw={700}>カラーテーマ</Text><Text size="sm" c="dimmed" mb="sm">システム設定に追従することもできます。</Text><SegmentedControl aria-label="カラーテーマ" value={colorScheme} onChange={(value) => setColorScheme(value as "light" | "dark" | "auto")} data={[{ value: "light", label: "ライト" }, { value: "dark", label: "ダーク" }, { value: "auto", label: "システム" }]} /></Box><Divider /><Switch label="高密度表示" description="ページ余白とカードの間隔を狭くします" checked={dense} onChange={(event) => { setDense(event.currentTarget.checked); applyDensity(event.currentTarget.checked); }} /><Group justify="space-between" wrap="nowrap"><Box miw={0}><Text size="sm" fw={500}>視覚効果を減らす</Text><Text size="xs" c="dimmed">OSのモーション設定に自動で従います</Text></Box><Badge variant="light" color="gray">システム設定</Badge></Group></Stack></Card><Card p="lg"><Text fw={700} mb="sm">キーボードショートカット</Text><Stack gap="xs"><Shortcut keys="Ctrl K" label="検索または画面移動" /><Shortcut keys="Ctrl L" label="ライブラリを開く" /><Shortcut keys="Ctrl Shift S" label="保存ワークスペース" /><Shortcut keys="Ctrl S" label="エディタで下書き保存" /></Stack></Card></Stack>; }

function Shortcut({ keys, label }: { keys: string; label: string }) { return <Group justify="space-between"><Text size="sm">{label}</Text><Code>{keys}</Code></Group>; }


function AboutSection({ runtime }: { runtime: boolean }) { return <Stack gap="lg"><SectionIntro title="piepについて" description="クリエイティブ作品を自分の手元で長く楽しむためのデスクトップライブラリ。" /><Card p="xl" className="about-card"><Group><PiepLockup size={34} /><Text c="dimmed">Version {APP_VERSION} · Tauri 2</Text></Group><Divider my="lg" /><Text size="sm" c="dimmed">保存元のデータを尊重しながら、検索、読書、ローカル編集、更新監視、EPUB書き出しを一つのワークスペースにまとめます。</Text></Card><AppUpdateCard runtime={runtime} /><Title order={3}>プロバイダー拡張</Title><Grid>{Object.values(providers).map((provider) => <Grid.Col key={provider.id} span={{ base: 12, sm: 6 }}><Card p="md"><Group><Avatar color="gray" style={{ color: provider.color }}>{provider.icon}</Avatar><Box flex={1}><Group gap="xs"><Text fw={700}>{provider.label}</Text><Badge size="xs" color={provider.capability === "available" ? "green" : "gray"}>{provider.capability === "available" ? "対応" : "計画中"}</Badge></Group><Text size="xs" c="dimmed">{provider.description}</Text></Box></Group></Card></Grid.Col>)}</Grid><Alert color="gray" icon={<Icons.info size={IconSize.nav} />}>piepは各サービスの公式アプリではありません。各サービスの利用規約と作品の権利を尊重して利用してください。</Alert></Stack>; }
