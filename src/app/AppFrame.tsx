import { useEffect, useMemo, useRef, type ReactNode } from "react";
import {
  ActionIcon,
  AppShell,
  Badge,
  Box,
  Burger,
  Divider,
  Group,
  Image,
  Indicator,
  Menu,
  NavLink,
  ScrollArea,
  Stack,
  Text,
  Tooltip,
  UnstyledButton,
  useComputedColorScheme,
  useMantineColorScheme,
} from "@mantine/core";
import { useDisclosure, useHotkeys, useLocalStorage, useMediaQuery } from "@mantine/hooks";
import { Spotlight, spotlight, type SpotlightActionData } from "@mantine/spotlight";
import {
  ArrowLeft,
  ArrowRight,
  BookOpen,
  BookText,
  CircleHelp,
  Download,
  FolderHeart,
  Home,
  LibraryBig,
  Menu as MenuIcon,
  Moon,
  PanelLeftClose,
  PanelLeftOpen,
  RefreshCw,
  Search,
  Settings,
  Sun,
} from "lucide-react";
import piepIcon from "@/assets/icon.svg";
import piepLockup from "@/assets/lockup.svg";
import { useAppNavigate, useAppRouter } from "@/app/router";
import { useWorkspace } from "@/app/WorkspaceContext";
import { isTauriRuntime } from "@/services/dbApi";

interface NavItem {
  label: string;
  path: string;
  icon: typeof Home;
  badge?: number;
}

const RAIL_WIDTH = 62;
const NAVBAR_WIDTH = 194;

function PiepBrand({ collapsed = false, onClick }: { collapsed?: boolean; onClick?: () => void }) {
  return (
    <UnstyledButton className="app-brand" onClick={onClick} aria-label="ホームへ移動">
      {collapsed
        ? <Image src={piepIcon} alt="piep" className="app-brand__mark" />
        : <Image src={piepLockup} alt="piep" className="app-brand__lockup" />}
    </UnstyledButton>
  );
}

function NavItemLink({ item, active, railed, onSelect }: { item: NavItem; active: boolean; railed: boolean; onSelect: () => void }) {
  const Icon = item.icon;
  const link = (
    <NavLink
      className="app-nav__link"
      active={active}
      label={railed ? undefined : item.label}
      leftSection={<Icon size={19} strokeWidth={1.8} />}
      rightSection={!railed && item.badge ? <Badge size="xs" variant="light" circle>{item.badge}</Badge> : undefined}
      onClick={onSelect}
      aria-label={railed ? item.label : undefined}
      aria-current={active ? "page" : undefined}
    />
  );
  // In the rail the label is gone, so the tooltip is the only affordance left.
  return railed ? <Tooltip label={item.label} position="right" withArrow>{link}</Tooltip> : link;
}

function Navigation({ railed, onNavigate }: { railed: boolean; onNavigate?: () => void }) {
  const navigate = useAppNavigate();
  const location = useAppRouter();
  const { epubQueue } = useWorkspace();
  // The descriptions repeated what each label already said and forced the rail
  // wide enough to eat into the content area.
  const sections: { label: string; items: NavItem[] }[] = [
    {
      label: "Workspace",
      items: [
        { label: "ホーム", path: "/", icon: Home },
        { label: "ライブラリ", path: "/library", icon: LibraryBig },
        { label: "保存", path: "/save/pixiv", icon: Download },
      ],
    },
    {
      label: "Production",
      items: [
        { label: "EPUB", path: "/epub", icon: BookText, badge: epubQueue.length || undefined },
        { label: "更新", path: "/updates", icon: RefreshCw },
      ],
    },
  ];

  const go = (path: string) => {
    navigate(path);
    onNavigate?.();
  };
  const isActive = (path: string) => {
    const root = `/${path.split("/").filter(Boolean)[0] ?? ""}`;
    return path === "/" ? location.pathname === "/" : location.pathname === root || location.pathname.startsWith(`${root}/`);
  };

  return (
    <Stack h="100%" gap={0} className="app-nav" data-railed={railed || undefined}>
      <Box className="app-nav__brand"><PiepBrand collapsed={railed} onClick={() => go("/")} /></Box>
      <Divider />
      <ScrollArea flex={1} px={railed ? 6 : "xs"} py="sm" type="auto">
        <Stack gap="md">
          {sections.map((section) => (
            <Stack gap={2} key={section.label}>
              {railed
                ? <Divider my={4} />
                : <Text px="sm" size="10px" fw={800} c="dimmed" tt="uppercase" lts="0.1em" mb={2}>{section.label}</Text>}
              {section.items.map((item) => (
                <NavItemLink key={item.path} item={item} active={isActive(item.path)} railed={railed} onSelect={() => go(item.path)} />
              ))}
            </Stack>
          ))}
        </Stack>
      </ScrollArea>
      <Divider />
      <Stack gap={2} p={railed ? 6 : "xs"}>
        <NavItemLink item={{ label: "設定", path: "/settings", icon: Settings }} active={isActive("/settings")} railed={railed} onSelect={() => go("/settings")} />
        {!railed && (
          <Group px="sm" py={6} justify="space-between" wrap="nowrap">
            <Group gap={8} wrap="nowrap">
              <Indicator color={isTauriRuntime() ? "green" : "gray"} size={7} processing={isTauriRuntime()}>
                <Box w={9} h={9} />
              </Indicator>
              <Text size="xs" c="dimmed">{isTauriRuntime() ? "Desktop ready" : "Preview"}</Text>
            </Group>
            <Text size="xs" c="dimmed">v0.2</Text>
          </Group>
        )}
      </Stack>
    </Stack>
  );
}

export function AppFrame({ children }: { children: ReactNode }) {
  const navigate = useAppNavigate();
  const location = useAppRouter();
  const [mobileOpened, mobile] = useDisclosure(false);
  const [railed, setRailed] = useLocalStorage({ key: "piep.nav-railed", defaultValue: false });
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
    if (location.pathname.startsWith("/epub")) return "EPUB";
    if (location.pathname.startsWith("/updates")) return "更新";
    if (location.pathname.startsWith("/settings")) return "設定";
    return "piep";
  }, [location.pathname]);

  useEffect(() => { document.title = `${pageTitle} · piep`; }, [pageTitle]);
  useEffect(() => { mainRef.current?.scrollTo({ top: 0, left: 0 }); }, [location.pathname, location.search]);
  useHotkeys([
    ["mod+K", () => spotlight.open()],
    ["mod+P", () => spotlight.open()],
    ["mod+L", () => navigate("/library")],
    ["mod+shift+S", () => navigate("/save/pixiv")],
  ]);

  const actions: SpotlightActionData[] = [
    { id: "home", label: "ホームを開く", description: "状況と最近の保存", onClick: () => navigate("/"), leftSection: <Home size={18} /> },
    { id: "library", label: "ライブラリを検索", description: "保存したすべての作品", onClick: () => navigate("/library"), leftSection: <LibraryBig size={18} /> },
    { id: "save-pixiv", label: "pixivから保存", description: "内蔵ブラウザを開く", onClick: () => navigate("/save/pixiv"), leftSection: <Download size={18} /> },
    { id: "save-fanbox", label: "FANBOXから保存", description: "内蔵ブラウザを開く", onClick: () => navigate("/save/fanbox"), leftSection: <FolderHeart size={18} /> },
    { id: "epub", label: `EPUBキューを開く${epubQueue.length ? `（${epubQueue.length}件）` : ""}`, description: "書き出しを設定", onClick: () => navigate("/epub"), leftSection: <BookOpen size={18} /> },
    { id: "updates", label: "更新を確認", description: "変更と新着をチェック", onClick: () => navigate("/updates"), leftSection: <RefreshCw size={18} /> },
    { id: "settings", label: "設定を開く", description: "接続・外観・ライブラリ", onClick: () => navigate("/settings"), leftSection: <Settings size={18} /> },
  ];

  return (
    <>
      <a className="skip-link" href="#main-content">本文へ移動</a>
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
              <Burger opened={mobileOpened} onClick={mobile.toggle} hiddenFrom="md" size="sm" aria-label="ナビゲーションを開く" />
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
                  {effectiveRailed ? <PanelLeftOpen size={18} /> : <PanelLeftClose size={18} />}
                </ActionIcon>
              </Tooltip>
              {/* A desktop window has no browser chrome, so hiding history
                  controls on narrow widths left no way back at all. */}
              <Group gap={4} wrap="nowrap">
                <Tooltip label="戻る"><ActionIcon variant="subtle" color="gray" aria-label="前の画面へ戻る" onClick={() => navigate(-1)}><ArrowLeft size={18} /></ActionIcon></Tooltip>
                <Tooltip label="進む"><ActionIcon variant="subtle" color="gray" aria-label="次の画面へ進む" onClick={() => navigate(1)}><ArrowRight size={18} /></ActionIcon></Tooltip>
                <Divider orientation="vertical" h={20} mx={6} visibleFrom="md" />
                <Text size="sm" fw={680} visibleFrom="md">{pageTitle}</Text>
              </Group>
              <Text size="sm" fw={650} hiddenFrom="md" className="line-clamp-1">{pageTitle}</Text>
            </Group>
            <Group gap={6} wrap="nowrap">
              <Tooltip label="検索または移動（Ctrl K）"><ActionIcon variant="subtle" color="gray" aria-label="検索または移動" onClick={() => spotlight.open()}><Search size={18} /></ActionIcon></Tooltip>
              <Tooltip label="ヘルプ"><ActionIcon variant="subtle" color="gray" aria-label="ヘルプ" onClick={() => navigate("/settings?section=about")}><CircleHelp size={18} /></ActionIcon></Tooltip>
              <Tooltip label={colorScheme === "dark" ? "ライトモード" : "ダークモード"}>
                <ActionIcon variant="subtle" color="gray" aria-label={colorScheme === "dark" ? "ライトモードに切替" : "ダークモードに切替"} onClick={() => setColorScheme(colorScheme === "dark" ? "light" : "dark")}>
                  {colorScheme === "dark" ? <Sun size={18} /> : <Moon size={18} />}
                </ActionIcon>
              </Tooltip>
              <Menu position="bottom-end" width={220}>
                <Menu.Target>
                  <Tooltip label="メニュー"><ActionIcon variant="subtle" color="gray" aria-label="アプリメニュー"><MenuIcon size={18} /></ActionIcon></Tooltip>
                </Menu.Target>
                {/* Only what the navigation rail does not already offer. */}
                <Menu.Dropdown>
                  <Menu.Item leftSection={<Settings size={15} />} onClick={() => navigate("/settings")}>設定</Menu.Item>
                  <Menu.Item leftSection={<CircleHelp size={15} />} onClick={() => navigate("/settings?section=about")}>piepについて</Menu.Item>
                </Menu.Dropdown>
              </Menu>
            </Group>
          </Group>
        </AppShell.Header>
        <AppShell.Navbar className="app-navbar">
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
        searchProps={{ leftSection: <Search size={18} />, placeholder: "画面や操作を検索…", "aria-label": "画面や操作を検索" }}
      />
    </>
  );
}
