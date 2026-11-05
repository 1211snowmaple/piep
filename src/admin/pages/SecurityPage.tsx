import { useEffect, useState } from "react";
import { Alert, Button, Group, PasswordInput, Skeleton, Stack, Text, TextInput } from "@mantine/core";
import { modals } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { DANGER_CONFIRM, DangerButton } from "@/components/DangerButton";
import { adminApi, failureMessage, type Connection } from "../api";
import { Card, Facts, PageHead, showFailure, useReauth, useStatus } from "../shared";

const TLS_LABEL: Record<Connection["tls"], string> = {
  none: "このサーバーの手前で受けます",
  "self-signed": "自己署名の証明書",
  files: "指定した証明書",
};

/** 危ない組み合わせと、その直し方。 */
export function connectionWarnings(connection: Connection): string[] {
  const warnings: string[] = [];
  const open = (address: string) => !address.startsWith("127.") && !address.startsWith("[::1]");
  if (connection.adminListen && open(connection.adminListen)) {
    warnings.push(`管理用のアドレス ${connection.adminListen} に、この機械の外から届きます。家の中のネットワークか VPN の中だけに置いてください。`);
  }
  if (!connection.adminListen && open(connection.listen)) {
    warnings.push(`携帯用と管理用が同じアドレス ${connection.listen} で、この機械の外から届きます。server.toml に admin_listen を書いて分けてください。`);
  }
  return warnings;
}

function NameCard() {
  const status = useStatus();
  const client = useQueryClient();
  const [name, setName] = useState("");
  useEffect(() => { if (status.data) setName(status.data.serverName); }, [status.data?.serverName]); // eslint-disable-line react-hooks/exhaustive-deps
  const save = async () => {
    try {
      await adminApi.setServerName(name);
      notifications.show({ message: "名前を変えました" });
      await client.invalidateQueries({ queryKey: ["status"] });
    } catch (error) {
      showFailure(error);
    }
  };
  return (
    <Card title="サーバーの名前">
      <form onSubmit={(event) => { event.preventDefault(); void save(); }}>
        <Group gap="sm" wrap="nowrap" align="flex-start">
          <TextInput aria-label="サーバーの名前" style={{ flex: 1, minWidth: 0, maxWidth: 360 }} value={name} maxLength={64} onChange={(event) => setName(event.currentTarget.value)} />
          <Button type="submit" variant="default" disabled={!status.data || name === status.data.serverName}>保存</Button>
        </Group>
      </form>
    </Card>
  );
}

function PasswordCard() {
  const { confirmed } = useReauth();
  const [password, setPassword] = useState("");
  const [again, setAgain] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const ready = [...password].length >= 15 && password === again;
  const submit = async () => {
    setPending(true);
    setProblem(null);
    try {
      const value = password;
      await confirmed(async () => { await adminApi.changePassword(value); return true; }).then((done) => {
        if (done) {
          setPassword("");
          setAgain("");
          notifications.show({ message: "パスワードを変えました。ほかのログインは切れました。" });
        }
      });
    } catch (error) {
      setProblem(failureMessage(error));
    } finally {
      setPending(false);
    }
  };
  return (
    <Card title="パスワードの変更">
      <form onSubmit={(event) => { event.preventDefault(); if (ready) void submit(); }}>
        <Stack gap="sm" maw={360}>
          <PasswordInput label="新しいパスワード" description="15 文字以上。文章でもかまいません。" autoComplete="new-password" value={password} onChange={(event) => setPassword(event.currentTarget.value)} />
          <PasswordInput label="もう一度" autoComplete="new-password" value={again} onChange={(event) => setAgain(event.currentTarget.value)} error={again.length > 0 && again !== password ? "上と違います" : undefined} />
          {problem && <Alert color="red" variant="light">{problem}</Alert>}
          <Group><Button type="submit" variant="default" loading={pending} disabled={!ready}>変更する</Button></Group>
        </Stack>
      </form>
    </Card>
  );
}

export function SecurityPage({ onDisabled }: { onDisabled: () => void }) {
  const { confirmed } = useReauth();
  const connection = useQuery({ queryKey: ["connection"], queryFn: adminApi.connection });
  const warnings = connection.data ? connectionWarnings(connection.data) : [];
  const disable = () => {
    modals.openConfirmModal({
      title: "管理画面を止めますか",
      children: (
        <Stack gap="xs">
          <Text size="sm">ログインはすべて切れ、この URL では開けなくなります。読むことと、デスクトップ版からの送信は続きます。</Text>
          <Text size="sm">戻すときは、サーバーで次を実行します。</Text>
          <Text className="adm-code">piep-server admin-web --data &lt;置き場所&gt; --enable</Text>
        </Stack>
      ),
      labels: { confirm: "管理画面を止める", cancel: "キャンセル" },
      confirmProps: DANGER_CONFIRM,
      onConfirm: () => {
        void confirmed(async () => { await adminApi.disableWeb(); return true; }).then((done) => { if (done) onDisabled(); }).catch(showFailure);
      },
    });
  };
  return (
    <div className="adm-section">
      <PageHead title="接続と安全" />
      {warnings.map((warning) => (
        <section key={warning} className="sv-card" style={{ padding: 16 }}>
          <Group gap={10} wrap="nowrap" align="flex-start">
            <span className="sv-dot" data-tone="bad" aria-hidden style={{ marginTop: 6 }} />
            <Text size="sm">{warning}</Text>
          </Group>
        </section>
      ))}
      <Card title="接続">
        {connection.isPending ? <Skeleton height={110} /> : !connection.data ? <Text size="sm" c="dimmed">読み取れませんでした。</Text> : (
          <Stack gap="md">
            <Facts items={[
              ["携帯用のアドレス", connection.data.listen],
              ["管理用のアドレス", connection.data.adminListen ?? "携帯用と同じ"],
              ["公開の URL", connection.data.publicUrl ?? "なし"],
              ["HTTPS", TLS_LABEL[connection.data.tls]],
            ]} />
            <Text size="xs" c="dimmed">変えるときは server.toml を書き換えて、サーバーを再起動してください。</Text>
          </Stack>
        )}
      </Card>
      <NameCard />
      <PasswordCard />
      <Card title="管理画面を止める">
        <Group justify="space-between" wrap="nowrap" gap="md">
          <Text size="sm">使わないときは止めておけます。読むことと送信は続きます。</Text>
          <DangerButton size="xs" style={{ flex: "none" }} onClick={disable}>管理画面を止める</DangerButton>
        </Group>
      </Card>
    </div>
  );
}
