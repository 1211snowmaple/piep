import { useEffect, useRef, useState } from "react";
import { ActionRow } from "@/components/ActionRow";
import { transitionContent } from "@/lib/contentTransition";
import {
  ActionIcon,
  Accordion,
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
import { useAppNavigate, useAppSearchParams } from "@/app/router";
import { RuntimeNotice } from "@/components/RuntimeNotice";
import { applyDensity, isDense } from "@/lib/density";
import { PAGE_SIZE_OPTIONS, PAGING_SCOPES, useDefaultPagingMode, usePageSize, useScopedPagingPreference, type PagingMode, type PagingPreference, type PagingScope } from "@/components/ListPager";
import { VIEW_SCOPES, useDefaultViewMode, useScopedViewPreference, type ViewMode, type ViewPreference, type ViewScope } from "@/lib/viewMode";
import { errorMessage, formatBytes, formatNumber } from "@/lib/format";
import { getProvider, providers } from "@/lib/providers";
import { connectFanboxSession, connectPixivToken, disconnectAccount, getConnectionStatus, loginFanboxWebview, loginPixivWebview } from "@/services/authApi";
import type { ConnectionStatus } from "@/services/authApi";
import { getSearchIndexStatus, isTauriRuntime } from "@/services/dbApi";
import { shelfDashboard } from "@/services/shelfHealthApi";
import { shelfReimportFolders } from "@/services/shelfBackupApi";
import { useShelfStatus } from "@/features/library/shelf";
import { ShelfBackupCard } from "./ShelfBackupCard";
import type { SearchIndexStatus } from "@/types/library";
import { openFilesystemPath } from "@/services/openerApi";
import { trackManualRebuild, useSearchIndexProgress } from "@/features/search/searchIndexProgress";
import { invalidateWorkSetViews } from "@/features/library/workSetInvalidation";
import DiagnosticsPage from "@/features/diagnostics/DiagnosticsPage";
import { cancelSearchRebuildIndex, startSearchRebuildIndex, type SearchRebuildProgress } from "@/services/searchApi";
import { reportJobAction, requestOperationCancel, startOperation, useOperationJobs, type OperationController } from "@/features/jobs/operationJobs";
import { APP_VERSION } from "@/lib/version";
import { AppUpdateCard } from "@/features/settings/AppUpdateCard";
import { AssistSection } from "@/features/settings/AssistSection";
import { ServerSection } from "@/features/settings/ServerSection";
import { LibrarySidebarSettings } from "./LibrarySidebarSettings";
import { SectionIntro } from "./SectionIntro";
import { SettingsNavigation } from "./SettingsNavigation";
import { searchModes, useDefaultSearchMode } from "@/lib/searchPreferences";
import { DANGER_CONFIRM, DangerButton } from "@/components/DangerButton";

/**
 * 設定の区画。
 *
 * 型と一覧を別々に書くと、片方だけ足したときに**押しても何も起きない項目**が
 * できる（実際に「コレクションの名前」がそうなった）。区画は URL に載るので、
 * 一覧に無い値は弾かれて既定へ戻る — 押した本人には理由が見えない。
 * 一覧から型を導いて、ずれようがなくする。
 */
const SECTIONS = [
  "connections",
  "library",
  "server",
  "search",
  "assist",
  "diagnostics",
  "appearance",
  "about",
] as const;
type Section = (typeof SECTIONS)[number];
function isSection(value: string | null): value is Section {
  return value !== null && (SECTIONS as readonly string[]).includes(value);
}
type ConnectionState = ConnectionStatus;

/** ブラウザプレビュー用の索引の様子。追いついた棚を見せる。 */
const PREVIEW_INDEX_STATUS: SearchIndexStatus = {
  totalDownloads: 1284,
  indexedDownloads: 1284,
  pendingDownloads: 0,
  isComplete: true,
  phase: "ready",
  semanticIndexedChunks: 4192,
  semanticIndexedDownloads: 1284,
  semanticPendingDownloads: 0,
  semanticEnabled: true,
  semanticModelReady: true,
  embeddingProvider: "DirectML (preview)",
  gpuEnabled: true,
  throughputPerSec: null,
};

/**
 * 手で始めた作り直しが終わったときの始末。**画面の外に置く。**
 *
 * 設定画面を離れていても、履歴は閉じ、結果は伝える。ここが画面の中にあった
 * ころは、離れた瞬間に結び付けが切れて履歴が途中で固まっていた。
 */
function settleManualRebuild(progress: SearchRebuildProgress) {
  if (progress.status === "failed") {
    const message = progress.error || "検索インデックスの再構築に失敗しました";
    notifications.show({ color: "red", title: "再構築に失敗しました", message });
    return;
  }
  if (progress.status === "canceled") {
    notifications.show({ color: "gray", message: "再構築を中止しました。次回は途中から再開します" });
    return;
  }
  const failed = progress.failed ?? 0;
  const summary = `${formatNumber(progress.processed ?? 0)}件を索引しました${failed ? `（${formatNumber(failed)}件は読み込めずスキップしました）` : ""}`;
  notifications.show({ color: failed ? "yellow" : "green", title: "検索インデックスを再構築しました", message: summary });
}

export default function SettingsPage() {
  const runtime = isTauriRuntime();
  const [searchParams] = useAppSearchParams();
  const navigate = useAppNavigate();
  const requestedSection = searchParams.get("section");
  // Derived from the URL rather than mirrored into state, so "piepについて" in
  // the header switches sections even when Settings is already open, and the
  // history buttons move between sections.
  const section: Section = isSection(requestedSection) ? requestedSection : "connections";
  const setSection = (next: Section) => {
    const params = new URLSearchParams(searchParams);
    if (next === "connections") params.delete("section"); else params.set("section", next);
    if (next === section) return;
    transitionContent(() => navigate(`/settings${params.size ? `?${params}` : ""}`, { scroll: "top" }), {
      content: () => document.querySelector<HTMLElement>(".settings-content"),
    });
  };
  const queryClient = useQueryClient();
  const operations = useOperationJobs();
  const libraryOperationRunning = operations.some((job) =>
    (job.kind === "backup" || job.kind === "maintenance" || job.kind === "restore") &&
    (job.status === "queued" || job.status === "running" || job.status === "canceling"),
  );
  // Read from the shared store rather than only tracking runs this page
  // started: the app catches the index up on its own at launch, and this screen
  // has to show that run too.
  const rebuild = useSearchIndexProgress();
  const rebuildOperationRef = useRef<OperationController | null>(null);
  const { colorScheme, setColorScheme } = useMantineColorScheme();
  const auth = useQuery({
    queryKey: ["settings-auth"],
    queryFn: async (): Promise<ConnectionState> => {
      if (!runtime) return { pixiv: null, fanbox: null };
      return getConnectionStatus();
    },
  });
  // 数と保存先は棚のもの。
  const shelfStatus = useShelfStatus(runtime);
  const stats = useQuery({ queryKey: ["stats"], queryFn: () => runtime ? shelfDashboard().then((summary) => summary.stats) : Promise.resolve({ totalDownloads: 1284, pixivCount: 936, fanboxCount: 348, totalAssets: 8241, totalSizeBytes: 14_680_000_000 }) });
  const shelfPath = shelfStatus.data && "path" in shelfStatus.data ? shelfStatus.data.path : null;
  const storagePath = useQuery({ queryKey: ["storage-path", shelfPath], enabled: !runtime || shelfPath !== null, queryFn: () => Promise.resolve(runtime ? shelfPath! : "C:\\Users\\preview\\AppData\\Roaming\\io.github.u1211snowmaple.piep\\library") });
  const index = useQuery({ queryKey: ["search-index-status"], queryFn: () => runtime ? getSearchIndexStatus() : Promise.resolve(PREVIEW_INDEX_STATUS) });
  const pixivForm = useForm({ initialValues: { token: "" }, validate: { token: isNotEmpty("リフレッシュトークンを入力してください") } });
  const fanboxForm = useForm({ initialValues: { session: "", userAgent: "Mozilla/5.0" }, validate: { session: isNotEmpty("FANBOXSESSIDを入力してください") } });
  const connectionMutation = useMutation({
    mutationFn: async (input: { source: "pixiv" | "fanbox"; mode: "web" | "manual"; values?: Record<string, string> }) => {
      if (!runtime) throw new Error("接続はデスクトップアプリで利用できます");
      // 鍵はバックエンドが資格情報マネージャーに置く。画面へ返るのは接続の様子だけ。
      // ブラウザ経由の接続だけが web のセッションを持ち帰る。手動でトークンを
      // 貼った場合は Cookie が無いので、更新確認は従来の経路になる。
      if (input.source === "pixiv") return input.mode === "web" ? loginPixivWebview() : connectPixivToken(input.values?.token ?? "");
      return input.mode === "web" ? loginFanboxWebview() : connectFanboxSession(input.values?.session ?? "", input.values?.userAgent || "Mozilla/5.0");
    },
    onSuccess: (status, input) => { notifications.show({ color: "green", title: "接続しました", message: getProvider(input.source).label }); queryClient.setQueryData(["settings-auth"], status); queryClient.invalidateQueries({ queryKey: ["auth-status"] }); pixivForm.reset(); fanboxForm.reset(); },
    onError: (error) => notifications.show({ color: "red", title: "接続できません", message: errorMessage(error) }),
  });
  const disconnect = (source: "pixiv" | "fanbox") => modals.openConfirmModal({
    title: `${getProvider(source).label}との接続を解除しますか？`, children: <Text size="sm">保存済みの作品は削除されません。再度保存・更新するには接続が必要です。</Text>, labels: { confirm: "接続を解除", cancel: "キャンセル" }, confirmProps: DANGER_CONFIRM,
    onConfirm: async () => {
      try {
        queryClient.setQueryData(["settings-auth"], await disconnectAccount(source));
        queryClient.invalidateQueries({ queryKey: ["auth-status"] });
      } catch (error) {
        notifications.show({ color: "red", title: "接続を解除できません", message: errorMessage(error) });
      }
    },
  });
  const maintenanceMutation = useMutation({
    mutationFn: async () => {
      if (!runtime) throw new Error("デスクトップアプリで利用できます");
      const operation = startOperation({
        kind: "maintenance",
        label: "保存フォルダーの再取り込み",
        onRetry: () => maintenanceMutation.mutate(),
      });
      try {
        const outcome = await shelfReimportFolders();
        // 飛ばしたものがあることを黙っていると、件数が合わない理由が誰にも
        // 分からない。何件・なぜ、を同じ行に出す。
        const summary = outcome.skipped.length
          ? `${outcome.imported}件を再取り込みし、${outcome.skipped.length}件は読めずに飛ばしました`
          : `${outcome.imported}件を再取り込みしました`;
        outcome.skipped.forEach((reason) => operation.log(reason, "warn"));
        operation.complete(summary);
        return summary;
      } catch (error) {
        operation.fail(error);
        throw error;
      }
    },
    // 再取り込みは作品を丸ごと増やすので、増えたときに古くなる場所すべてに
    // 知らせる（保存フォルダーから数百件戻ることがある）。
    onSuccess: (message) => {
      if (message) notifications.show({ color: "green", message });
      queryClient.invalidateQueries({ queryKey: ["library"] });
      queryClient.invalidateQueries({ queryKey: ["stats"] });
      invalidateWorkSetViews(queryClient);
    },
    onError: (error) => notifications.show({ color: "red", title: "操作に失敗しました", message: errorMessage(error) }),
  });
  /**
   * 作り直しを始める。**意味検索を切る側に倒れているときだけ、先に確認する。**
   *
   * この画面のスイッチは「その回だけの指定」ではなく機能そのものの入切で、
   * 切って作り直すと**作った埋め込みを捨てる**。9千件ぶんが消えて、入れ直すには
   * 全作品の作り直しが要る。作り直しの押しついでに起きてよい規模ではない。
   */
  const startRebuild = (includeSemantic: boolean) => {
    const covered = index.data?.semanticIndexedDownloads ?? 0;
    if (includeSemantic || !index.data?.semanticEnabled) {
      rebuildMutation.mutate(includeSemantic);
      return;
    }
    modals.openConfirmModal({
      title: "意味検索を無効にする",
      children: <Stack gap="sm">
        <Text size="sm">このまま作り直すと、<b>作品単位の意味検索</b>を無効にします。</Text>
        <Text size="sm">{covered > 0 ? `作成済みの ${formatNumber(covered)}件ぶんの意味ベクトルを削除します。` : "作成済みの意味ベクトルを削除します。"}もう一度有効にするときは、全作品ぶんを作り直すことになります。</Text>
        <Text size="sm" c="dimmed">全文検索の索引はこの操作では消えません。</Text>
      </Stack>,
      labels: { confirm: "無効にして作り直す", cancel: "やめる" },
      confirmProps: DANGER_CONFIRM,
      onConfirm: () => rebuildMutation.mutate(includeSemantic),
    });
  };

  const rebuildMutation = useMutation({
    mutationFn: async (includeSemantic: boolean) => {
      if (!runtime) throw new Error("デスクトップアプリで利用できます");
      const pending = index.data?.pendingDownloads ?? index.data?.totalDownloads ?? 0;
      const jobId = await startSearchRebuildIndex({ includeSemantic });
      const operation = startOperation({
        kind: "search",
        label: includeSemantic ? "検索インデックスを再構築（意味検索を含む）" : "検索インデックスを再構築",
        total: pending,
        onCancel: () => cancelSearchRebuildIndex(jobId),
        onRetry: () => rebuildMutation.mutate(includeSemantic),
      });
      rebuildOperationRef.current = operation;
      // 見張りは画面の外へ預ける。ここを離れても履歴が更新され続ける。
      trackManualRebuild({ jobId, operation, onSettled: settleManualRebuild });
      return jobId;
    },
    onError: (error) => notifications.show({ color: "red", title: "再構築を開始できません", message: errorMessage(error) }),
  });
  // Only runs started here get an entry in the operation history and a toast.
  // An automatic catch-up at launch is reported by the header indicator; it
  // needs no announcement and certainly no notification every time.
  useEffect(() => {
    if (!rebuild || rebuild.status === "running") return;
    rebuildOperationRef.current = null;
    queryClient.invalidateQueries({ queryKey: ["search-index-status"] });
    // 診断の計測は `enabled: false` なので、**古いと印を付けても取り直されない。**
    // 印だけ付けて放っておくと、再構築のあとも前の数字が出つづける。かといって
    // 勝手に測り直すのは筋が違う - 実データでの検索速度まで測る重い処理で、
    // 「走ることのほうが頼まれるのを待つ」というのがこの画面の決まりである。
    // 嘘の数字を残さず、測っていない状態へ戻す。
    queryClient.removeQueries({ queryKey: ["library-diagnostics"] });
  }, [queryClient, rebuild]);
  // An automatic run still changes the numbers this page shows.
  const automaticFinished = rebuild && rebuild.origin === "automatic" && rebuild.status === "completed";
  useEffect(() => {
    if (automaticFinished) queryClient.invalidateQueries({ queryKey: ["search-index-status"] });
  }, [automaticFinished, queryClient]);

  const nav = [
    { id: "connections", label: "サービス接続", description: "保存・更新に使うアカウント", icon: Icons.credentials, group: "接続とデータ", keywords: "pixiv FANBOX 認証 ログイン 再接続" },
    { id: "library", label: "保存とバックアップ", description: "保存先・書き出し・復元", icon: Icons.database, group: "接続とデータ", keywords: "ローカルライブラリ フォルダー 再取り込み" },
    { id: "server", label: "サーバー", beta: true, description: "携帯で読む・ライブラリを送る", icon: Icons.server, group: "接続とデータ", keywords: "Android モバイル スマホ 外出先 同期 piep-server" },
    { id: "appearance", label: "表示と操作", description: "テーマ・サイドバー・一覧", icon: Icons.appearance, group: "使いやすくする", keywords: "外観 文字 色 ページ 件数 高密度 ショートカット" },
    { id: "search", label: "検索", description: "探し方・検索データ", icon: Icons.search, group: "使いやすくする", keywords: "インデックス 索引 意味検索 GPU" },
    { id: "assist", label: "AIの手伝い", beta: true, description: "モデル・接続・利用する機能", icon: Icons.optimize, group: "使いやすくする", keywords: "推論 サーバー" },
    { id: "diagnostics", label: "診断とメンテナンス", description: "保存状態・容量・修復", icon: Icons.diagnostics, group: "管理する", keywords: "性能 最適化 エラー 画像 重複" },
    { id: "about", label: "piepについて", description: "バージョン・アプリの更新", icon: Icons.info, group: "管理する", keywords: "アップデート リリース 拡張" },
  ];

  return (
    <div className="page page--contained settings-page">
      <PageHeader title="設定" description="接続情報、ローカルデータ、検索と表示を管理します。" />
      <RuntimeNotice />
      <Grid gap="xl" mt="lg" align="flex-start">
        <Grid.Col span={{ base: 12, md: 4, lg: 3 }} className="settings-nav-column"><div className="settings-nav-scroller"><SettingsNavigation items={nav} active={section} onSelect={(next) => { if (isSection(next)) setSection(next); }} /></div></Grid.Col>
        <Grid.Col span={{ base: 12, md: 8, lg: 9 }} className="settings-content">
          {section === "connections" && (auth.isLoading ? <LoadingState label="接続状態を確認しています" /> : auth.error ? <ErrorState error={auth.error} retry={() => auth.refetch()} /> : <ConnectionsSection auth={auth.data ?? { pixiv: null, fanbox: null }} runtime={runtime} pixivForm={pixivForm} fanboxForm={fanboxForm} mutation={connectionMutation} disconnect={disconnect} />)}
          {section === "library" && (stats.isLoading || storagePath.isLoading ? <LoadingState label="ライブラリ情報を読み込んでいます" /> : stats.error || storagePath.error ? <ErrorState error={stats.error ?? storagePath.error} retry={() => { stats.refetch(); storagePath.refetch(); }} /> : <LibrarySection stats={stats.data} path={storagePath.data} runtime={runtime} pending={maintenanceMutation.isPending} busy={libraryOperationRunning} reimport={() => maintenanceMutation.mutate()} />)}
          {section === "server" && <ServerSection runtime={runtime} />}
          {section === "search" && (index.isLoading ? <LoadingState label="検索インデックスを確認しています" /> : index.error ? <ErrorState error={index.error} retry={() => index.refetch()} /> : <SearchSection status={index.data} rebuild={rebuild} runtime={runtime} rebuilding={rebuildMutation.isPending || rebuild?.status === "running"} start={(includeSemantic) => startRebuild(includeSemantic)} cancel={() => { if (rebuildOperationRef.current) { void requestOperationCancel(rebuildOperationRef.current.id); return; } if (rebuild) reportJobAction(cancelSearchRebuildIndex(rebuild.jobId), "索引の作り直しを中止できません"); }} />)}
          {section === "assist" && <AssistSection />}
          {section === "diagnostics" && <DiagnosticsPage embedded />}
          {section === "appearance" && <AppearanceSection colorScheme={colorScheme} setColorScheme={setColorScheme} />}
          {section === "about" && <AboutSection runtime={runtime} />}
        </Grid.Col>
      </Grid>
    </div>
  );
}

type ConnectionInput = { source: "pixiv" | "fanbox"; mode: "web" | "manual"; values?: Record<string, string> };

function ConnectionsSection({ auth, runtime, pixivForm, fanboxForm, mutation, disconnect }: { auth: ConnectionState; runtime: boolean; pixivForm: UseFormReturnType<{ token: string }, { token: string }, any>; fanboxForm: UseFormReturnType<{ session: string; userAgent: string }, { session: string; userAgent: string }, any>; mutation: { mutate: (input: ConnectionInput) => void; isPending: boolean }; disconnect: (source: "pixiv" | "fanbox") => void }) {
  // web のセッションは「二つ目の接続」ではなく、この接続の内訳である。
  // 揃っていることは既定なので何も言わない。欠けているときだけ、何が
  // できないのかと、どうすれば戻るのかを言う。
  return <Stack gap="lg"><SectionIntro title="取得用の接続" description="作品の保存・更新確認に使うアカウントです。内蔵ブラウザで表示するアカウントとは独立しています。" /><ConnectionCard source="pixiv" user={auth.pixiv} note={auth.pixiv && !auth.pixiv.webSession ? "一覧を使った高速確認には、ブラウザでの再接続が必要です" : null} runtime={runtime} loading={mutation.isPending} onWeb={() => mutation.mutate({ source: "pixiv", mode: "web" })} onDisconnect={() => disconnect("pixiv")} manual={<form onSubmit={pixivForm.onSubmit((values) => mutation.mutate({ source: "pixiv", mode: "manual", values }))}><Group align="flex-start"><PasswordInput flex={1} disabled={!runtime} label="リフレッシュトークン" placeholder="手動で入力" {...pixivForm.getInputProps("token")} /><Button type="submit" mt={25} variant="light" disabled={!runtime}>検証して保存</Button></Group></form>} /><ConnectionCard source="fanbox" user={auth.fanbox} runtime={runtime} loading={mutation.isPending} onWeb={() => mutation.mutate({ source: "fanbox", mode: "web" })} onDisconnect={() => disconnect("fanbox")} manual={<form onSubmit={fanboxForm.onSubmit((values) => mutation.mutate({ source: "fanbox", mode: "manual", values }))}><Stack><PasswordInput disabled={!runtime} label="FANBOXSESSID" placeholder="手動で入力" {...fanboxForm.getInputProps("session")} /><TextInput disabled={!runtime} label="User-Agent" {...fanboxForm.getInputProps("userAgent")} /><ActionRow><Button type="submit" variant="light" disabled={!runtime}>検証して保存</Button></ActionRow></Stack></form>} /><Text size="sm" c="dimmed">内蔵ブラウザのログインは保存画面で行います。ログイン・ログアウトしても取得用の接続は変わりません。</Text><Note icon={Icons.secure} title="ローカル保存">piepは接続情報を外部サーバーへ送信しません。各サービスへの通信とローカル保存だけに使用します。</Note></Stack>;
}

function ConnectionCard({ source, user, note, runtime, loading, onWeb, onDisconnect, manual }: { source: "pixiv" | "fanbox"; user: { name: string } | null; note?: string | null; runtime: boolean; loading: boolean; onWeb: () => void; onDisconnect: () => void; manual: React.ReactNode }) {
  // 保存元の記号を出す。ここは長く `profile_image_urls` を `Avatar` に渡して
  // いたが、pixiv の画像は `Referer` を求めるので `<img>` からは取れず、必ず
  // 人型に落ちていた。**取れない絵を頼んで、取れなかった姿を見せていた。**
  // 作者の顔は棚が持っている（保存時に控えている）。ここで要るのは
  // 「どのサービスに繋がっているか」なので、サービスの記号のほうが答えである。
  const provider = getProvider(source); return <Card p="lg"><Group justify="space-between" align="flex-start"><Group><Avatar size={54} color="gray" variant="light" style={{ color: provider.color }}>{provider.icon}</Avatar><Box><Group gap="xs"><Text fw={700}>{provider.label}</Text><Badge color={user ? "green" : "gray"} variant="light">{user ? "取得用接続済み" : "未接続"}</Badge></Group><Text size="sm" c="dimmed" mt={4}>{user?.name || provider.description}</Text>{note && <Text size="xs" c="dimmed" mt={2}>{note}</Text>}</Box></Group>{user ? <Group gap="xs"><Button variant="light" size="xs" disabled={!runtime} loading={loading} onClick={onWeb}>再接続</Button><DangerButton size="xs" leftSection={<Icons.delete size={IconSize.menu} />} onClick={onDisconnect}>接続解除</DangerButton></Group> : <Button disabled={!runtime} loading={loading} leftSection={<Icons.credentials size={IconSize.menu} />} onClick={onWeb}>ブラウザで接続</Button>}</Group>{!user && <><Divider my="lg" label="または手動入力" labelPosition="center" />{manual}</>}</Card>;
}

function LibrarySection({ stats, path, runtime, pending, busy, reimport }: { stats?: { totalDownloads: number; totalAssets: number; totalSizeBytes: number }; path?: string; runtime: boolean; pending: boolean; busy: boolean; reimport: () => void }) { return <Stack gap="lg"><SectionIntro title="保存とバックアップ" description="保存先の確認、バックアップの作成と復元を行います。" /><Grid><Grid.Col span={{ base: 12, sm: 4 }}><Metric icon={Icons.read} label="作品" value={formatNumber(stats?.totalDownloads)} /></Grid.Col><Grid.Col span={{ base: 12, sm: 4 }}><Metric icon={Icons.archive} label="画像・添付" value={formatNumber(stats?.totalAssets)} /></Grid.Col><Grid.Col span={{ base: 12, sm: 4 }}><Metric icon={Icons.storage} label="使用容量" value={formatBytes(stats?.totalSizeBytes ?? 0)} /></Grid.Col></Grid><Card p="lg"><Text fw={700}>保存先</Text><Group mt="sm" wrap="nowrap"><Code block flex={1}>{path || "読み込み中…"}</Code><ActionIcon variant="light" aria-label="保存先を開く" disabled={!runtime || !path} onClick={() => path && openFilesystemPath(path)}><Icons.openFolder size={IconSize.action} /></ActionIcon></Group></Card><ShelfBackupCard runtime={runtime} busy={busy} /><Card p="lg"><Group justify="space-between" align="flex-start" wrap="nowrap"><Box miw={0}><Text fw={700} size="sm">フォルダーから再取り込み</Text><Text size="xs" c="dimmed" mt={4}>アプリのフォルダーの downloads に置いた保存フォルダーを調べ、棚にまだ無い作品を版ごと取り込みます。棚にある作品や、棚から外した作品には触れません。</Text></Box><Button style={{ flexShrink: 0 }} disabled={!runtime || busy} loading={pending} size="xs" variant="default" leftSection={<Icons.retry size={IconSize.menu} />} onClick={reimport}>保存フォルダーを調べる</Button></Group></Card></Stack>; }

function Metric({ icon: Icon, label, value }: { icon: LucideIcon; label: string; value: string }) { return <Card p="md"><Group><ThemeIcon variant="light"><Icon size={17} /></ThemeIcon><Box><Text size="xs" c="dimmed">{label}</Text><Text fw={750}>{value}</Text></Box></Group></Card>; }

const REBUILD_PHASE_LABEL: Record<string, string> = {
  preparing: "対象を数えています",
  indexing: "本文を解析して索引に書き込んでいます",
  committing: "索引を確定しています",
  ready: "完了しました",
  failed: "失敗しました",
};

function SearchSection({ status, rebuild, runtime, rebuilding, start, cancel }: { status?: SearchIndexStatus; rebuild: SearchRebuildProgress | null; runtime: boolean; rebuilding: boolean; start: (includeSemantic: boolean) => void; cancel: () => void }) {
  const [defaultSearchMode, setDefaultSearchMode] = useDefaultSearchMode();
  const [includeSemantic, setIncludeSemantic] = useState(status?.semanticEnabled ?? false);
  useEffect(() => setIncludeSemantic(status?.semanticEnabled ?? false), [status?.semanticEnabled]);
  const indexedRatio = status?.totalDownloads ? status.indexedDownloads / status.totalDownloads * 100 : 100;
  const running = rebuilding && rebuild && rebuild.status === "running";
  return <Stack gap="lg">
    <SectionIntro title="検索" description="いつもの探し方と、検索に使うデータを管理します。" />
    <Card p="lg"><Text fw={700}>既定の検索方法</Text><Text size="sm" c="dimmed" mt={4} mb="md">ライブラリと全体検索を開いたときに使います。検索欄でも切り替えられます。</Text><SegmentedControl fullWidth aria-label="既定の検索方法" value={defaultSearchMode} onChange={(value) => setDefaultSearchMode(value === "semantic" || value === "exact" ? value : "smart")} data={searchModes.map(({value,label}) => ({value,label}))} /><Stack gap={6} mt="md">{searchModes.map((mode) => <Text key={mode.value} size="sm" c="dimmed"><b>{mode.label}</b>：{mode.description}</Text>)}</Stack>{defaultSearchMode === "semantic" && !status?.semanticEnabled && <Note mt="sm">意味検索を使うには、下の検索データの保守で意味検索を有効にしてください。</Note>}</Card>
    <Accordion variant="separated"><Accordion.Item value="maintenance"><Accordion.Control><Text fw={700}>検索データの保守</Text><Text size="xs" c="dimmed">反映状況・索引の作り直し・意味検索モデル</Text></Accordion.Control><Accordion.Panel><Stack gap="md">
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
      <Divider my="md" />
      <Switch
        checked={includeSemantic}
        disabled={rebuilding}
        onChange={(event) => setIncludeSemantic(event.currentTarget.checked)}
        label="作品単位の意味検索を使う"
        description="作り直しを実行すると反映されます。オフで実行すると、確認後に意味検索のデータを削除します。"
      />
      <ActionRow mt="lg">
        {rebuilding && <DangerButton leftSection={<Icons.cancel size={IconSize.menu} />} onClick={cancel}>中止</DangerButton>}
        {/* status が届くまでは押させない。届く前の `includeSemantic` は既定の
            false なので、押せてしまうと**意味検索を切る**側に倒れる。 */}
        <Button disabled={!runtime || rebuilding || !status} leftSection={<Icons.retry size={IconSize.menu} />} onClick={() => start(includeSemantic)}>
          {status?.isComplete ? "インデックスを再構築" : "未反映分を索引する"}
        </Button>
      </ActionRow>
    </Card>
    <Card p="lg">
      <Group justify="space-between">
        <Box><Text fw={700}>意味検索モデル</Text><Text size="sm" c="dimmed" mt={5}>{status?.semanticModelReady ? status.embeddingProvider : "初回の意味検索で読み込みます"}</Text></Box>
        <Badge color={status?.semanticModelReady ? "green" : "gray"} variant="light">{!status?.semanticEnabled ? "未使用" : status.semanticModelReady ? "利用可能" : "読み込み前"}</Badge>
      </Group>
      <Divider my="md" />
      {/* 断片の数だけでは、棚のどこまで届いているのかが分からない。作品数で言う。 */}
      <Group gap="xl">
        <Text size="sm">検索データを保存済みの作品 <b>{formatNumber(status?.semanticIndexedDownloads)}</b> / {formatNumber(status?.totalDownloads)}</Text>
        <Text size="sm">作品ベクトル <b>{formatNumber(status?.semanticIndexedChunks)}</b></Text>
        <Text size="sm">GPU <b>{!status?.semanticModelReady ? "読み込み前" : status.gpuEnabled ? "有効" : "未使用"}</b></Text>
      </Group>
      {status?.semanticEnabled && Boolean(status?.semanticPendingDownloads) && <Note mt="md">{formatNumber(status?.semanticPendingDownloads)}件は意味検索の対象になっていません。上の「作品単位の意味検索」を有効にして再構築すると追いつきます。</Note>}
      <Note mt="md">全文検索の索引作成はCPUで行います。意味検索は初回利用時にモデルを読み込み、利用可能な環境ではGPU（DirectML）を使います。保存済みの検索データを作り直す必要はありません。</Note>
    </Card>
    <Note>再構築中もライブラリの通常検索は利用できます。中止したり、アプリを終了したりした場合は、確定済みのところまでが保持され、次回は続きから再開します。</Note>
    </Stack></Accordion.Panel></Accordion.Item></Accordion>
  </Stack>;
}

function AppearanceSection({ colorScheme, setColorScheme }: { colorScheme: string; setColorScheme: (value: "light" | "dark" | "auto") => void }) {
  const [dense, setDense] = useState(isDense);
  const [pageSize, setPageSize] = usePageSize();
  return <Stack gap="lg">
    <SectionIntro title="表示と操作" description="見た目、サイドバー、一覧の使い方を調整します。" />
    <Card p="lg"><Stack gap="md"><Text fw={700}>テーマと余白</Text><SegmentedControl aria-label="カラーテーマ" value={colorScheme} onChange={(value) => setColorScheme(value as "light" | "dark" | "auto")} data={[{ value: "light", label: "ライト" }, { value: "dark", label: "ダーク" }, { value: "auto", label: "システム" }]} /><Switch label="高密度表示" description="ページ余白とカードの間隔を狭くします" checked={dense} onChange={(event) => { setDense(event.currentTarget.checked); applyDensity(event.currentTarget.checked); }} /><Text size="xs" c="dimmed">アニメーションはOSの「視覚効果を減らす」設定に従います。</Text></Stack></Card>
    <Card p="lg"><LibrarySidebarSettings /></Card>
    <Card p="lg"><Stack gap="md"><Text fw={700}>一覧</Text><Text size="sm" c="dimmed">一覧の画面でも切り替えられます。画面ごとに変える場合は下の詳細を開いてください。</Text><Accordion variant="separated"><Accordion.Item value="view"><Accordion.Control>表紙・リストの表示</Accordion.Control><Accordion.Panel><ViewSettings /></Accordion.Panel></Accordion.Item><Accordion.Item value="paging"><Accordion.Control>読み込み方と画面ごとの指定</Accordion.Control><Accordion.Panel><PagingSettings /><Text size="xs" c="dimmed" mt="sm">関連度順は続きから順に読み込みます。日付や名前などの並び順ではページ番号も利用できます。</Text></Accordion.Panel></Accordion.Item></Accordion><Box><Text size="sm" fw={650} mb="sm">1回に表示する件数</Text><SegmentedControl aria-label="1ページの件数" value={String(pageSize)} onChange={(value) => setPageSize(Number(value))} data={PAGE_SIZE_OPTIONS.map((size) => ({ value: String(size), label: `${size}件` }))} /></Box></Stack></Card>
    <Card p="lg"><Text fw={700} mb="sm">キーボードショートカット</Text><Stack gap="xs"><Shortcut keys="Ctrl K" label="ライブラリ検索・画面移動" /><Shortcut keys="F5" label="入力や処理を保って画面を更新" /><Shortcut keys="Ctrl L" label="ライブラリを開く" /><Shortcut keys="Ctrl Shift S" label="保存画面を開く" /><Shortcut keys="Ctrl S" label="エディタで下書き保存" /></Stack></Card>
  </Stack>;
}

/**
 * 読み込み方を、まとめてと一覧ごとの二段で決める。
 *
 * 一覧の性質は同じではない。束の中身は番号で飛びたいが棚は流し読みしたい、と
 * いった向き不向きがあるので、全部を一つの好みに縛るとどちらかが必ず不便になる。
 * かといって毎回ここへ来るのも面倒なので、**まとめて変える**を上に置く。
 *
 * まとめて変えると個別の指定も消える。消さないと「まとめて変えたのに変わらない
 * 一覧がある」ことになり、押した本人にはその理由が見えない。
 */
function PagingSettings() {
  const [defaultMode, setDefaultMode] = useDefaultPagingMode();
  return (
    <Stack gap="sm">
      <SegmentedControl
        aria-label="一覧の読み込み方"
        value={defaultMode}
        onChange={(value) => {
          setDefaultMode(value as PagingMode);
          PAGING_SCOPES.forEach(({ value: scope }) => window.localStorage.removeItem(`piep.paging-mode.${scope}`));
        }}
        data={[{ value: "auto", label: "スクロールで自動" }, { value: "pages", label: "ページ番号" }]}
      />
      <Text size="xs" c="dimmed">まとめて変えると、一覧ごとの指定は「全体に合わせる」へ戻ります。</Text>
      <Divider my={4} />
      <Text size="sm" fw={650}>一覧ごとに変える</Text>
      {PAGING_SCOPES.map((scope) => <ScopedPagingRow key={scope.value} scope={scope} />)}
    </Stack>
  );
}

/**
 * 見え方も、読み込み方と同じ二段で決める。
 *
 * 隣に並ぶ二つの設定が、片方は全体だけ・片方は一覧ごと、では説明が付かない。
 * 表紙で選びたいのは棚で、束の中身は順番を追うので行のほうが読みやすい、
 * という向き不向きは読み込み方と同じようにある。
 */
function ViewSettings() {
  const [defaultMode, setDefaultMode] = useDefaultViewMode();
  return (
    <Stack gap="sm">
      <SegmentedControl
        aria-label="一覧の見え方"
        value={defaultMode}
        onChange={(value) => {
          setDefaultMode(value as ViewMode);
          VIEW_SCOPES.forEach(({ value: scope }) => window.localStorage.removeItem(`piep.library-view.${scope}`));
        }}
        data={[{ value: "gallery", label: "表紙で見る" }, { value: "compact", label: "一覧で見る" }]}
      />
      <Text size="xs" c="dimmed">まとめて変えると、一覧ごとの指定は「全体に合わせる」へ戻ります。</Text>
      <Divider my={4} />
      <Text size="sm" fw={650}>一覧ごとに変える</Text>
      {VIEW_SCOPES.map((scope) => <ScopedViewRow key={scope.value} scope={scope} />)}
    </Stack>
  );
}

function ScopedViewRow({ scope }: { scope: { value: ViewScope; label: string } }) {
  const [preference, setPreference] = useScopedViewPreference(scope.value);
  return (
    <Group justify="space-between" wrap="nowrap" gap="sm">
      <Text size="sm" miw={0}>{scope.label}</Text>
      <SegmentedControl
        size="xs"
        aria-label={`${scope.label}の見え方`}
        value={preference}
        onChange={(value) => setPreference(value as ViewPreference)}
        data={[
          { value: "inherit", label: "全体に合わせる" },
          { value: "gallery", label: "表紙" },
          { value: "compact", label: "一覧" },
        ]}
      />
    </Group>
  );
}

function ScopedPagingRow({ scope }: { scope: { value: PagingScope; label: string } }) {
  const [preference, setPreference] = useScopedPagingPreference(scope.value);
  return (
    <Group justify="space-between" wrap="nowrap" gap="sm">
      <Text size="sm" miw={0}>{scope.label}</Text>
      <SegmentedControl
        size="xs"
        aria-label={`${scope.label}の読み込み方`}
        value={preference}
        onChange={(value) => setPreference(value as PagingPreference)}
        data={[
          { value: "inherit", label: "全体に合わせる" },
          { value: "auto", label: "自動" },
          { value: "pages", label: "ページ番号" },
        ]}
      />
    </Group>
  );
}

function Shortcut({ keys, label }: { keys: string; label: string }) { return <Group justify="space-between"><Text size="sm">{label}</Text><Code>{keys}</Code></Group>; }


function AboutSection({ runtime }: { runtime: boolean }) { return <Stack gap="lg"><SectionIntro title="piepについて" description="クリエイティブ作品を自分の手元で長く楽しむためのデスクトップライブラリ。" /><Card p="xl" className="about-card"><Group><PiepLockup size={34} /><Text c="dimmed">Version {APP_VERSION} · Tauri 2</Text></Group><Divider my="lg" /><Text size="sm" c="dimmed">保存元のデータを尊重しながら、検索、読書、ローカル編集、更新監視、EPUB書き出しを一つのワークスペースにまとめます。</Text></Card><AppUpdateCard runtime={runtime} /><Title order={3}>プロバイダー拡張</Title><Grid>{Object.values(providers).map((provider) => <Grid.Col key={provider.id} span={{ base: 12, sm: 6 }}><Card p="md"><Group><Avatar color="gray" style={{ color: provider.color }}>{provider.icon}</Avatar><Box flex={1}><Group gap="xs"><Text fw={700}>{provider.label}</Text><Badge size="xs" color={provider.capability === "available" ? "green" : "gray"}>{provider.capability === "available" ? "対応" : "計画中"}</Badge></Group><Text size="xs" c="dimmed">{provider.description}</Text></Box></Group></Card></Grid.Col>)}</Grid><Alert color="gray" icon={<Icons.info size={IconSize.nav} />}>piepは各サービスの公式アプリではありません。各サービスの利用規約と作品の権利を尊重して利用してください。</Alert></Stack>; }
