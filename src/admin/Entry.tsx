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

/** 新しいパスワードが条件を満たしたか。 */
export function newPasswordReady(password: string, again: string): boolean {
  return [...password].length >= MIN_PASSWORD && again === password;
}

/** 新しいパスワードと確認の 2 欄。作成・決め直し・変更で同じ言葉と動きにする。 */
export function NewPasswordFields({ label = "パスワード", password, again, onPassword, onAgain }: {
  label?: string;
  password: string;
  again: string;
  onPassword: (value: string) => void;
  onAgain: (value: string) => void;
}) {
  const length = [...password].length;
  const remaining = Math.max(0, MIN_PASSWORD - length);
  // 打っている途中で赤くしない。確認の欄は、上と同じ長さまで打ってから比べる。
  const mismatch = again.length > 0 && [...again].length >= length && again !== password;
  return (
    <>
      <PasswordInput
        label={label}
        description={remaining > 0 ? `${MIN_PASSWORD} 文字以上（あと ${remaining} 文字）` : `${MIN_PASSWORD} 文字以上`}
        value={password}
        onChange={(event) => onPassword(event.currentTarget.value)}
        autoComplete="new-password"
        required
       
      />
      <PasswordInput
        label={`${label}（確認）`}
        value={again}
        onChange={(event) => onAgain(event.currentTarget.value)}
        autoComplete="new-password"
        error={mismatch ? "パスワードが一致しません" : undefined}
        required
       
      />
    </>
  );
}

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
  const ready = newPasswordReady(password, again) && (resetting || username.trim().length > 0);
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
      {/* 塊の間は広く、パスワードと確認の 2 欄は詰めて 1 組に見せる。 */}
      <Stack gap={26}>
        <EntryHead
          title={resetting ? "パスワードを変更" : "管理者のアカウントを作成"}
          lead={resetting
            ? "新しいパスワードにすると、ログインしているブラウザはすべてログアウトします。"
            : "この管理画面に入るためのアカウントです。作成すると、ほかの PC や携帯のブラウザからも、このパスワードでログインできます。"}
        />
        {resetting
          ? (
            // 変えられない値は入力欄に見せない。
            <div className="adm-entry-fixed">
              <Text size="sm" fw={600}>アカウント名</Text>
              <Text size="sm">{ticket.username}</Text>
            </div>
          )
          : <TextInput label="アカウント名" value={username} onChange={(event) => setUsername(event.currentTarget.value)} autoComplete="username" maxLength={64} required />}
        <Stack gap={10}>
          <NewPasswordFields
            label={resetting ? "新しいパスワード" : "パスワード"}
            password={password}
            again={again}
            onPassword={setPassword}
            onAgain={setAgain}
          />
        </Stack>
        <Stack gap="sm">
          {problem && <Alert color="red" variant="light">{problem}</Alert>}
          <Button type="submit" loading={pending} disabled={!ready} fullWidth>{resetting ? "変更してログイン" : "作成してログイン"}</Button>
        </Stack>
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
        <TextInput label="アカウント名" value={username} onChange={(event) => setUsername(event.currentTarget.value)} autoComplete="username" required />
        <PasswordInput label="パスワード" value={password} onChange={(event) => setPassword(event.currentTarget.value)} autoComplete="current-password" required />
        {problem && <Alert color="red" variant="light">{problem}</Alert>}
        <Button type="submit" loading={pending} disabled={!username || !password} fullWidth mt={4}>ログイン</Button>
      </Stack>
    </form>
  );
}

/** 入口の見出し。どの入口も、題と一文の説明を同じ形で出す。 */
function EntryHead({ title, lead }: { title: string; lead?: string }) {
  return (
    <div className="adm-entry-head">
      <Title order={1} className="adm-entry-title">{title}</Title>
      {lead && <Text size="sm" c="dimmed" className="adm-entry-lead">{lead}</Text>}
    </div>
  );
}

/** ログインの前の画面。ロゴを丸ごと出してから、用件を書く。 */
function EntryFrame({ children }: { children: ReactNode }) {
  return (
    <div className="adm-entry app-shell">
      <div className="adm-entry-card">
        <div className="adm-entry-brand"><PiepLockup size={34} /></div>
        {children}
      </div>
    </div>
  );
}

export function EntryCard({ onDone, hasAccount, notice }: { onDone: () => void; hasAccount: boolean; notice?: string }) {
  return hasAccount ? (
    <EntryFrame>
      <Stack gap={22}>
        <EntryHead title="管理画面にログイン" />
        <LoginForm onDone={onDone} notice={notice ?? null} />
      </Stack>
    </EntryFrame>
  ) : (
    <EntryFrame>
      <Stack gap={22}>
        <EntryHead title="管理者のアカウントがまだありません" lead="アカウントは、一度きりのリンクから作成します。" />
        <ol className="adm-entry-steps">
          <li>デスクトップ版で「設定」→「サーバー」を開く</li>
          <li>「管理画面を開く」を押す</li>
        </ol>
        <Text size="xs" c="dimmed">
          デスクトップ版を使っていない場合は、サーバーの PC で次を実行し、表示されたリンクを開いてください。
        </Text>
        <Text className="adm-code">{`piep-server admin-setup --data <置き場所> --url ${window.location.origin}/`}</Text>
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
      {state ? <SetupForm ticket={state} onDone={onDone} /> : <Stack align="center" gap="sm" py="xl"><Loader size="sm" /><Text size="sm" c="dimmed">リンクを確かめています</Text></Stack>}
    </EntryFrame>
  );
}
