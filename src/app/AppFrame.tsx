import { useEffect, useLayoutEffect, useMemo, useRef, type ReactNode } from "react";
import {
  ActionIcon,
  AppShell,
  Box,
  Burger,
  Divider,
  Group,
  Image,
  Indicator,
  Loader,
  Menu,
  Stack,
  Text,
  Tooltip,
  UnstyledButton,
  useComputedColorScheme,
  useMantineColorScheme,
} from "@mantine/core";
import { useDisclosure, useHotkeys, useLocalStorage, useMediaQuery } from "@mantine/hooks";
import { Spotlight, spotlight, type SpotlightActionData } from "@mantine/spotlight";
import { Icons, IconSize } from "@/lib/icons";
import piepIcon from "@/assets/icon.svg";
import { PiepLockup } from "@/components/PiepLockup";
import { useAppNavigate, useAppRouter, type NavigationType } from "@/app/router";
import { useWorkspace } from "@/app/WorkspaceContext";
import { WorkspaceNav, WorkspaceNavFooter } from "@/app/WorkspaceNav";
import { isTauriRuntime } from "@/services/dbApi";
import { isRebuildRunning, rebuildPercent, useSearchIndexProgress } from "@/features/search/searchIndexProgress";
import { useAppUpdateNotice } from "@/app/useAppUpdateNotice";
import { useUpdateScheduler } from "@/features/updates/useUpdateScheduler";
import { APP_VERSION } from "@/lib/version";

const RAIL_WIDTH = 62;
const NAVBAR_WIDTH = 194;

function PiepBrand({ collapsed = false, onClick }: { collapsed?: boolean; onClick?: () => void }) {
  return (
    <UnstyledButton className="app-brand" onClick={onClick} aria-label="ホームへ移動">
      {collapsed
        ? <Image src={piepIcon} alt="" className="app-brand__mark" />
        : <PiepLockup size={24} />}
    </UnstyledButton>
  );
}

/**
 * How long to keep trying after the screen last changed height.
 *
 * A fixed budget is the wrong shape: half a second was not enough for a library
 * that fetches its rows over IPC, and any figure large enough for a slow machine
 * spends most of itself waiting on a screen that finished instantly. So the
 * clock restarts every time the content grows, and the cap below is only there
 * to stop something that never settles from holding the position forever.
 */
const RESTORE_QUIET_MS = 700;
const RESTORE_MAX_MS = 6000;

/** How long the offset has to survive the screen filling in to count as settled. */
const RESTORE_STABLE_FRAMES = 5;

/** Anything the reader does to the scroll position themselves. */
const USER_SCROLL_EVENTS = ["wheel", "touchstart", "keydown", "pointerdown"] as const;

/**
 * Puts each screen back where it was left.
 *
 * A new destination opens at the top, but going back has to return you to the
 * row you came from - being dropped at the top of a two thousand item library
 * every time you close a work makes the back button useless. Rewriting the
 * query string, which is what a tab or a filter does, must not move the page at
 * all.
 *
 * Positions belong to history entries rather than to addresses. Two visits to
 * the library are two places the reader was standing, and keying by path meant
 * whichever they left last decided where both of them resumed.
 */
function useScrollRestoration(
  ref: React.RefObject<HTMLElement | null>,
  historyIndex: number,
  pathname: string,
  navigationType: NavigationType,
) {
  const positions = useRef(new Map<number, number>());
  const currentIndex = useRef(historyIndex);
  const previousPathname = useRef(pathname);
  const restoring = useRef(false);

  useEffect(() => {
    const element = ref.current;
    if (!element) return;
    // A restore scrolls too, and its intermediate offsets are not where anybody
    // chose to be - recording them overwrites the position being restored.
    const remember = () => { if (!restoring.current) positions.current.set(currentIndex.current, element.scrollTop); };
    element.addEventListener("scroll", remember, { passive: true });
    return () => {
      remember();
      element.removeEventListener("scroll", remember);
    };
  }, [ref]);

  // Deliberately a layout effect. Replacing one screen with another changes the
  // height of the document, and the browser clamps the scroll position to fit
  // and raises a scroll event for having done so. A passive effect runs after
  // that has already happened, so the clamped offset was filed under the entry
  // being left - overwriting the position that entry existed to remember.
  useLayoutEffect(() => {
    const samePathname = previousPathname.current === pathname;
    previousPathname.current = pathname;
    currentIndex.current = historyIndex;
    const element = ref.current;
    if (!element || navigationType === "replace") return;
    // A push that only rewrote the query string - the next page of a listing,
    // another tab - is still the same screen, and the screen itself knows which
    // part of it the reader just asked for. Forcing the top here scrolled past
    // the author profile they were paging through underneath.
    if (navigationType === "push" && samePathname) return;
    const target = navigationType === "pop" ? positions.current.get(historyIndex) ?? 0 : 0;
    if (target === 0) {
      element.scrollTo({ top: 0, left: 0 });
      return;
    }
    // The screen it is restoring into is still fetching and measuring its rows.
    // Each time it grows the offset has to be reapplied, and each time it is
    // briefly short the browser clamps the offset away and reports that as a
    // scroll - so recording stays off for the whole window, not just until the
    // first successful attempt. Stopping at the first match was enough to lose
    // the position to a virtualised list that finished measuring one frame
    // later, which is what a large library does every time.
    restoring.current = true;
    let frame = 0;
    let held = 0;
    let height = element.scrollHeight;
    let quietSince = performance.now();
    const limit = performance.now() + RESTORE_MAX_MS;
    const stop = () => {
      restoring.current = false;
      cancelAnimationFrame(frame);
      for (const event of USER_SCROLL_EVENTS) element.removeEventListener(event, stop);
    };
    const apply = () => {
      element.scrollTo({ top: target, left: 0 });
      const now = performance.now();
      if (element.scrollHeight !== height) { height = element.scrollHeight; quietSince = now; }
      held = Math.abs(element.scrollTop - target) <= 1 ? held + 1 : 0;
      // Settled: the offset has survived several frames and the screen has
      // stopped growing under it.
      const settled = held >= RESTORE_STABLE_FRAMES && now - quietSince >= RESTORE_QUIET_MS;
      if (settled || now >= limit) {
        stop();
        return;
      }
      frame = requestAnimationFrame(apply);
    };
    // Scrolling by hand outranks the restore: being dragged back to where you
    // were while trying to leave is worse than losing the position.
    for (const event of USER_SCROLL_EVENTS) element.addEventListener(event, stop, { passive: true });
    // The first attempt is made now rather than a frame from now, so a window
    // that is not painting - minimised, in the background - still lands the
    // position it already has the height for.
    apply();
    return stop;
  }, [historyIndex, navigationType, pathname, ref]);
}

function Navigation({ railed, onNavigate }: { railed: boolean; onNavigate?: () => void }) {
  const navigate = useAppNavigate();
  const go = (path: string) => {
    navigate(path);
    onNavigate?.();
  };

  return (
    <Stack h="100%" gap={0} className="app-nav" data-railed={railed || undefined}>
      <Box className="app-nav__brand"><PiepBrand collapsed={railed} onClick={() => go("/")} /></Box>
      <Divider />
      <WorkspaceNav railed={railed} onNavigate={onNavigate} />
      <WorkspaceNavFooter railed={railed} onNavigate={onNavigate} />
      {!railed && (
        <Group px="sm" py={6} justify="space-between" wrap="nowrap">
          <Group gap={8} wrap="nowrap">
            <Indicator color={isTauriRuntime() ? "green" : "gray"} size={7} processing={isTauriRuntime()}>
              <Box w={9} h={9} />
            </Indicator>
            <Text size="xs" c="dimmed">{isTauriRuntime() ? "デスクトップ" : "プレビュー"}</Text>
          </Group>
          <Text size="xs" c="dimmed">v{APP_VERSION}</Text>
        </Group>
      )}
    </Stack>
  );
}


/**
 * Says so when the app is catching its own search index up.
 *
 * Background work that nobody can see is indistinguishable from a slow app, and
 * search results are incomplete until it finishes - so this states both the
 * fact and the progress, and stays out of the way otherwise.
 */
function IndexingIndicator() {
  const navigate = useAppNavigate();
  const progress = useSearchIndexProgress();
  if (!isRebuildRunning(progress)) return null;
  const percent = rebuildPercent(progress);
  const label = percent === null
    ? "検索インデックスを更新しています"
    : `検索インデックスを更新しています（${Math.floor(percent)}%）`;
  return (
    <Tooltip label={`${label}。完了までライブラリ検索の結果は一部だけです。`} multiline w={260}>
      <UnstyledButton
        className="indexing-chip"
        aria-label={label}
        onClick={() => navigate("/settings?section=search")}
      >
        <Loader size={13} color="piep" />
        <Text size="xs" fw={650} visibleFrom="sm">
          索引更新中{percent === null ? "" : ` ${Math.floor(percent)}%`}
        </Text>
      </UnstyledButton>
    </Tooltip>
  );
}

export function AppFrame({ children }: { children: ReactNode }) {
  const navigate = useAppNavigate();
  const location = useAppRouter();
  const [mobileOpened, mobile] = useDisclosure(false);
  // Read on the first render, or the sidebar opens wide and snaps to the rail.
  const [railed, setRailed] = useLocalStorage({ key: "piep.nav-railed", defaultValue: false, getInitialValueInEffect: false });
  // Below the breakpoint the navbar is an overlay drawer, where a 62px rail
  // would just be a broken-looking sliver, so the preference only applies to
  // the docked sidebar.
  const wideEnoughToRail = useMediaQuery("(min-width: 62em)", true);
  const effectiveRailed = railed && wideEnoughToRail;
  const navbarWidth = effectiveRailed ? RAIL_WIDTH : NAVBAR_WIDTH;
  const mainRef = useRef<HTMLElement>(null);
  const { setColorScheme } = useMantineColorScheme();
  const colorScheme = useComputedColorScheme("light");
  const { epubQueue } = useWorkspace();

  const pageTitle = useMemo(() => {
    if (location.pathname === "/") return "ホーム";
    if (location.pathname.startsWith("/library")) return "ライブラリ";
    if (location.pathname.startsWith("/save")) return "保存";
    if (location.pathname.startsWith("/reader")) return "リーダー";
    if (location.pathname.startsWith("/editor")) return "エディタ";
    if (location.pathname.startsWith("/works")) return "作品";
    if (location.pathname.startsWith("/people")) return "作者";
    if (location.pathname.startsWith("/series")) return "シリーズ";
    // 抜けていた。コレクションを開くと窓の題が「piep · piep」になっていた。
    if (location.pathname.startsWith("/collections")) return "コレクション";
    if (location.pathname.startsWith("/epub/templates")) return "テンプレートスタジオ";
    if (location.pathname.startsWith("/epub")) return "EPUB";
    if (location.pathname.startsWith("/updates")) return "更新";
    if (location.pathname.startsWith("/operations")) return "操作履歴";
    if (location.pathname.startsWith("/diagnostics")) return "ライブラリ診断";
    if (location.pathname.startsWith("/settings")) return "設定";
    return "piep";
  }, [location.pathname]);

  useEffect(() => { document.title = `${pageTitle} · piep`; }, [pageTitle]);
  // 自動の更新確認は、どの画面を開いていても回っていてほしい。
  useUpdateScheduler();
  // こちらは piep 自身の版。見つけたら知らせるだけで、入れ替えは押させる。
  useAppUpdateNotice(() => navigate("/settings?section=about"));
  useScrollRestoration(mainRef, location.historyIndex, location.pathname, location.navigationType);
  useHotkeys([
    ["mod+K", () => spotlight.open()],
    ["mod+P", () => spotlight.open()],
    ["mod+L", () => navigate("/library")],
    ["mod+shift+S", () => navigate("/save/pixiv")],
  ]);

  const actions: SpotlightActionData[] = [
    { id: "home", label: "ホームを開く", description: "状況と最近の保存", onClick: () => navigate("/"), leftSection: <Icons.home size={IconSize.nav} /> },
    { id: "library", label: "ライブラリを検索", description: "保存したすべての作品", onClick: () => navigate("/library"), leftSection: <Icons.library size={IconSize.nav} /> },
    { id: "save-pixiv", label: "pixivから保存", description: "内蔵ブラウザを開く", onClick: () => navigate("/save/pixiv"), leftSection: <Icons.collect size={IconSize.nav} /> },
    { id: "save-fanbox", label: "FANBOXから保存", description: "内蔵ブラウザを開く", onClick: () => navigate("/save/fanbox"), leftSection: <Icons.collect size={IconSize.nav} /> },
    { id: "epub", label: `EPUBキューを開く${epubQueue.length ? `（${epubQueue.length}件）` : ""}`, description: "書き出しを設定", onClick: () => navigate("/epub"), leftSection: <Icons.epub size={IconSize.nav} /> },
    { id: "epub-templates", label: "テンプレートスタジオを開く", description: "EPUBの見た目と構成を編集", onClick: () => navigate("/epub/templates"), leftSection: <Icons.epubTemplate size={IconSize.nav} /> },
    { id: "updates", label: "更新を確認", description: "変更と新着をチェック", onClick: () => navigate("/updates"), leftSection: <Icons.updates size={IconSize.nav} /> },
    { id: "operations", label: "操作履歴を開く", description: "進行状況・再試行・ログ", onClick: () => navigate("/operations"), leftSection: <Icons.history size={IconSize.nav} /> },
    { id: "diagnostics", label: "ライブラリを診断", description: "実データ性能・容量・索引", onClick: () => navigate("/settings?section=diagnostics"), leftSection: <Icons.diagnostics size={IconSize.nav} /> },
    { id: "settings", label: "設定を開く", description: "接続・外観・ライブラリ", onClick: () => navigate("/settings"), leftSection: <Icons.settings size={IconSize.nav} /> },
  ];

  return (
    <>
      {/* A normal #main-content link would replace this app's hash route. */}
      <button type="button" className="skip-link" onClick={() => mainRef.current?.focus()}>本文へ移動</button>
      <AppShell
        header={{ height: 58 }}
        navbar={{ width: navbarWidth, breakpoint: "md", collapsed: { mobile: !mobileOpened } }}
        padding={0}
        className="app-shell"
        // Elements pinned over the content (the selection bar, the reader's
        // pager) centre themselves against this, so it has to follow the rail.
        style={{ "--piep-sidebar-width": `${navbarWidth}px` } as React.CSSProperties}
      >
        <AppShell.Header className="app-header">
          <Group h="100%" px={{ base: "sm", md: "md" }} justify="space-between" wrap="nowrap">
            <Group gap="sm" wrap="nowrap">
              <Burger
                opened={mobileOpened}
                onClick={mobile.toggle}
                hiddenFrom="md"
                size="sm"
                aria-label={mobileOpened ? "ナビゲーションを閉じる" : "ナビゲーションを開く"}
                aria-controls="app-navigation"
              />
              {/* Sits exactly where the burger appears on a narrow window, so
                  the control for the sidebar is always in the same place. A
                  panel glyph rather than another set of bars, which would read
                  as a second app menu. */}
              <Tooltip label={effectiveRailed ? "サイドバーを開く" : "サイドバーをたたむ"}>
                <ActionIcon
                  visibleFrom="md"
                  variant="subtle"
                  color="gray"
                  aria-label={effectiveRailed ? "サイドバーを開く" : "サイドバーをたたむ"}
                  aria-expanded={!effectiveRailed}
                  onClick={() => setRailed((value) => !value)}
                >
                  {effectiveRailed ? <Icons.sidebarOpen size={IconSize.nav} /> : <Icons.sidebarClose size={IconSize.nav} />}
                </ActionIcon>
              </Tooltip>
              {/* A desktop window has no browser chrome, so hiding history
                  controls on narrow widths left no way back at all. They are
                  dimmed at the ends of the history: a lit control that does
                  nothing when pressed reads as the app having hung. */}
              <Group gap={4} wrap="nowrap">
                <Tooltip label="戻る"><ActionIcon variant="subtle" color="gray" aria-label="前の画面へ戻る" disabled={!location.canGoBack} onClick={() => navigate(-1)}><Icons.back size={IconSize.nav} /></ActionIcon></Tooltip>
                <Tooltip label="進む"><ActionIcon variant="subtle" color="gray" aria-label="次の画面へ進む" disabled={!location.canGoForward} onClick={() => navigate(1)}><Icons.forward size={IconSize.nav} /></ActionIcon></Tooltip>
                <Divider orientation="vertical" h={20} mx={6} visibleFrom="md" />
                <Text size="sm" fw={680} visibleFrom="md">{pageTitle}</Text>
              </Group>
              <Text size="sm" fw={650} hiddenFrom="md" className="line-clamp-1">{pageTitle}</Text>
            </Group>
            <Group gap={6} wrap="nowrap">
              <IndexingIndicator />
              <Tooltip label="検索または移動（Ctrl K）"><ActionIcon variant="subtle" color="gray" aria-label="検索または移動" onClick={() => spotlight.open()}><Icons.search size={IconSize.nav} /></ActionIcon></Tooltip>
              <Tooltip label={colorScheme === "dark" ? "ライトモード" : "ダークモード"}>
                <ActionIcon variant="subtle" color="gray" aria-label={colorScheme === "dark" ? "ライトモードに切替" : "ダークモードに切替"} onClick={() => setColorScheme(colorScheme === "dark" ? "light" : "dark")}>
                  {colorScheme === "dark" ? <Icons.themeLight size={IconSize.nav} /> : <Icons.themeDark size={IconSize.nav} />}
                </ActionIcon>
              </Tooltip>
              <Menu position="bottom-end" width={220}>
                <Menu.Target>
                  <Tooltip label="メニュー"><ActionIcon variant="subtle" color="gray" aria-label="アプリメニュー"><Icons.appMenu size={IconSize.nav} /></ActionIcon></Tooltip>
                </Menu.Target>
                {/* Only what the navigation rail does not already offer. */}
                <Menu.Dropdown>
                  <Menu.Item leftSection={<Icons.settings size={IconSize.menu} />} onClick={() => navigate("/settings")}>設定</Menu.Item>
                  <Menu.Item leftSection={<Icons.help size={IconSize.menu} />} onClick={() => navigate("/settings?section=about")}>piepについて</Menu.Item>
                </Menu.Dropdown>
              </Menu>
            </Group>
          </Group>
        </AppShell.Header>
        <AppShell.Navbar className="app-navbar" id="app-navigation">
          <Navigation railed={effectiveRailed} onNavigate={mobile.close} />
        </AppShell.Navbar>
        <AppShell.Main ref={mainRef} id="main-content" className="app-main" tabIndex={-1}>
          {children}
        </AppShell.Main>
      </AppShell>
      <Spotlight
        actions={actions}
        nothingFound="一致する操作がありません"
        highlightQuery
        searchProps={{ leftSection: <Icons.search size={IconSize.nav} />, placeholder: "画面や操作を検索…", "aria-label": "画面や操作を検索" }}
      />
    </>
  );
}
