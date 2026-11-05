import { useEffect, useState } from "react";
import { ActionIcon, Button, Image, Loader, Modal, NativeSelect, NavLink, Tooltip, useMantineColorScheme } from "@mantine/core";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import piepIcon from "@/assets/icon.svg";
import { PiepLockup } from "@/components/PiepLockup";
import { Icons, IconSize, type LucideIcon } from "@/lib/icons";
import { adminApi, ApiFailure } from "./api";
import { EntryCard, LoginForm, TicketEntry, takeTicket } from "./Entry";
import type { AdminPage } from "./format";
import { DevicesPage } from "./pages/DevicesPage";
import { LibraryPage } from "./pages/LibraryPage";
import { OverviewPage } from "./pages/OverviewPage";
import { RecordsPage } from "./pages/RecordsPage";
import { SecurityPage } from "./pages/SecurityPage";
import { UpdatesPage } from "./pages/UpdatesPage";
import { onSessionExpired } from "./queryClient";
import { ReauthProvider, showFailure, useStatus } from "./shared";

const PAGES: Array<{ id: AdminPage; label: string; icon: LucideIcon }> = [
  { id: "overview", label: "概要", icon: Icons.home },
  { id: "updates", label: "更新", icon: Icons.updates },
  { id: "devices", label: "端末", icon: Icons.phone },
  { id: "library", label: "ライブラリ", icon: Icons.library },
  { id: "records", label: "記録", icon: Icons.history },
  { id: "security", label: "接続と安全", icon: Icons.secure },
];

function pageFromHash(): AdminPage {
  const name = window.location.hash.replace(/^#/, "");
  return PAGES.some((page) => page.id === name) ? (name as AdminPage) : "overview";
}

function ThemeButton() {
  const { colorScheme, setColorScheme } = useMantineColorScheme();
  const dark = colorScheme === "dark" || (colorScheme === "auto" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  const label = dark ? "明るい表示にする" : "暗い表示にする";
  return (
    <Tooltip label={label}>
      <ActionIcon variant="subtle" color="gray" aria-label={label} onClick={() => setColorScheme(dark ? "light" : "dark")}>
        {dark ? <Icons.themeLight size={IconSize.nav} /> : <Icons.themeDark size={IconSize.nav} />}
      </ActionIcon>
    </Tooltip>
  );
}

function Workspace({ onLoggedOut }: { onLoggedOut: () => void }) {
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
        <div className="app-nav__brand"><PiepLockup size={24} /></div>
        <div className="adm-server">{name} の管理画面</div>
        <div className="adm-links">
          {PAGES.map(({ id, label, icon: Icon }) => (
            <NavLink
              key={id}
              component="button"
              type="button"
              className="app-nav__link"
              active={page === id}
              label={label}
              leftSection={<Icon size={IconSize.nav} strokeWidth={1.8} />}
              aria-current={page === id ? "page" : undefined}
              onClick={() => go(id)}
            />
          ))}
        </div>
        <div className="adm-foot">
          <Button size="compact-sm" variant="subtle" color="gray" leftSection={<Icons.logout size={14} />} onClick={() => void logout()}>ログアウト</Button>
          <ThemeButton />
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
              data={PAGES.map(({ id, label }) => ({ value: id, label }))}
            />
            <ThemeButton />
            <Tooltip label="ログアウト">
              <ActionIcon variant="subtle" color="gray" aria-label="ログアウト" onClick={() => void logout()}><Icons.logout size={IconSize.nav} /></ActionIcon>
            </Tooltip>
          </div>
          {(unreachable || applying) && (
            <div className="adm-notice" role="status">
              {(applying || unreachable) && <Loader size={14} />}
              {applying ? "更新しています。戻ったら続けます。" : "サーバーに届きません。最後に読んだ内容を出しています。"}
            </div>
          )}
          {page === "overview" && <OverviewPage go={go} />}
          {page === "updates" && <UpdatesPage />}
          {page === "devices" && <DevicesPage />}
          {page === "library" && <LibraryPage />}
          {page === "records" && <RecordsPage />}
          {page === "security" && <SecurityPage onDisabled={onLoggedOut} />}
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
      <Workspace onLoggedOut={() => { client.clear(); setExpired(false); void web.refetch(); }} />
      {/* 画面を消さず上に出す。書きかけの設定を失わない。 */}
      <Modal opened={expired} onClose={() => undefined} withCloseButton={false} closeOnEscape={false} closeOnClickOutside={false} centered title="ログインが切れました">
        <LoginForm onDone={entered} />
      </Modal>
    </ReauthProvider>
  );
}
