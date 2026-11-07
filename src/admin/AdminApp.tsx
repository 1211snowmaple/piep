import { useEffect, useState } from "react";
import { Image, Loader, Menu, Modal, NativeSelect, NavLink, UnstyledButton, useMantineColorScheme, type MantineColorScheme } from "@mantine/core";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import piepIcon from "@/assets/icon.svg";
import { PiepLockup } from "@/components/PiepLockup";
import { Icons, IconSize, type LucideIcon } from "@/lib/icons";
import { adminApi, ApiFailure } from "./api";
import { EntryCard, LoginForm, TicketEntry, takeTicket } from "./Entry";
import { pageFromName, type AdminPage } from "./format";
import { AccountPage } from "./pages/AccountPage";
import { DevicesPage } from "./pages/DevicesPage";
import { LibraryPage } from "./pages/LibraryPage";
import { OverviewPage } from "./pages/OverviewPage";
import { RecordsPage } from "./pages/RecordsPage";
import { SettingsPage } from "./pages/SettingsPage";
import { UpdatesPage } from "./pages/UpdatesPage";
import { onSessionExpired } from "./queryClient";
import { ReauthProvider, showFailure, useStatus } from "./shared";

const PAGES: Array<{ id: AdminPage; label: string; icon: LucideIcon }> = [
  { id: "overview", label: "概要", icon: Icons.home },
  { id: "updates", label: "更新", icon: Icons.updates },
  { id: "devices", label: "端末", icon: Icons.phone },
  { id: "library", label: "ライブラリ", icon: Icons.library },
  { id: "records", label: "記録", icon: Icons.history },
  { id: "settings", label: "設定", icon: Icons.settings },
];

function pageFromHash(): AdminPage {
  return pageFromName(window.location.hash.replace(/^#/, "")) ?? "overview";
}

const THEMES: Array<{ value: MantineColorScheme; label: string; icon: LucideIcon }> = [
  { value: "light", label: "明るい", icon: Icons.themeLight },
  { value: "dark", label: "暗い", icon: Icons.themeDark },
  { value: "auto", label: "端末に合わせる", icon: Icons.appearance },
];

/** 自分のアカウント。表示の切り替えとログアウトもここに置く。 */
function AccountMenu({ username, compact, onAccount, onLogout }: { username: string; compact: boolean; onAccount: () => void; onLogout: () => void }) {
  const { colorScheme, setColorScheme } = useMantineColorScheme();
  const initial = username.slice(0, 1).toUpperCase() || "?";
  return (
    <Menu position={compact ? "bottom-end" : "top-start"} width={compact ? 240 : "target"} offset={6}>
      <Menu.Target>
        {compact ? (
          <UnstyledButton className="adm-avatar-button" aria-label={`${username}（アカウントと表示）`}>
            <span className="adm-avatar" aria-hidden>{initial}</span>
          </UnstyledButton>
        ) : (
          <UnstyledButton className="adm-account" aria-label={`${username}（アカウントと表示）`}>
            <span className="adm-avatar" aria-hidden>{initial}</span>
            <span className="adm-account-text">
              <span className="adm-account-name">{username}</span>
              <span className="adm-account-role">管理者</span>
            </span>
            <Icons.more size={IconSize.menu} className="adm-account-more" aria-hidden />
          </UnstyledButton>
        )}
      </Menu.Target>
      <Menu.Dropdown>
        {compact && <Menu.Label>{username}（管理者）</Menu.Label>}
        <Menu.Item leftSection={<Icons.person size={IconSize.menu} />} onClick={onAccount}>アカウント</Menu.Item>
        <Menu.Divider />
        <Menu.Label>表示</Menu.Label>
        {THEMES.map(({ value, label, icon: Icon }) => (
          <Menu.Item
            key={value}
            leftSection={<Icon size={IconSize.menu} />}
            rightSection={colorScheme === value ? <Icons.confirm size={IconSize.menu} aria-label="選択中" /> : null}
            closeMenuOnClick={false}
            onClick={() => setColorScheme(value)}
          >{label}</Menu.Item>
        ))}
        <Menu.Divider />
        <Menu.Item leftSection={<Icons.logout size={IconSize.menu} />} onClick={onLogout}>ログアウト</Menu.Item>
      </Menu.Dropdown>
    </Menu>
  );
}

function Workspace({ username, onLoggedOut }: { username: string; onLoggedOut: () => void }) {
  const [page, setPage] = useState<AdminPage>(pageFromHash);
  const status = useStatus();
  const go = (next: AdminPage) => {
    setPage(next);
    window.history.replaceState(null, "", `#${next}`);
    window.scrollTo({ top: 0 });
  };
  useEffect(() => {
    const onHash = () => setPage(pageFromHash());
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);
  const unreachable = status.isError && !(status.error instanceof ApiFailure);
  const applying = status.data?.update?.phase.state === "applying";
  const logout = async () => {
    try {
      await adminApi.logout();
      onLoggedOut();
    } catch (error) {
      showFailure(error);
    }
  };
  const name = status.data?.serverName ?? "サーバー";
  return (
    <div className="adm app-shell">
      <nav className="adm-nav app-navbar" aria-label="管理の項目">
        <div className="app-nav__brand adm-brand">
          <div className="app-brand"><PiepLockup size={24} /></div>
          <div className="adm-server" title={name}>{name}</div>
        </div>
        <div className="adm-links">
          {PAGES.map(({ id, label, icon: Icon }) => (
            <NavLink
              key={id}
              component="button"
              type="button"
              className="app-nav__link adm-link"
              active={page === id}
              label={label}
              leftSection={<Icon size={IconSize.nav} strokeWidth={1.8} />}
              aria-current={page === id ? "page" : undefined}
              onClick={() => go(id)}
            />
          ))}
        </div>
        <div className="adm-foot">
          <AccountMenu username={username} compact={false} onAccount={() => go("account")} onLogout={() => void logout()} />
        </div>
      </nav>
      <main className="adm-main">
        <div className="adm-body">
          <div className="adm-select">
            <Image src={piepIcon} alt="piep" className="app-brand__mark" />
            <NativeSelect
              aria-label="管理の項目"
              style={{ flex: 1, minWidth: 0 }}
              value={page}
              onChange={(event) => go(event.currentTarget.value as AdminPage)}
              data={[...PAGES.map(({ id, label }) => ({ value: id, label })), { value: "account", label: "アカウント" }]}
            />
            <AccountMenu username={username} compact onAccount={() => go("account")} onLogout={() => void logout()} />
          </div>
          {(unreachable || applying) && (
            <div className="adm-notice" role="status">
              <Loader size={14} />
              {applying ? "更新しています。戻ったら続けます。" : "サーバーに届きません。最後に読んだ内容を出しています。"}
            </div>
          )}
          {page === "overview" && <OverviewPage go={go} />}
          {page === "updates" && <UpdatesPage />}
          {page === "devices" && <DevicesPage />}
          {page === "library" && <LibraryPage />}
          {page === "records" && <RecordsPage />}
          {page === "settings" && <SettingsPage onDisabled={onLoggedOut} />}
          {page === "account" && <AccountPage username={username} onLogout={() => void logout()} />}
        </div>
      </main>
    </div>
  );
}

export function AdminApp() {
  const client = useQueryClient();
  const [ticket, setTicket] = useState<string | null>(takeTicket);
  const [notice, setNotice] = useState<string | undefined>();
  const [expired, setExpired] = useState(false);
  const web = useQuery({ queryKey: ["web-state"], queryFn: adminApi.webState, enabled: ticket === null, retry: 2 });
  useEffect(() => onSessionExpired(() => setExpired(true)), []);

  const entered = () => {
    setTicket(null);
    setExpired(false);
    setNotice(undefined);
    void client.invalidateQueries();
  };

  if (ticket !== null) {
    return (
      <TicketEntry
        ticket={ticket}
        onDone={entered}
        onInvalid={(message) => {
          setTicket(null);
          setNotice(message);
        }}
      />
    );
  }
  if (web.isPending) {
    return <div className="adm-entry"><Loader size="sm" aria-label="読み込み中" /></div>;
  }
  if (!web.data?.authenticated) {
    return <EntryCard hasAccount={web.data?.account ?? true} notice={notice ?? (web.isError ? "サーバーに届きませんでした。" : undefined)} onDone={entered} />;
  }
  return (
    <ReauthProvider>
      <Workspace username={web.data.username ?? "管理者"} onLoggedOut={() => { client.clear(); setExpired(false); void web.refetch(); }} />
      {/* 画面を消さず上に出す。書きかけの設定を失わない。 */}
      <Modal opened={expired} onClose={() => undefined} withCloseButton={false} closeOnEscape={false} closeOnClickOutside={false} centered title="ログインが切れました">
        <LoginForm onDone={entered} />
      </Modal>
    </ReauthProvider>
  );
}
