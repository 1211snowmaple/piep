import { useEffect, useState, type ReactNode } from "react";
import { Alert, Button, Loader, PasswordInput, Stack, Text, TextInput, Title } from "@mantine/core";
import { PiepLockup } from "@/components/PiepLockup";
import { adminApi, ApiFailure, failureMessage, type TicketKind } from "./api";

/** アドレスから券を取り出して、すぐに消す。履歴と転送元に残さない。 */
export function takeTicket(): string | null {
  const match = /^#ticket=(.+)$/.exec(window.location.hash);
  if (!match?.[1]) return null;
  window.history.replaceState(null, "", window.location.pathname);
  return match[1];
}

/** 文字数の下限はサーバーも確かめる。画面では入力の途中で知らせるために見る。 */
const MIN_PASSWORD = 15;

interface TicketState {
  ticket: string;
  kind: TicketKind;
  username: string | null;
}

function SetupForm({ ticket, onDone }: { ticket: TicketState; onDone: () => void }) {
  const resetting = ticket.username !== null;
  const [username, setUsername] = useState("admin");
  const [password, setPassword] = useState("");
  const [again, setAgain] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const short = password.length > 0 && [...password].length < MIN_PASSWORD;
  const mismatch = again.length > 0 && again !== password;
  const ready = [...password].length >= MIN_PASSWORD && again === password && (resetting || username.trim().length > 0);
  const submit = async () => {
    setPending(true);
    setProblem(null);
    try {
      await adminApi.setup(ticket.ticket, resetting ? null : username, password);
      onDone();
    } catch (error) {
      setProblem(failureMessage(error));
    } finally {
      setPending(false);
    }
  };
  return (
    <form onSubmit={(event) => { event.preventDefault(); if (ready) void submit(); }}>
      <Stack gap="md">
        <div>
          <Title order={1} fz={22}>{resetting ? "パスワードを決め直す" : "管理者を作る"}</Title>
          <Text size="sm" c="dimmed" mt={6}>
            {resetting ? "決め直すと、今のログインはすべて切れます。" : "このサーバーを管理するアカウントです。あとで別の PC や携帯のブラウザからも入れます。"}
          </Text>
        </div>
        {resetting
          ? <TextInput label="アカウント" value={ticket.username ?? ""} readOnly autoComplete="username" />
          : <TextInput label="アカウント" value={username} onChange={(event) => setUsername(event.currentTarget.value)} autoComplete="username" maxLength={64} required />}
        <PasswordInput
          label="パスワード"
          description="15 文字以上。文章でもかまいません。"
          value={password}
          onChange={(event) => setPassword(event.currentTarget.value)}
          autoComplete="new-password"
          error={short ? "あと少し長くしてください" : undefined}
          required
        />
        <PasswordInput
          label="もう一度"
          value={again}
          onChange={(event) => setAgain(event.currentTarget.value)}
          autoComplete="new-password"
          error={mismatch ? "上と違います" : undefined}
          required
        />
        {problem && <Alert color="red" variant="light">{problem}</Alert>}
        <Button type="submit" loading={pending} disabled={!ready} fullWidth>{resetting ? "決め直す" : "作成して開く"}</Button>
      </Stack>
    </form>
  );
}

export function LoginForm({ onDone, notice = null }: { onDone: () => void; notice?: string | null }) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const submit = async () => {
    setPending(true);
    setProblem(null);
    try {
      await adminApi.login(username, password);
      setPassword("");
      onDone();
    } catch (error) {
      setPassword("");
      if (error instanceof ApiFailure && error.retryAfterSeconds !== null) {
        setProblem(`${error.message}。${Math.ceil(error.retryAfterSeconds / 60)} 分ほど待ってから試してください。`);
      } else {
        setProblem(failureMessage(error));
      }
    } finally {
      setPending(false);
    }
  };
  return (
    <form onSubmit={(event) => { event.preventDefault(); void submit(); }}>
      <Stack gap="md">
        {notice && <Alert color="gray" variant="light">{notice}</Alert>}
        <TextInput label="アカウント" value={username} onChange={(event) => setUsername(event.currentTarget.value)} autoComplete="username" required />
        <PasswordInput label="パスワード" value={password} onChange={(event) => setPassword(event.currentTarget.value)} autoComplete="current-password" required />
        {problem && <Alert color="red" variant="light">{problem}</Alert>}
        <Button type="submit" loading={pending} disabled={!username || !password} fullWidth>ログイン</Button>
      </Stack>
    </form>
  );
}

/** ログインの前の画面。ロゴを丸ごと出してから、用件を書く。 */
function EntryFrame({ title, children }: { title?: string; children: ReactNode }) {
  return (
    <div className="adm-entry app-shell">
      <div className="adm-entry-card">
        <div className="adm-entry-brand"><PiepLockup size={34} /></div>
        {title && <Title order={1} fz={18} ta="center" mb="lg">{title}</Title>}
        {children}
      </div>
    </div>
  );
}

export function EntryCard({ onDone, hasAccount, notice }: { onDone: () => void; hasAccount: boolean; notice?: string }) {
  return hasAccount ? (
    <EntryFrame title="管理画面にログイン"><LoginForm onDone={onDone} notice={notice ?? null} /></EntryFrame>
  ) : (
    <EntryFrame title="管理者がまだいません">
      <Stack gap="sm">
        <Text size="sm">デスクトップ版の「管理画面を開く」から開くか、サーバーで次のコマンドを実行して、表示されたリンクを開いてください。</Text>
        <Text className="adm-code">piep-server admin-setup --data &lt;置き場所&gt;</Text>
      </Stack>
    </EntryFrame>
  );
}

/** 券が付いて開かれたときの入口。 */
export function TicketEntry({ ticket, onDone, onInvalid }: { ticket: string; onDone: () => void; onInvalid: (message: string) => void }) {
  const [state, setState] = useState<TicketState | null>(null);
  useEffect(() => {
    let live = true;
    void (async () => {
      try {
        const info = await adminApi.ticketInfo(ticket);
        if (!live) return;
        if (info.kind === "setup") {
          setState({ ticket, kind: info.kind, username: info.username });
          return;
        }
        await adminApi.useTicket(ticket);
        if (live) onDone();
      } catch (error) {
        if (live) onInvalid(failureMessage(error));
      }
    })();
    return () => { live = false; };
    // 券は一度きりなので、開いたときに一度だけ読む。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return (
    <EntryFrame>
      {state ? <SetupForm ticket={state} onDone={onDone} /> : <Stack align="center" py="xl"><Loader size="sm" /><Text size="sm" c="dimmed">確かめています</Text></Stack>}
    </EntryFrame>
  );
}
