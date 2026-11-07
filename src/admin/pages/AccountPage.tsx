import { useState } from "react";
import { Alert, Button, Group, Skeleton, Stack, Text } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Icons, IconSize } from "@/lib/icons";
import { adminApi, failureMessage, type AdminSession } from "../api";
import { NewPasswordFields, newPasswordReady } from "../Entry";
import { relativeTime } from "../format";
import { Card, Facts, PageHead, showFailure, useReauth } from "../shared";

/** ブラウザの名前を読める形にする。正確さより見分けやすさを優先する。 */
export function browserLabel(agent: string | null): string {
  if (!agent) return "不明なブラウザ";
  const browser = /Edg\//.test(agent) ? "Edge" : /Firefox\//.test(agent) ? "Firefox" : /Chrome\//.test(agent) ? "Chrome" : /Safari\//.test(agent) ? "Safari" : "ブラウザ";
  const system = /Android/.test(agent) ? "Android" : /iPhone|iPad/.test(agent) ? "iOS" : /Windows/.test(agent) ? "Windows" : /Mac OS/.test(agent) ? "Mac" : /Linux/.test(agent) ? "Linux" : null;
  return system ? `${browser}（${system}）` : browser;
}

function PasswordCard() {
  const { confirmed } = useReauth();
  const [password, setPassword] = useState("");
  const [again, setAgain] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const ready = newPasswordReady(password, again);
  const submit = async () => {
    setPending(true);
    setProblem(null);
    try {
      const value = password;
      const done = await confirmed(async () => { await adminApi.changePassword(value); return true; });
      if (done) {
        setPassword("");
        setAgain("");
        notifications.show({ message: "パスワードを変更しました。ほかのブラウザはログアウトしました。" });
      }
    } catch (error) {
      setProblem(failureMessage(error));
    } finally {
      setPending(false);
    }
  };
  return (
    <Card title="パスワード" lead="変えると、このブラウザ以外はログアウトします。">
      <form onSubmit={(event) => { event.preventDefault(); if (ready) void submit(); }}>
        <Stack gap="md" maw={400}>
          <NewPasswordFields label="新しいパスワード" password={password} again={again} onPassword={setPassword} onAgain={setAgain} />
          {problem && <Alert color="red" variant="light">{problem}</Alert>}
          <Group><Button type="submit" loading={pending} disabled={!ready}>パスワードを変更</Button></Group>
        </Stack>
      </form>
    </Card>
  );
}

function SessionTable({ sessions, currentId, onEnd }: { sessions: AdminSession[]; currentId: string | null; onEnd: (id: string) => void }) {
  return (
    <div className="adm-table-wrap">
      <table className="adm-table">
        <thead>
          <tr>
            <th>ブラウザ</th>
            <th>接続元</th>
            <th>ログイン</th>
            <th>最後の操作</th>
            <th aria-label="操作" />
          </tr>
        </thead>
        <tbody>
          {sessions.map((session) => {
            const current = session.id === currentId;
            return (
              <tr key={session.id}>
                <td data-label="ブラウザ">
                  <span className="adm-strong">{browserLabel(session.label)}</span>
                  {current && <span className="adm-tag">このブラウザ</span>}
                </td>
                <td data-label="接続元" className="adm-num">{session.ip ?? "不明"}</td>
                <td data-label="ログイン" className="adm-num">{relativeTime(session.createdAt)}</td>
                <td data-label="最後の操作" className="adm-num">{relativeTime(session.lastSeenAt)}</td>
                <td className="adm-cell-action">
                  {!current && <Button size="compact-sm" variant="default" onClick={() => onEnd(session.id)}>ログアウトさせる</Button>}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

export function AccountPage({ username, onLogout }: { username: string; onLogout: () => void }) {
  const client = useQueryClient();
  const sessions = useQuery({ queryKey: ["sessions"], queryFn: adminApi.sessions, refetchInterval: 30_000 });
  const current = sessions.data?.sessions.find((session) => session.id === sessions.data?.currentId);
  const endSession = (id: string) => {
    void adminApi.endSession(id).then(() => client.invalidateQueries({ queryKey: ["sessions"] })).catch(showFailure);
  };
  return (
    <div className="adm-section">
      <PageHead title="アカウント" lead="この管理画面にログインするアカウントです。管理者は 1 人だけです。" />
      <Card
        title="ログイン中のアカウント"
        side={<Button variant="default" leftSection={<Icons.logout size={IconSize.menu} />} onClick={onLogout}>ログアウト</Button>}
      >
        <div className="adm-account-card">
          <span className="adm-avatar adm-avatar-large" aria-hidden>{username.slice(0, 1).toUpperCase()}</span>
          <Facts items={[
            ["アカウント名", <span className="adm-strong" key="name">{username}</span>],
            ["役割", "管理者"],
            ["このログイン", current ? `${relativeTime(current.createdAt)}から・${browserLabel(current.label)}` : "—"],
          ]} />
        </div>
      </Card>
      <PasswordCard />
      <Card title="ログイン中のブラウザ" lead="心当たりのないものは、ログアウトさせてからパスワードを変えてください。" flush>
        {sessions.isPending ? <div className="adm-card-pad"><Skeleton height={60} /></div> : (sessions.data?.sessions ?? []).length === 0 ? <Text className="adm-card-pad" c="dimmed">ありません。</Text> : (
          <SessionTable sessions={sessions.data?.sessions ?? []} currentId={sessions.data?.currentId ?? null} onEnd={endSession} />
        )}
      </Card>
    </div>
  );
}
