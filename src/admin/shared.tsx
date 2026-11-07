import { createContext, useCallback, useContext, useRef, useState, type ReactNode } from "react";
import { Button, Group, Modal, PasswordInput, Stack, Text, Title } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { useQuery } from "@tanstack/react-query";
import { adminApi, ApiFailure, failureMessage, type AdminStatus } from "./api";
import { notifyIfExpired } from "./queryClient";

/** ページの見出し。操作は見出しの右に並べ、説明は見出しの下に置く。 */
export function PageHead({ title, lead, children }: { title: string; lead?: ReactNode; children?: ReactNode }) {
  return (
    <header className="adm-head">
      <div className="adm-head-text">
        <Title order={1} className="adm-title">{title}</Title>
        {lead && <Text className="adm-lead">{lead}</Text>}
      </div>
      {children && <Group gap={8} wrap="nowrap" className="adm-head-actions">{children}</Group>}
    </header>
  );
}

export function Card({ title, lead, side, flush, children }: { title?: string; lead?: ReactNode; side?: ReactNode; flush?: boolean; children: ReactNode }) {
  return (
    <section className="adm-card" data-flush={flush || undefined} aria-label={title}>
      {title && (
        <div className="adm-card-head">
          <div className="adm-card-title">
            <span>{title}</span>
            {lead && <span className="adm-card-lead">{lead}</span>}
          </div>
          {side && <div className="adm-card-side">{side}</div>}
        </div>
      )}
      {children}
    </section>
  );
}

export function Facts({ items }: { items: Array<[string, ReactNode]> }) {
  return (
    <dl className="adm-facts" style={{ margin: 0 }}>
      {items.map(([label, value]) => (
        <div key={label} style={{ display: "contents" }}>
          <dt>{label}</dt>
          <dd>{value}</dd>
        </div>
      ))}
    </dl>
  );
}

export function showFailure(error: unknown): void {
  notifyIfExpired(error);
  if (error instanceof ApiFailure && error.status === 401) return;
  notifications.show({ color: "red", message: failureMessage(error) });
}

/** 全ページで読む状況。止まっている間も最後の内容を残す。 */
export function useStatus() {
  return useQuery<AdminStatus>({
    queryKey: ["status"],
    queryFn: adminApi.status,
    refetchInterval: (query) => {
      const phase = query.state.data?.update?.phase.state;
      return query.state.status === "error" || phase === "applying" || phase === "checking" ? 2_000 : 15_000;
    },
    refetchIntervalInBackground: false,
  });
}

interface ReauthContextValue {
  /** 大事な操作を通す。パスワードを聞き直されたら、入力のあとにもう一度試す。 */
  confirmed: <T>(action: () => Promise<T>) => Promise<T | undefined>;
}

const ReauthContext = createContext<ReauthContextValue | null>(null);

export function useReauth(): ReauthContextValue {
  const value = useContext(ReauthContext);
  if (!value) throw new Error("ReauthProvider が必要です");
  return value;
}

export function ReauthProvider({ children }: { children: ReactNode }) {
  const [opened, setOpened] = useState(false);
  const [password, setPassword] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const waiting = useRef<((passed: boolean) => void) | null>(null);

  const ask = useCallback(() => new Promise<boolean>((resolve) => {
    waiting.current = resolve;
    setPassword("");
    setProblem(null);
    setOpened(true);
  }), []);

  const confirmed = useCallback(async <T,>(action: () => Promise<T>): Promise<T | undefined> => {
    try {
      return await action();
    } catch (error) {
      if (!(error instanceof ApiFailure && error.code === "reauth_required")) throw error;
    }
    if (!(await ask())) return undefined;
    return action();
  }, [ask]);

  const finish = (passed: boolean) => {
    setOpened(false);
    setPassword("");
    waiting.current?.(passed);
    waiting.current = null;
  };

  const submit = async () => {
    setPending(true);
    setProblem(null);
    try {
      await adminApi.reauth(password);
      finish(true);
    } catch (error) {
      setProblem(failureMessage(error));
    } finally {
      setPending(false);
    }
  };

  return (
    <ReauthContext.Provider value={{ confirmed }}>
      {children}
      <Modal opened={opened} onClose={() => finish(false)} title="パスワードの確認" centered>
        <form onSubmit={(event) => { event.preventDefault(); void submit(); }}>
          <Stack gap="md">
            <Text size="sm">大事な操作なので、もう一度パスワードを入力してください。</Text>
            <PasswordInput label="パスワード" autoComplete="current-password" value={password} onChange={(event) => setPassword(event.currentTarget.value)} error={problem} data-autofocus />
            <Group justify="flex-end" gap={8}>
              <Button variant="default" onClick={() => finish(false)}>キャンセル</Button>
              <Button type="submit" loading={pending} disabled={!password}>確認</Button>
            </Group>
          </Stack>
        </form>
      </Modal>
    </ReauthContext.Provider>
  );
}
