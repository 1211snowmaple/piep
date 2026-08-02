import { useEffect, useMemo, useRef, type ReactNode } from "react";
import {
  ActionIcon,
  AppShell,
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
import { useDisclosure, useHotkeys } from "@mantine/hooks";
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
  RefreshCw,
  Search,
  Settings,
  Sun,
} from "lucide-react";
import piepIcon from "@/assets/icon.svg";
import { useAppNavigate, useAppRouter } from "@/app/router";
import { useWorkspace } from "@/app/WorkspaceContext";
import { isTauriRuntime } from "@/services/dbApi";

interface NavItem {
  label: string;
  description: string;
  path: string;
  icon: typeof Home;
  badge?: number;
}

function PiepBrand({ collapsed = false, onClick }: { collapsed?: boolean; onClick?: () => void }) {
  return (
    <UnstyledButton className="app-brand" onClick={onClick} aria-label="ホームへ移動">
    <Group gap="sm" wrap="nowrap">
      <Image src={piepIcon} alt="" w={34} h={34} className="app-logo" />
      {!collapsed && (
        <Box>
          <Text fw={800} fz="lg" lh={1}>piep</Text>
          <Text size="xs" c="dimmed" mt={4}>Creative library</Text>
        </Box>
      )}
    </Group>
    </UnstyledButton>
  );
}

function Navigation({ onNavigate }: { onNavigate?: () => void }) {
  const navigate = useAppNavigate();
  const location = useAppRouter();
  const { epubQueue } = useWorkspace();
  const sections: { label: string; items: NavItem[] }[] = [
    {
      label: "Workspace",
      items: [
        { label: "ホーム", description: "今日の状況とクイック操作", path: "/", icon: Home },
        { label: "ライブラリ", description: "保存した作品を探す", path: "/library", icon: LibraryBig },
        { label: "保存", description: "Webから作品を取り込む", path: "/save/pixiv", icon: Download },
      ],
    },
    {
      label: "Production",
      items: [
        { label: "EPUB", description: epubQueue.length ? `${epubQueue.length}冊を書き出し待ち` : "電子書籍を書き出す", path: "/epub", icon: BookText },
        { label: "更新", description: "新着と変更をまとめて確認", path: "/updates", icon: RefreshCw },
      ],
    },
  ];

  const go = (path: string) => {
    navigate(path);
    onNavigate?.();
  };

  return (
    <Stack h="100%" gap={0}>
      <Box px="md" py="lg"><PiepBrand onClick={() => go("/")} /></Box>
      <Divider />
      <ScrollArea flex={1} px="sm" py="md" type="auto">
        <Stack gap="lg">
          {sections.map((section) => (
            <Stack gap={6} key={section.label}>
              <Text px="sm" size="10px" fw={800} c="dimmed" tt="uppercase" lts="0.1em">{section.label}</Text>
              {section.items.map((item) => {
                const active = item.path === "/" ? location.pathname === "/" : location.pathname.startsWith(item.path);
                const Icon = item.icon;
                return (
                  <NavLink
                    key={item.path}
                    active={active}
                    label={item.label}
                    description={item.description}
                    leftSection={<Icon size={18} strokeWidth={1.8} />}
                    onClick={() => go(item.path)}
                    aria-current={active ? "page" : undefined}
                  />
                );
              })}
            </Stack>
          ))}
        </Stack>
      </ScrollArea>
      <Divider />
      <Stack gap={4} p="sm">
        <NavLink label="設定" description="接続・表示・保存先" leftSection={<Settings size={18} />} active={location.pathname.startsWith("/settings")} onClick={() => go("/settings")} />
        <Group px="sm" py={8} justify="space-between">
          <Group gap={8}>
            <Indicator color={isTauriRuntime() ? "green" : "gray"} size={7} processing={isTauriRuntime()}>
              <Box w={9} h={9} />
            </Indicator>
            <Text size="xs" c="dimmed">{isTauriRuntime() ? "Desktop ready" : "Preview mode"}</Text>
          </Group>
          <Text size="xs" c="dimmed">v0.2</Text>
        </Group>
      </Stack>
    </Stack>
  );
}

export function AppFrame({ children }: { children: ReactNode }) {
  const navigate = useAppNavigate();
  const location = useAppRouter();
  const [mobileOpened, mobile] = useDisclosure(false);
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
        navbar={{ width: 248, breakpoint: "md", collapsed: { mobile: !mobileOpened } }}
        padding={0}
        className="app-shell"
      >
        <AppShell.Header className="app-header">
          <Group h="100%" px={{ base: "sm", md: "md" }} justify="space-between" wrap="nowrap">
            <Group gap="sm" wrap="nowrap">
              <Burger opened={mobileOpened} onClick={mobile.toggle} hiddenFrom="md" size="sm" aria-label="ナビゲーションを開く" />
              <Group gap={4} visibleFrom="md">
                <Tooltip label="戻る"><ActionIcon variant="subtle" color="gray" aria-label="前の画面へ戻る" onClick={() => navigate(-1)}><ArrowLeft size={18} /></ActionIcon></Tooltip>
                <Tooltip label="進む"><ActionIcon variant="subtle" color="gray" aria-label="次の画面へ進む" onClick={() => navigate(1)}><ArrowRight size={18} /></ActionIcon></Tooltip>
                <Divider orientation="vertical" h={20} mx={6} />
                <Text size="sm" fw={680}>{pageTitle}</Text>
              </Group>
              <Text size="sm" fw={650} hiddenFrom="md">{pageTitle}</Text>
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
                <Menu.Dropdown>
                  <Menu.Label>piep</Menu.Label>
                  <Menu.Item leftSection={<LibraryBig size={15} />} onClick={() => navigate("/library")}>ライブラリを開く</Menu.Item>
                  <Menu.Item leftSection={<Download size={15} />} onClick={() => navigate("/save/pixiv")}>新しく保存</Menu.Item>
                  <Menu.Divider />
                  <Menu.Item leftSection={<Settings size={15} />} onClick={() => navigate("/settings")}>設定</Menu.Item>
                  <Menu.Item leftSection={<CircleHelp size={15} />} onClick={() => navigate("/settings?section=about")}>piepについて</Menu.Item>
                </Menu.Dropdown>
              </Menu>
            </Group>
          </Group>
        </AppShell.Header>
        <AppShell.Navbar className="app-navbar"><Navigation onNavigate={mobile.close} /></AppShell.Navbar>
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
