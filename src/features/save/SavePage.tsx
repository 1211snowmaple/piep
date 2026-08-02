import { useCallback, useEffect, useRef, useState } from "react";
import {
  ActionIcon,
  Alert,
  Badge,
  Box,
  Button,
  Card,
  Checkbox,
  Divider,
  Group,
  Loader,
  Progress,
  ScrollArea,
  SegmentedControl,
  Stack,
  Text,
  TextInput,
  ThemeIcon,
  Title,
  Tooltip,
} from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { useLocalStorage } from "@mantine/hooks";
import { useQueryClient } from "@tanstack/react-query";
import {
  ArrowLeft,
  ArrowRight,
  Check,
  CircleAlert,
  Download,
  ExternalLink,
  Globe2,
  GripVertical,
  Home,
  LibraryBig,
  Link2,
  ListChecks,
  LockKeyhole,
  PanelRightClose,
  PanelRightOpen,
  ClipboardPaste,
  RotateCw,
  Search,
  X,
} from "lucide-react";
import { useAppNavigate, useAppSearchParams, useRouteParams } from "@/app/router";
import {
  detectDownloadTarget,
  getFanboxCreatorId,
  type FanboxPost,
  type PixivNovel,
  type SidebarDownloadType,
  type SidebarItem,
} from "@/features/browser/downloadCandidates";
import { normalizeFanboxPostPayload, normalizeFanboxSaveMetadata, normalizePixivSaveMetadata } from "@/features/browser/downloadMetadata";
import { refreshEntityProfilesForEntries, type UpdateDownloadEntry } from "@/features/updates/updateWorkflow";
import { errorMessage } from "@/lib/format";
import { getProvider, ProviderMark } from "@/lib/providers";
import {
  closeEmbeddedBrowser,
  destroyEmbeddedBrowser,
  getEmbeddedBrowserUrl,
  goBackEmbeddedBrowser,
  goForwardEmbeddedBrowser,
  navigateEmbeddedBrowser,
  openEmbeddedBrowser,
  reloadEmbeddedBrowser,
} from "@/services/browserApi";
import { getDownloadBySource, isTauriRuntime } from "@/services/dbApi";
import {
  downloadAndSave,
  fetchFanboxCreatorPosts,
  fetchFanboxPost,
  fetchPixivNovel,
  fetchPixivNovelByUrl,
  fetchPixivSeriesNovels,
  fetchPixivUserNovels,
} from "@/services/downloadApi";
import { onTauriEvent } from "@/services/eventBus";
import { openExternalUrl } from "@/services/openerApi";
import { store } from "@/store";
import type { DownloadEntry } from "@/types/library";

type SaveSource = "pixiv" | "fanbox";

export default function SavePage() {
  const { source: routeSource } = useRouteParams("/save/:source?");
  const navigate = useAppNavigate();
  const queryClient = useQueryClient();
  const [searchParams] = useAppSearchParams();
  const runtime = isTauriRuntime();
  const source: SaveSource = routeSource === "fanbox" ? "fanbox" : "pixiv";
  const initialUrl = searchParams.get("url") || getProvider(source).homeUrl || "https://www.pixiv.net/";
  const [currentUrl, setCurrentUrl] = useState(initialUrl);
  const [address, setAddress] = useState(initialUrl);
  const [items, setItems] = useState<SidebarItem[]>([]);
  const [downloadType, setDownloadType] = useState<SidebarDownloadType | null>(null);
  const [analyzing, setAnalyzing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [progress, setProgress] = useState<{ current: number; total: number; text: string } | null>(null);
  const [lastAnalysisUrl, setLastAnalysisUrl] = useState<string | null>(null);
  const [authConnected, setAuthConnected] = useState<boolean | null>(null);
  const [candidateWidth, setCandidateWidth] = useLocalStorage({ key: "piep.save-candidate-width", defaultValue: 360 });
  const [candidateCollapsed, setCandidateCollapsed] = useState(false);
  const browserViewportRef = useRef<HTMLDivElement>(null);
  const currentUrlRef = useRef(currentUrl);

  useEffect(() => { currentUrlRef.current = currentUrl; }, [currentUrl]);

  const positionBrowser = useCallback(async (url: string) => {
    const element = browserViewportRef.current;
    if (!runtime || !element) return;
    const rect = element.getBoundingClientRect();
    if (rect.width < 100 || rect.height < 100) return;
    await openEmbeddedBrowser(url, {
      x: Math.round(rect.left), y: Math.round(rect.top), width: Math.round(rect.width), height: Math.round(rect.height),
      userAgent: source === "fanbox" ? await store.get<string>("fanbox_user_agent") || undefined : undefined,
    });
  }, [runtime, source]);

  useEffect(() => {
    let cancelled = false;
    const home = searchParams.get("url") || getProvider(source).homeUrl || "https://www.pixiv.net/";
    setCurrentUrl(home); setAddress(home); setItems([]); setDownloadType(null); setLastAnalysisUrl(null);
    (source === "pixiv" ? store.get<string>("pixiv_refresh_token") : store.get<string>("fanbox_session_id")).then((value) => { if (!cancelled) setAuthConnected(Boolean(value)); });
    if (runtime) positionBrowser(home).catch((error) => notifications.show({ color: "red", title: "内蔵ブラウザを開けません", message: errorMessage(error) }));
    return () => { cancelled = true; };
  }, [positionBrowser, runtime, searchParams, source]);
  useEffect(() => {
    if (!runtime) return;
    const element = browserViewportRef.current;
    if (!element) return;
    const observer = new ResizeObserver(() => positionBrowser(currentUrlRef.current).catch(() => undefined));
    observer.observe(element);
    const onWindowResize = () => positionBrowser(currentUrlRef.current).catch(() => undefined);
    window.addEventListener("resize", onWindowResize);
    let dispose: (() => void) | undefined;
    onTauriEvent<string>("url-changed", (event) => { setCurrentUrl(event.payload); setAddress(event.payload); }).then((fn) => { dispose = fn; }).catch(() => undefined);
    const poll = window.setInterval(() => getEmbeddedBrowserUrl().then((url) => { setCurrentUrl(url); setAddress(url); }).catch(() => undefined), 2500);
    return () => { observer.disconnect(); window.removeEventListener("resize", onWindowResize); window.clearInterval(poll); dispose?.(); closeEmbeddedBrowser().catch(() => undefined); };
  }, [positionBrowser, runtime]);
  useEffect(() => {
    const guard = (event: BeforeUnloadEvent) => { if (saving) { event.preventDefault(); event.returnValue = ""; } };
    window.addEventListener("beforeunload", guard);
    return () => window.removeEventListener("beforeunload", guard);
  }, [saving]);

  const selectedCount = items.filter((item) => item.selected).length;
  const targetKind = detectDownloadTarget(currentUrl);
  const analysisStale = Boolean(lastAnalysisUrl && lastAnalysisUrl !== currentUrl);
  const navigateBrowser = async (url: string) => {
    const normalized = /^https?:\/\//i.test(url) ? url : `https://${url}`;
    setCurrentUrl(normalized); setAddress(normalized);
    if (!runtime) return;
    try { await navigateEmbeddedBrowser(normalized); } catch { await positionBrowser(normalized); }
  };
  const switchSource = async (next: string) => {
    if (next === source) return;
    // The two providers require different sessions/user agents. Reusing the
    // hidden child WebView can also deliver a late navigation event from the
    // previous provider and roll the selected source back.
    if (runtime) await destroyEmbeddedBrowser().catch(() => undefined);
    navigate(`/save/${next}`);
  };
  const openSourceExternally = async () => {
    try {
      if (runtime) await openExternalUrl(currentUrl);
      else window.open(currentUrl, "_blank", "noopener,noreferrer");
      notifications.show({ color: "blue", title: "外部ブラウザで開きました", message: "保存するページのURLをコピーし、アドレス欄の貼り付けボタンから戻せます。" });
    } catch (error) {
      notifications.show({ color: "red", title: "外部ブラウザを開けません", message: errorMessage(error) });
    }
  };
  const pasteUrl = async () => {
    try {
      const value = (await navigator.clipboard.readText()).trim();
      if (!value) return;
      setAddress(value);
      await navigateBrowser(value);
    } catch (error) {
      notifications.show({ color: "red", title: "クリップボードを読めません", message: errorMessage(error) });
    }
  };
  const startCandidateResize = (event: React.PointerEvent<HTMLDivElement>) => {
    event.preventDefault();
    const startX = event.clientX;
    const startWidth = candidateWidth;
    const move = (moveEvent: PointerEvent) => setCandidateWidth(Math.round(Math.max(280, Math.min(560, startWidth + startX - moveEvent.clientX))));
    const stop = () => { window.removeEventListener("pointermove", move); window.removeEventListener("pointerup", stop); document.body.style.removeProperty("user-select"); };
    document.body.style.userSelect = "none";
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", stop, { once: true });
  };
  const analyze = async () => {
    if (!runtime) return notifications.show({ color: "blue", message: "候補取得はデスクトップアプリで利用できます" });
    if (!authConnected) return notifications.show({ color: "yellow", title: `${getProvider(source).label}に接続してください`, message: "設定画面からログインすると候補を取得できます" });
    const target = detectDownloadTarget(currentUrl);
    if (target === "unsupported") return notifications.show({ color: "yellow", title: "対応ページではありません", message: "作品、シリーズ、作者・クリエイターページを開いてください" });
    setAnalyzing(true); setItems([]); setProgress(null);
    try {
      const pixivToken = await store.get<string>("pixiv_refresh_token") || "";
      const fanboxCookie = await store.get<string>("fanbox_session_id") || "";
      const fanboxUserAgent = await store.get<string>("fanbox_user_agent") || "Mozilla/5.0";
      let next: SidebarItem[] = [];
      if (target === "pixiv_single") {
        const data = await fetchPixivNovelByUrl<PixivNovel>(currentUrl, pixivToken);
        const novelId = String(data.id ?? data.detail?.id ?? "");
        next = [{ id: novelId, title: data.title || data.detail?.title || "Pixiv novel", subtitle: data.user?.name || data.detail?.user?.name, selected: true, originalData: data }];
      } else if (target === "pixiv_series") {
        const seriesId = currentUrl.match(/series(?:\/show\.php\?id=|\/)(\d+)/)?.[1] || "";
        next = (await fetchPixivSeriesNovels<PixivNovel[]>(seriesId, pixivToken)).map((novel) => ({ id: String(novel.id), title: novel.title, subtitle: novel.user?.name, selected: true, originalData: novel }));
      } else if (target === "pixiv_user") {
        const userId = currentUrl.match(/users\/(\d+)/)?.[1] || "";
        next = (await fetchPixivUserNovels<PixivNovel[]>(userId, pixivToken)).map((novel) => ({ id: String(novel.id), title: novel.title, subtitle: novel.user?.name, selected: true, originalData: novel }));
      } else if (target === "fanbox_single") {
        const postId = currentUrl.match(/posts\/(\d+)/)?.[1] || "";
        const data = normalizeFanboxPostPayload<FanboxPost>(await fetchFanboxPost<unknown>(postId, fanboxCookie, fanboxUserAgent));
        next = [{ id: postId, title: data.title || "FANBOX投稿", subtitle: data.user?.name, selected: true, originalData: data }];
      } else if (target === "fanbox_creator") {
        const creatorId = getFanboxCreatorId(currentUrl) || "";
        next = (await fetchFanboxCreatorPosts<FanboxPost[]>(creatorId, fanboxCookie, fanboxUserAgent)).map((post) => ({ id: String(post.id), title: post.title, subtitle: post.user?.name, selected: true, originalData: post }));
      }
      setItems(next); setDownloadType(target); setLastAnalysisUrl(currentUrl);
      notifications.show({ color: "green", title: `${next.length}件の候補を取得しました`, message: "保存する項目を確認してください" });
    } catch (error) {
      notifications.show({ color: "red", title: "候補を取得できません", message: errorMessage(error) });
    } finally { setAnalyzing(false); }
  };
  const execute = async () => {
    const selected = items.filter((item) => item.selected);
    if (!selected.length || !downloadType || !runtime) return;
    setSaving(true);
    let saved = 0, skipped = 0, failed = 0;
    let firstError = "";
    const savedEntries: UpdateDownloadEntry[] = [];
    try {
      const pixivToken = await store.get<string>("pixiv_refresh_token") || "";
      const fanboxCookie = await store.get<string>("fanbox_session_id") || "";
      const fanboxUserAgent = await store.get<string>("fanbox_user_agent") || "Mozilla/5.0";
      for (let index = 0; index < selected.length; index += 1) {
        const item = selected[index];
        setProgress({ current: index + 1, total: selected.length, text: item.title });
        setItems((rows) => rows.map((row) => row.id === item.id ? { ...row, status: "downloading" } : row));
        try {
          const itemSource = downloadType.startsWith("pixiv") ? "pixiv" : "fanbox";
          const existing = await getDownloadBySource<DownloadEntry>(itemSource, item.id);
          if (existing && !existing.watchUpdates) { skipped += 1; setItems((rows) => rows.map((row) => row.id === item.id ? { ...row, status: "skipped" } : row)); continue; }
          if (itemSource === "pixiv") {
            const data = await fetchPixivNovel<PixivNovel>(item.id, pixivToken);
            const metadata = normalizePixivSaveMetadata(data, item.title, item.subtitle);
            savedEntries.push(await downloadAndSave<UpdateDownloadEntry>({ data, source: "pixiv", sourceId: item.id, ...metadata, cookie: null, userAgent: null }));
          } else {
            const data = normalizeFanboxPostPayload<FanboxPost>(await fetchFanboxPost<unknown>(item.id, fanboxCookie, fanboxUserAgent));
            const metadata = normalizeFanboxSaveMetadata(data, item.title, item.subtitle);
            savedEntries.push(await downloadAndSave<UpdateDownloadEntry>({ data, source: "fanbox", sourceId: item.id, ...metadata, cookie: fanboxCookie, userAgent: fanboxUserAgent }));
          }
          saved += 1; setItems((rows) => rows.map((row) => row.id === item.id ? { ...row, status: "success" } : row));
        } catch (error) {
          const message = errorMessage(error);
          if (!firstError) firstError = message;
          failed += 1; setItems((rows) => rows.map((row) => row.id === item.id ? { ...row, status: "failed", error: message } : row));
        }
      }
      if (savedEntries.length > 0) {
        setProgress({ current: selected.length, total: selected.length, text: "作者・シリーズ情報を取得しています" });
        await refreshEntityProfilesForEntries(savedEntries, { refreshToken: pixivToken, fanboxCookie, fanboxUserAgent });
      }
      queryClient.invalidateQueries({ queryKey: ["library"] }); queryClient.invalidateQueries({ queryKey: ["dashboard"] }); queryClient.invalidateQueries({ queryKey: ["library-facets"] });
      queryClient.invalidateQueries({ queryKey: ["entity"] }); queryClient.invalidateQueries({ queryKey: ["entity-works"] });
      notifications.show({ color: failed ? "yellow" : "green", title: "保存が完了しました", message: `保存 ${saved} · スキップ ${skipped} · 失敗 ${failed}` });
      if (firstError) notifications.show({ color: "red", title: "保存できなかった項目があります", message: firstError });
    } finally { setSaving(false); setProgress(null); }
  };

  return (
    <div className="save-page">
      <div className="save-page__header">
        <Group justify="space-between" h="100%" px="md" wrap="nowrap">
          <Group gap="md" wrap="nowrap"><Box><Text size="xs" c="dimmed" fw={700}>SAVE WORKSPACE</Text><Title order={1} fz="h2">Webから保存</Title></Box><SegmentedControl value={source} onChange={switchSource} data={[{ value: "pixiv", label: <Group gap={6} wrap="nowrap"><ProviderMark provider="pixiv" compact /><Text size="xs" fw={700}>pixiv</Text></Group> }, { value: "fanbox", label: <Group gap={6} wrap="nowrap"><ProviderMark provider="fanbox" compact /><Text size="xs" fw={700}>FANBOX</Text></Group> }]} /></Group>
          <Group gap="xs"><Badge color={authConnected ? "green" : "gray"} variant="light" leftSection={<span className="status-dot" />}>{authConnected ? "接続済み" : "未接続"}</Badge><Button variant="subtle" size="xs" onClick={() => navigate("/settings")}>接続設定</Button></Group>
        </Group>
      </div>
      <div className="save-layout" style={{ gridTemplateColumns: candidateCollapsed ? "minmax(360px, 1fr) 6px 54px" : `minmax(360px, 1fr) 6px clamp(280px, ${candidateWidth}px, calc(100vw - 430px))` }}>
        <section className="browser-pane">
          <div className="browser-toolbar">
            <Group gap={4} wrap="nowrap">
              <Tooltip label="戻る"><ActionIcon variant="subtle" color="gray" aria-label="ブラウザで戻る" disabled={!runtime} onClick={() => goBackEmbeddedBrowser()}><ArrowLeft size={17} /></ActionIcon></Tooltip>
              <Tooltip label="進む"><ActionIcon variant="subtle" color="gray" aria-label="ブラウザで進む" disabled={!runtime} onClick={() => goForwardEmbeddedBrowser()}><ArrowRight size={17} /></ActionIcon></Tooltip>
              <Tooltip label="再読み込み"><ActionIcon variant="subtle" color="gray" aria-label="ページを再読み込み" disabled={!runtime} onClick={() => reloadEmbeddedBrowser()}><RotateCw size={16} /></ActionIcon></Tooltip>
              <Tooltip label="ホーム"><ActionIcon variant="subtle" color="gray" aria-label="サービスのホームを開く" onClick={() => navigateBrowser(getProvider(source).homeUrl!)}><Home size={16} /></ActionIcon></Tooltip>
              <form onSubmit={(event) => { event.preventDefault(); navigateBrowser(address); }} style={{ flex: 1 }}>
                <TextInput value={address} onChange={(event) => setAddress(event.currentTarget.value)} leftSection={<LockKeyhole size={13} />} rightSection={<Group gap={0} wrap="nowrap"><Tooltip label="クリップボードのURLを開く"><ActionIcon type="button" variant="subtle" size="sm" aria-label="クリップボードのURLを開く" onClick={pasteUrl}><ClipboardPaste size={14} /></ActionIcon></Tooltip><ActionIcon type="submit" variant="subtle" size="sm" aria-label="URLを開く"><ArrowRight size={14} /></ActionIcon></Group>} rightSectionWidth={58} size="sm" aria-label="ブラウザのアドレス" />
              </form>
              <Tooltip label="外部ブラウザで開く"><ActionIcon variant="subtle" color="gray" aria-label="外部ブラウザで開く" onClick={openSourceExternally}><ExternalLink size={16} /></ActionIcon></Tooltip>
            </Group>
          </div>
          <div ref={browserViewportRef} className="browser-viewport">
            {!runtime && <Stack align="center" justify="center" h="100%" p="xl" ta="center"><ThemeIcon size={72} radius="xl" variant="light"><Globe2 size={34} /></ThemeIcon><Title order={3}>内蔵ブラウザ</Title><Text size="sm" c="dimmed" maw={430}>Tauriアプリでは、この領域に{getProvider(source).label}が表示されます。ページを移動して右側の「候補を取得」を押します。</Text><Button component="a" href={getProvider(source).homeUrl!} target="_blank" variant="light" rightSection={<ExternalLink size={14} />}>公式サイトを開く</Button></Stack>}
          </div>
        </section>

        <div className="save-resizer" role="separator" aria-orientation="vertical" aria-label="保存候補パネルの幅を変更" onPointerDown={startCandidateResize}><GripVertical size={15} /></div>

        <aside className="candidate-pane" data-collapsed={candidateCollapsed || undefined}>
          {candidateCollapsed ? <Tooltip label="保存候補を開く" position="left"><ActionIcon variant="subtle" color="gray" size="lg" mt="sm" mx="auto" aria-label="保存候補を開く" onClick={() => setCandidateCollapsed(false)}><PanelRightOpen size={18} /></ActionIcon></Tooltip> :
          <Stack h="100%" gap={0}>
            <Box p="md">
              <Group justify="space-between" mb="xs"><Box><Text fw={750}>保存候補</Text><Text size="xs" c="dimmed">開いているページから自動判定</Text></Box><Group gap={4}><ThemeIcon variant="light"><ListChecks size={17} /></ThemeIcon><Tooltip label="保存候補をたたむ"><ActionIcon variant="subtle" color="gray" aria-label="保存候補をたたむ" onClick={() => setCandidateCollapsed(true)}><PanelRightClose size={17} /></ActionIcon></Tooltip></Group></Group>
              <Alert color={targetKind === "unsupported" ? "gray" : "blue"} variant="light" icon={targetKind === "unsupported" ? <Link2 size={16} /> : <Check size={16} />} py="xs">
                <Text size="xs">{targetLabel(targetKind)}</Text>
              </Alert>
              <Button fullWidth mt="sm" leftSection={analyzing ? undefined : <Search size={15} />} loading={analyzing} disabled={!runtime || saving || targetKind === "unsupported"} onClick={analyze}>候補を取得</Button>
            </Box>
            <Divider />
            {analysisStale && <Alert color="yellow" radius={0} icon={<CircleAlert size={16} />}>ページが変わりました。候補を再取得してください。</Alert>}
            <Group justify="space-between" px="md" py="sm"><Text size="xs" c="dimmed">{items.length ? `${selectedCount} / ${items.length}件を選択` : "候補はまだありません"}</Text>{items.length > 0 && <Group gap={6}><Button variant="subtle" size="compact-xs" onClick={() => setItems((rows) => rows.map((item) => ({ ...item, selected: true })))}>すべて</Button><Button variant="subtle" color="gray" size="compact-xs" onClick={() => setItems((rows) => rows.map((item) => ({ ...item, selected: false })))}>解除</Button></Group>}</Group>
            <ScrollArea flex={1} px="md" type="auto" className="candidate-list">
              <Stack gap="xs" pb="md">
                {!items.length && !analyzing && <EmptyCandidates source={source} />}
                {items.map((item) => <CandidateRow key={item.id} item={item} disabled={saving} onToggle={() => setItems((rows) => rows.map((row) => row.id === item.id ? { ...row, selected: !row.selected } : row))} />)}
              </Stack>
            </ScrollArea>
            <Divider />
            <Box p="md">
              {progress && <Stack gap={5} mb="sm"><Group justify="space-between"><Text size="xs" className="line-clamp-1">{progress.text}</Text><Text size="xs" c="dimmed">{progress.current}/{progress.total}</Text></Group><Progress value={progress.current / progress.total * 100} animated /></Stack>}
              <Button fullWidth size="md" leftSection={<Download size={16} />} loading={saving} disabled={!runtime || !selectedCount || analysisStale} onClick={execute}>{selectedCount ? `${selectedCount}件をライブラリに保存` : "保存する項目を選択"}</Button>
              <Button fullWidth mt="xs" variant="subtle" color="gray" leftSection={<LibraryBig size={14} />} onClick={() => navigate("/library")}>ライブラリを開く</Button>
            </Box>
          </Stack>}
        </aside>
      </div>
    </div>
  );
}

function targetLabel(kind: ReturnType<typeof detectDownloadTarget>): string {
  const labels: Record<string, string> = { pixiv_single: "pixiv小説を保存できます", pixiv_series: "シリーズ内の小説を選択できます", pixiv_user: "作者の小説を選択できます", fanbox_single: "FANBOX投稿を保存できます", fanbox_creator: "クリエイターの投稿を選択できます", unsupported: "作品・シリーズ・作者ページを開いてください" };
  return labels[kind];
}

function CandidateRow({ item, disabled, onToggle }: { item: SidebarItem; disabled: boolean; onToggle: () => void }) {
  const status = item.status;
  return (
    <Card p="sm" className="candidate-row" data-selected={item.selected || undefined}>
      <Group wrap="nowrap" align="flex-start">
        <Checkbox checked={item.selected} disabled={disabled || status === "success" || status === "skipped"} onChange={onToggle} aria-label={`${item.title}を保存対象にする`} mt={3} />
        <Stack gap={3} flex={1} miw={0}><Text size="sm" fw={650} className="line-clamp-2">{item.title}</Text>{item.subtitle && <Text size="xs" c="dimmed" className="line-clamp-1">{item.subtitle}</Text>}{item.error && <Text size="xs" c="red" className="line-clamp-3">{item.error}</Text>}<Text size="10px" c="dimmed">ID {item.id}</Text></Stack>
        <StatusIcon status={status} />
      </Group>
    </Card>
  );
}

function StatusIcon({ status }: { status: SidebarItem["status"] }) {
  if (status === "downloading") return <Loader size="xs" />;
  if (status === "success") return <ThemeIcon color="green" size="sm" radius="xl"><Check size={12} /></ThemeIcon>;
  if (status === "skipped") return <Badge color="gray" size="xs">保存済み</Badge>;
  if (status === "failed") return <ThemeIcon color="red" size="sm" radius="xl"><X size={12} /></ThemeIcon>;
  return null;
}

function EmptyCandidates({ source }: { source: SaveSource }) {
  return <Stack align="center" ta="center" py={48} px="md"><ThemeIcon size={48} radius="xl" variant="light" color="gray"><Link2 size={22} /></ThemeIcon><Text size="sm" fw={700}>ページを開いて候補を取得</Text><Text size="xs" c="dimmed">{source === "pixiv" ? "小説、シリーズ、作者ページ" : "投稿またはクリエイターページ"}に対応しています。</Text></Stack>;
}
