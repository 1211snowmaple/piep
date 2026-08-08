import { useEffect, useState } from "react";
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
  Image,
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
import { Archive, BookOpen, Database, Download, FolderOpen, HardDrive, Info, KeyRound, Palette, RefreshCw, Search, ShieldCheck, Trash2, Upload, UserRound, X } from "lucide-react";
import piepLockup from "@/assets/lockup.svg";
import { PageHeader } from "@/components/PageHeader";
import { useAppSearchParams } from "@/app/router";
import { RuntimeNotice } from "@/components/RuntimeNotice";
import { applyDensity, isDense } from "@/lib/density";
import { errorMessage, formatBytes, formatNumber } from "@/lib/format";
import { getProvider, providers } from "@/lib/providers";
import { exportAllZip, getStoragePath, importZip } from "@/services/archiveApi";
import { loginFanboxWebview, loginPixivWebview, verifyFanboxSession, verifyPixivToken } from "@/services/authApi";
import { getSearchIndexStatus, getStats, isTauriRuntime, scanAndReimportDownloads } from "@/services/dbApi";
import { openSingleDialog, saveDialog } from "@/services/dialogApi";
import { onTauriEvent } from "@/services/eventBus";
import { openFilesystemPath } from "@/services/openerApi";
import { cancelSearchRebuildIndex, startSearchRebuildIndex, type SearchRebuildProgress } from "@/services/searchApi";
import { store } from "@/store";

type Section = "connections" | "library" | "search" | "appearance" | "about";
const SECTIONS: Section[] = ["connections", "library", "search", "appearance", "about"];
function isSection(value: string | null): value is Section {
  return value !== null && (SECTIONS as string[]).includes(value);
}
interface PixivUser { id: string; name: string; profile_image_urls?: { medium?: string } }
interface FanboxUser { userId: string; name: string; iconUrl?: string | null }
interface ConnectionState { pixiv: PixivUser | null; fanbox: FanboxUser | null }

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
  const [rebuild, setRebuild] = useState<SearchRebuildProgress | null>(null);
  const { colorScheme, setColorScheme } = useMantineColorScheme();
  const auth = useQuery({
    queryKey: ["settings-auth"],
    queryFn: async (): Promise<ConnectionState> => {
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
      if (action === "backup") { const path = await saveDialog({ title: "ライブラリのバックアップ", defaultPath: `piep-backup-${new Date().toISOString().slice(0, 10)}.zip`, filters: [{ name: "ZIP archive", extensions: ["zip"] }] }); if (path) await exportAllZip(path); return path ? "バックアップを書き出しました" : ""; }
      if (action === "restore") { const path = await openSingleDialog({ title: "バックアップを復元", filters: [{ name: "ZIP archive", extensions: ["zip"] }] }); if (path) { const count = await importZip(path); return `${count}件を復元しました`; } return ""; }
      const count = await scanAndReimportDownloads(); return `${count}件を再取り込みしました`;
    },
    onSuccess: (message) => { if (message) notifications.show({ color: "green", message }); queryClient.invalidateQueries({ queryKey: ["dashboard"] }); queryClient.invalidateQueries({ queryKey: ["library"] }); queryClient.invalidateQueries({ queryKey: ["stats"] }); },
    onError: (error) => notifications.show({ color: "red", title: "操作に失敗しました", message: errorMessage(error) }),
  });
  const rebuildMutation = useMutation({
    mutationFn: async () => { if (!runtime) throw new Error("デスクトップアプリで利用できます"); const jobId = await startSearchRebuildIndex(64); setRebuild({ jobId, status: "running", totalDownloads: index.data?.totalDownloads ?? 0, indexedDownloads: 0, pendingDownloads: index.data?.totalDownloads ?? 0, isComplete: false, phase: "preparing", indexedChunks: 0 }); return jobId; },
    onError: (error) => notifications.show({ color: "red", title: "再構築を開始できません", message: errorMessage(error) }),
  });
  useEffect(() => {
    let dispose: (() => void) | undefined;
    if (runtime) onTauriEvent<SearchRebuildProgress>("search-rebuild-progress", (event) => { setRebuild(event.payload); if (event.payload.isComplete) { notifications.show({ color: "green", message: "検索インデックスを再構築しました" }); queryClient.invalidateQueries({ queryKey: ["search-index-status"] }); } }).then((fn) => { dispose = fn; }).catch(() => undefined);
    return () => dispose?.();
  }, [queryClient, runtime]);

  const nav = [
    { id: "connections" as const, label: "サービス接続", description: "pixiv・FANBOX", icon: KeyRound },
    { id: "library" as const, label: "ローカルライブラリ", description: "保存先・バックアップ", icon: Database },
    { id: "search" as const, label: "検索", description: "インデックス・意味検索", icon: Search },
    { id: "appearance" as const, label: "外観と操作", description: "テーマ・表示", icon: Palette },
    { id: "about" as const, label: "piepについて", description: "バージョン・拡張", icon: Info },
  ];

  return (
    <div className="page page--contained settings-page">
      <PageHeader title="設定" description="接続情報、ローカルデータ、検索と表示を管理します。" />
      <RuntimeNotice />
      <Grid gap="xl" mt="lg" align="flex-start">
        <Grid.Col span={{ base: 12, md: 4, lg: 3 }}><Card p="xs" className="settings-nav">{nav.map((item) => { const Icon = item.icon; return <NavLink key={item.id} active={section === item.id} label={item.label} description={item.description} leftSection={<Icon size={18} />} onClick={() => setSection(item.id)} />; })}</Card></Grid.Col>
        <Grid.Col span={{ base: 12, md: 8, lg: 9 }}>
          {section === "connections" && <ConnectionsSection auth={auth.data ?? { pixiv: null, fanbox: null }} runtime={runtime} pixivForm={pixivForm} fanboxForm={fanboxForm} mutation={connectionMutation} disconnect={disconnect} />}
          {section === "library" && <LibrarySection stats={stats.data} path={storagePath.data} runtime={runtime} pending={maintenanceMutation.isPending} run={(action) => maintenanceMutation.mutate(action)} />}
          {section === "search" && <SearchSection status={index.data} rebuild={rebuild} runtime={runtime} rebuilding={rebuildMutation.isPending || rebuild?.status === "running"} start={() => rebuildMutation.mutate()} cancel={() => rebuild && cancelSearchRebuildIndex(rebuild.jobId)} />}
          {section === "appearance" && <AppearanceSection colorScheme={colorScheme} setColorScheme={setColorScheme} />}
          {section === "about" && <AboutSection />}
        </Grid.Col>
      </Grid>
    </div>
  );
}

function SectionIntro({ title, description }: { title: string; description: string }) { return <Box mb="lg"><Title order={2}>{title}</Title><Text c="dimmed" size="sm" mt={5}>{description}</Text></Box>; }

type ConnectionInput = { source: "pixiv" | "fanbox"; mode: "web" | "manual"; values?: Record<string, string> };

function ConnectionsSection({ auth, runtime, pixivForm, fanboxForm, mutation, disconnect }: { auth: ConnectionState; runtime: boolean; pixivForm: UseFormReturnType<{ token: string }, { token: string }, any>; fanboxForm: UseFormReturnType<{ session: string; userAgent: string }, { session: string; userAgent: string }, any>; mutation: { mutate: (input: ConnectionInput) => void; isPending: boolean }; disconnect: (source: "pixiv" | "fanbox") => void }) {
  return <Stack gap="lg"><SectionIntro title="サービス接続" description="認証情報は端末内のTauri Storeに保存され、作品取得のときだけ使用します。" /><ConnectionCard source="pixiv" user={auth.pixiv} runtime={runtime} loading={mutation.isPending} onWeb={() => mutation.mutate({ source: "pixiv", mode: "web" })} onDisconnect={() => disconnect("pixiv")} manual={<form onSubmit={pixivForm.onSubmit((values) => mutation.mutate({ source: "pixiv", mode: "manual", values }))}><Group align="flex-start"><PasswordInput flex={1} label="リフレッシュトークン" placeholder="手動で入力" {...pixivForm.getInputProps("token")} /><Button type="submit" mt={25} variant="light">検証して保存</Button></Group></form>} /><ConnectionCard source="fanbox" user={auth.fanbox} runtime={runtime} loading={mutation.isPending} onWeb={() => mutation.mutate({ source: "fanbox", mode: "web" })} onDisconnect={() => disconnect("fanbox")} manual={<form onSubmit={fanboxForm.onSubmit((values) => mutation.mutate({ source: "fanbox", mode: "manual", values }))}><Stack><PasswordInput label="FANBOXSESSID" placeholder="手動で入力" {...fanboxForm.getInputProps("session")} /><TextInput label="User-Agent" {...fanboxForm.getInputProps("userAgent")} /><Button type="submit" variant="light">検証して保存</Button></Stack></form>} /><Alert color="green" icon={<ShieldCheck size={18} />} title="ローカル保存">piepは接続情報を外部サーバーへ送信しません。各サービスへの通信とローカル保存だけに使用します。</Alert></Stack>;
}

function ConnectionCard({ source, user, runtime, loading, onWeb, onDisconnect, manual }: { source: "pixiv" | "fanbox"; user: PixivUser | FanboxUser | null; runtime: boolean; loading: boolean; onWeb: () => void; onDisconnect: () => void; manual: React.ReactNode }) {
  const provider = getProvider(source); const icon = user ? (source === "pixiv" ? (user as PixivUser).profile_image_urls?.medium : (user as FanboxUser).iconUrl ?? undefined) : undefined; return <Card p="lg"><Group justify="space-between" align="flex-start"><Group><Avatar src={icon} size={54} color="piep">{user ? <UserRound size={22} /> : provider.icon}</Avatar><Box><Group gap="xs"><Text fw={700}>{provider.label}</Text><Badge color={user ? "green" : "gray"} variant="light">{user ? "接続済み" : "未接続"}</Badge></Group><Text size="sm" c="dimmed" mt={4}>{user?.name || provider.description}</Text></Box></Group>{user ? <Button color="red" variant="subtle" size="xs" leftSection={<Trash2 size={14} />} onClick={onDisconnect}>接続解除</Button> : <Button disabled={!runtime} loading={loading} leftSection={<KeyRound size={15} />} onClick={onWeb}>ブラウザで接続</Button>}</Group>{!user && <><Divider my="lg" label="または手動入力" labelPosition="center" />{manual}</>}</Card>;
}

function LibrarySection({ stats, path, runtime, pending, run }: { stats?: { totalDownloads: number; totalAssets: number; totalSizeBytes: number }; path?: string; runtime: boolean; pending: boolean; run: (action: "backup" | "restore" | "scan") => void }) { return <Stack gap="lg"><SectionIntro title="ローカルライブラリ" description="保存ファイルとデータベースの場所、バックアップを管理します。" /><Grid><Grid.Col span={{ base: 12, sm: 4 }}><Metric icon={BookOpen} label="作品" value={formatNumber(stats?.totalDownloads)} /></Grid.Col><Grid.Col span={{ base: 12, sm: 4 }}><Metric icon={Archive} label="アセット" value={formatNumber(stats?.totalAssets)} /></Grid.Col><Grid.Col span={{ base: 12, sm: 4 }}><Metric icon={HardDrive} label="使用容量" value={formatBytes(stats?.totalSizeBytes ?? 0)} /></Grid.Col></Grid><Card p="lg"><Text fw={700}>保存先</Text><Group mt="sm" wrap="nowrap"><Code block flex={1}>{path || "読み込み中…"}</Code><ActionIcon variant="light" aria-label="保存先を開く" disabled={!runtime || !path} onClick={() => path && openFilesystemPath(path)}><FolderOpen size={17} /></ActionIcon></Group></Card><Card p="lg"><Stack gap="md"><Box><Text fw={700}>バックアップと復元</Text><Text size="sm" c="dimmed">メタデータ、履歴、アセットをZIPにまとめます。</Text></Box><Group><Button disabled={!runtime} loading={pending} variant="light" leftSection={<Download size={15} />} onClick={() => run("backup")}>バックアップを書き出す</Button><Button disabled={!runtime} loading={pending} variant="default" leftSection={<Upload size={15} />} onClick={() => run("restore")}>バックアップを復元</Button></Group><Divider /><Box><Text fw={700} size="sm">フォルダーから再取り込み</Text><Text size="xs" c="dimmed" mt={4}>DBにない保存フォルダーを検出し、ライブラリへ戻します。</Text><Button mt="sm" disabled={!runtime} loading={pending} size="xs" variant="default" leftSection={<RefreshCw size={14} />} onClick={() => run("scan")}>スキャンを実行</Button></Box></Stack></Card></Stack>; }

function Metric({ icon: Icon, label, value }: { icon: typeof BookOpen; label: string; value: string }) { return <Card p="md"><Group><ThemeIcon variant="light"><Icon size={17} /></ThemeIcon><Box><Text size="xs" c="dimmed">{label}</Text><Text fw={750}>{value}</Text></Box></Group></Card>; }

function SearchSection({ status, rebuild, runtime, rebuilding, start, cancel }: { status?: { totalDownloads: number; indexedDownloads: number; pendingDownloads: number; isComplete: boolean; indexedChunks: number; semanticIndexedChunks: number; semanticModelReady: boolean; embeddingProvider: string; gpuEnabled: boolean }; rebuild: SearchRebuildProgress | null; runtime: boolean; rebuilding: boolean; start: () => void; cancel: () => void }) { const progress = rebuild ? (rebuild.totalDownloads ? rebuild.indexedDownloads / rebuild.totalDownloads * 100 : 0) : status?.totalDownloads ? status.indexedDownloads / status.totalDownloads * 100 : 100; return <Stack gap="lg"><SectionIntro title="検索" description="全文検索と意味検索のインデックス状態を管理します。" /><Card p="lg"><Group justify="space-between"><Box><Group gap="xs"><Text fw={700}>検索インデックス</Text><Badge color={status?.isComplete ? "green" : "yellow"}>{status?.isComplete ? "最新" : "更新中"}</Badge></Group><Text size="sm" c="dimmed" mt={5}>{formatNumber(status?.indexedDownloads)} / {formatNumber(status?.totalDownloads)}作品 · {formatNumber(status?.indexedChunks)}チャンク</Text></Box><ThemeIcon size={48} variant="light"><Search size={22} /></ThemeIcon></Group><Progress value={progress} animated={rebuilding} mt="lg" /><Group mt="lg"><Button disabled={!runtime || rebuilding} leftSection={<RefreshCw size={15} />} onClick={start}>インデックスを再構築</Button>{rebuilding && <Button variant="subtle" color="red" leftSection={<X size={15} />} onClick={cancel}>中止</Button>}</Group></Card><Card p="lg"><Group justify="space-between"><Box><Text fw={700}>意味検索モデル</Text><Text size="sm" c="dimmed" mt={5}>{status?.embeddingProvider || "未初期化"}</Text></Box><Badge color={status?.semanticModelReady ? "green" : "gray"} variant="light">{status?.semanticModelReady ? "利用可能" : "準備中"}</Badge></Group><Divider my="md" /><Group gap="xl"><Text size="sm">意味ベクトル <b>{formatNumber(status?.semanticIndexedChunks)}</b></Text><Text size="sm">GPU <b>{status?.gpuEnabled ? "有効" : "無効"}</b></Text></Group></Card><Alert color="blue">再構築中もライブラリの通常検索は利用できます。アプリを終了した場合は次回起動時に再開できます。</Alert></Stack>; }

function AppearanceSection({ colorScheme, setColorScheme }: { colorScheme: string; setColorScheme: (value: "light" | "dark" | "auto") => void }) { const [dense, setDense] = useState(isDense); return <Stack gap="lg"><SectionIntro title="外観と操作" description="デスクトップ環境に合わせて表示を調整します。" /><Card p="lg"><Stack gap="lg"><Box><Text fw={700}>カラーテーマ</Text><Text size="sm" c="dimmed" mb="sm">システム設定に追従することもできます。</Text><SegmentedControl value={colorScheme} onChange={(value) => setColorScheme(value as "light" | "dark" | "auto")} data={[{ value: "light", label: "ライト" }, { value: "dark", label: "ダーク" }, { value: "auto", label: "システム" }]} /></Box><Divider /><Switch label="高密度表示" description="ページ余白とカードの間隔を狭くします" checked={dense} onChange={(event) => { setDense(event.currentTarget.checked); applyDensity(event.currentTarget.checked); }} /><Switch label="視覚効果を減らす" description="OSのモーション設定が優先されます" disabled /></Stack></Card><Card p="lg"><Text fw={700} mb="sm">キーボードショートカット</Text><Stack gap="xs"><Shortcut keys="Ctrl K" label="検索または画面移動" /><Shortcut keys="Ctrl L" label="ライブラリを開く" /><Shortcut keys="Ctrl Shift S" label="保存ワークスペース" /><Shortcut keys="Ctrl S" label="エディタで下書き保存" /></Stack></Card></Stack>; }

function Shortcut({ keys, label }: { keys: string; label: string }) { return <Group justify="space-between"><Text size="sm">{label}</Text><Code>{keys}</Code></Group>; }


function AboutSection() { return <Stack gap="lg"><SectionIntro title="piepについて" description="クリエイティブ作品を自分の手元で長く楽しむためのデスクトップライブラリ。" /><Card p="xl" className="about-card"><Group><Image src={piepLockup} alt="piep" className="about-card__lockup" /><Text c="dimmed">Version 0.2.0 · Tauri 2</Text></Group><Divider my="lg" /><Text size="sm" c="dimmed">保存元のデータを尊重しながら、検索、読書、ローカル編集、更新監視、EPUB書き出しを一つのワークスペースにまとめます。</Text></Card><Title order={3}>プロバイダー拡張</Title><Grid>{Object.values(providers).map((provider) => <Grid.Col key={provider.id} span={{ base: 12, sm: 6 }}><Card p="md"><Group><Avatar color="gray" style={{ color: provider.color }}>{provider.icon}</Avatar><Box flex={1}><Group gap="xs"><Text fw={700}>{provider.label}</Text><Badge size="xs" color={provider.capability === "available" ? "green" : "gray"}>{provider.capability === "available" ? "対応" : "計画中"}</Badge></Group><Text size="xs" c="dimmed">{provider.description}</Text></Box></Group></Card></Grid.Col>)}</Grid><Alert color="gray" icon={<Info size={18} />}>piepは各サービスの公式アプリではありません。各サービスの利用規約と作品の権利を尊重して利用してください。</Alert></Stack>; }
