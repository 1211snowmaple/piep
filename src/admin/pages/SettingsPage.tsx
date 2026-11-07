import { useEffect, useMemo, useState } from "react";
import { Alert, Anchor, Button, Group, Loader, Modal, Radio, Skeleton, Stack, Switch, Text, TextInput } from "@mantine/core";
import { modals } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { DANGER_CONFIRM, DangerButton } from "@/components/DangerButton";
import { Icons, IconSize } from "@/lib/icons";
import { adminApi, failureMessage, type Connection, type ConnectionChange, type ConnectionState, type TlsMode } from "../api";
import { adminPlace, connectionWarnings, draftConnection, sameConnection, type ConnectionDraft } from "../format";
import { Card, PageHead, showFailure, useReauth, useStatus } from "../shared";

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
    <Card title="サーバーの名前" lead="携帯のアプリと、この画面の左上に出ます。">
      <form onSubmit={(event) => { event.preventDefault(); void save(); }}>
        <Group gap="sm" wrap="nowrap" align="flex-start">
          <TextInput aria-label="サーバーの名前" style={{ flex: 1, minWidth: 0, maxWidth: 400 }} value={name} maxLength={64} onChange={(event) => setName(event.currentTarget.value)} />
          <Button type="submit" variant="default" disabled={!status.data || name === status.data.serverName}>保存</Button>
        </Group>
      </form>
    </Card>
  );
}

const TLS_CHOICES: Array<{ value: TlsMode; label: string; description: string }> = [
  { value: "none", label: "使わない", description: "サーバーの PC の中だけで使うときや、公開の URL の側で HTTPS を受けているとき。" },
  { value: "self-signed", label: "自己署名の証明書を作る", description: "家の中のネットワークで直接つなぐとき。携帯は登録のときに証明書を覚えます。" },
  { value: "files", label: "証明書のファイルを使う", description: "持っている証明書と鍵を使うとき。" },
];

/** 再起動を待つ。始まった時刻が変われば立ち上がり直している。 */
function RestartDialog({ restart, onDone }: { restart: { startedAt: string | null; expected: Connection; moveTo: string | null } | null; onDone: () => void }) {
  const client = useQueryClient();
  const [state, setState] = useState<"waiting" | "back" | "rolledBack" | "slow">("waiting");
  useEffect(() => {
    if (!restart || restart.moveTo) return;
    setState("waiting");
    let stopped = false;
    const begun = Date.now();
    const poll = async () => {
      while (!stopped) {
        await new Promise((resolve) => setTimeout(resolve, 1_500));
        if (stopped) return;
        try {
          const status = await adminApi.status();
          if (status.startedAt === restart.startedAt) continue;
          const connection = await adminApi.connection();
          client.setQueryData(["connection"], connection);
          await client.invalidateQueries();
          setState(sameConnection(connection.running, restart.expected) ? "back" : "rolledBack");
          return;
        } catch {
          // 止まっている間は届かない。
        }
        if (Date.now() - begun > 60_000) setState("slow");
      }
    };
    void poll();
    return () => { stopped = true; };
  }, [restart]); // eslint-disable-line react-hooks/exhaustive-deps
  const waiting = state === "waiting" || state === "slow";
  return (
    <Modal opened={restart !== null} onClose={onDone} withCloseButton={!waiting || Boolean(restart?.moveTo)} closeOnClickOutside={false} closeOnEscape={!waiting} title="サーバーの再起動">
      {restart?.moveTo ? (
        <Stack gap="md">
          <Text>管理画面は次の場所へ移りました。サーバーが立ち上がったら、新しい場所で開いてログインしてください。</Text>
          {restart.moveTo.includes("サーバーの PC のアドレス")
            ? <Text className="adm-code adm-move-link">{restart.moveTo}</Text>
            : <Anchor href={restart.moveTo} className="adm-code adm-move-link">{restart.moveTo}</Anchor>}
          {restart.moveTo.startsWith("http://127.0.0.1") && <Text c="dimmed" size="sm">この場所は、サーバーの PCのブラウザでだけ開けます。</Text>}
        </Stack>
      ) : waiting ? (
        <Stack gap="md" align="flex-start">
          <Group gap="sm"><Loader size="sm" /><Text>立ち上げ直しています。数秒から数十秒かかります。</Text></Group>
          {state === "slow" && <Text c="dimmed" size="sm">時間がかかっています。新しい設定で立ち上がれないときは、前の設定に戻して立ち上がります。サーバーの PCで様子を確かめてください。</Text>}
        </Stack>
      ) : state === "back" ? (
        <Stack gap="md">
          <Text>新しい設定で立ち上がりました。</Text>
          <Group justify="flex-end"><Button onClick={onDone}>閉じる</Button></Group>
        </Stack>
      ) : (
        <Stack gap="md">
          <Text>新しい設定では立ち上がれなかったので、前の設定へ戻して立ち上げました。理由は「記録」にあります。</Text>
          <Group justify="flex-end"><Button onClick={onDone}>閉じる</Button></Group>
        </Stack>
      )}
    </Modal>
  );
}

function ConnectionForm({ state }: { state: ConnectionState }) {
  const client = useQueryClient();
  const status = useStatus();
  const { confirmed } = useReauth();
  const base = state.saved ?? state.running;
  const [draft, setDraft] = useState<ConnectionDraft>(() => draftConnection(base));
  const [problem, setProblem] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [restart, setRestart] = useState<{ startedAt: string | null; expected: Connection; moveTo: string | null } | null>(null);
  useEffect(() => { setDraft(draftConnection(base)); }, [base]); // eslint-disable-line react-hooks/exhaustive-deps
  const set = (patch: Partial<ConnectionDraft>) => setDraft((current) => ({ ...current, ...patch }));
  const next = useMemo(() => toConnection(draft), [draft]);
  const place = adminPlace(next);
  // 名前で開いていることもあるので、ブラウザのアドレスではなく今の設定の場所と比べる。
  const runningPlace = adminPlace(state.running)?.url ?? null;
  const movedTo = (connection: Connection) => {
    const target = adminPlace(connection)?.url ?? null;
    return target !== runningPlace ? target : null;
  };
  const moves = (place?.url ?? null) !== runningPlace;
  const dirty = !sameDraft(draft, draftConnection(base)) || draft.cert.trim() !== "" || draft.key.trim() !== "";
  const waitingRestart = state.saved !== null && !sameConnection(state.saved, state.running);

  const change = (): ConnectionChange => ({
    listen: draft.listen.trim(),
    adminListen: draft.split ? draft.adminListen.trim() : null,
    publicUrl: draft.publicUrl.trim(),
    tls: draft.tls,
    cert: draft.tls === "files" ? draft.cert.trim() || null : null,
    key: draft.tls === "files" ? draft.key.trim() || null : null,
  });

  const restartNow = async (expected: Connection, moveTo: string | null) => {
    const startedAt = status.data?.startedAt ?? null;
    const done = await confirmed(async () => { await adminApi.restart(); return true; });
    if (done) setRestart({ startedAt, expected, moveTo });
  };

  const save = async (andRestart: boolean) => {
    setPending(true);
    setProblem(null);
    try {
      const saved = await confirmed(() => adminApi.saveConnection(change()));
      if (!saved) return;
      client.setQueryData(["connection"], saved);
      if (andRestart && saved.saved) {
        await restartNow(saved.saved, movedTo(saved.saved));
      } else {
        notifications.show({ message: "保存しました。サーバーを動かし直すと切り替わります。" });
      }
    } catch (error) {
      setProblem(failureMessage(error));
    } finally {
      setPending(false);
    }
  };

  const askSave = () => {
    if (!state.restartable && !moves) {
      void save(false);
      return;
    }
    modals.openConfirmModal({
      title: state.restartable ? "保存して再起動しますか" : "保存しますか",
      children: (
        <Stack gap="xs">
          {state.restartable && <Text size="sm">サーバーは数秒止まり、携帯の読み込みも止まります。新しい設定で立ち上がれないときは、前の設定に戻して立ち上げます。</Text>}
          {moves && <Text size="sm">{state.restartable ? "" : "サーバーを動かし直すと、"}{place ? <>管理画面は <span className="adm-code">{place.url}</span> へ移ります。</> : "管理画面は出なくなります。"}このページは開けなくなります。</Text>}
        </Stack>
      ),
      labels: { confirm: state.restartable ? "保存して再起動" : "保存", cancel: "キャンセル" },
      onConfirm: () => void save(state.restartable),
    });
  };

  return (
    <>
      {waitingRestart && (
        <Alert color="yellow" variant="light" icon={<Icons.notice size={IconSize.action} />} title="保存した設定は、まだ使われていません">
          <Group justify="space-between" gap="md">
            <Text size="sm">サーバーを再起動すると切り替わります。</Text>
            {state.restartable && state.saved && (
              <Button variant="default" onClick={() => {
                const saved = state.saved as Connection;
                void restartNow(saved, movedTo(saved)).catch(showFailure);
              }}>再起動</Button>
            )}
          </Group>
        </Alert>
      )}
      <form onSubmit={(event) => { event.preventDefault(); if (dirty) askSave(); }}>
        <Stack gap="lg">
          <div className="adm-form-grid">
            <TextInput
              label="携帯用のアドレス"
              description="携帯のアプリがつなぐアドレスです。ほかの機器からつなぐときは 0.0.0.0:38777 にして、HTTPS を選びます。"
              placeholder="127.0.0.1:38777"
              value={draft.listen}
              onChange={(event) => set({ listen: event.currentTarget.value })}
            />
            <TextInput
              label="公開の URL"
              description="インターネットから使うときの、https で始まるアドレスです。携帯の登録に使います。空のときは、デスクトップ版がつないでいるアドレスを使います。"
              placeholder="https://example.com/piep/"
              value={draft.publicUrl}
              onChange={(event) => set({ publicUrl: event.currentTarget.value })}
            />
          </div>
          <Stack gap="xs">
            <Switch
              label="管理画面のアドレスを分ける"
              description="携帯用のアドレスをインターネットに出すときは分けます。分けずに公開の URL を使うと、管理画面はサーバーの PC のブラウザでだけ開けます（127.0.0.1:38778）。"
              checked={draft.split}
              onChange={(event) => set({ split: event.currentTarget.checked })}
            />
            {draft.split && (
              <TextInput
                aria-label="管理画面のアドレス"
                placeholder="127.0.0.1:38778"
                maw={360}
                ml={54}
                value={draft.adminListen}
                onChange={(event) => set({ adminListen: event.currentTarget.value })}
              />
            )}
          </Stack>
          <Radio.Group label="HTTPS" value={draft.tls} onChange={(value) => set({ tls: value as TlsMode })}>
            <Stack gap="sm" mt="xs">
              {TLS_CHOICES.map((choice) => <Radio key={choice.value} value={choice.value} label={choice.label} description={choice.description} />)}
            </Stack>
          </Radio.Group>
          {draft.tls === "files" && (
            <div className="adm-form-grid">
              <TextInput label="証明書のファイル" placeholder={base.tls === "files" ? "今のファイルのまま" : "C:\\piep\\cert.pem"} value={draft.cert} onChange={(event) => set({ cert: event.currentTarget.value })} />
              <TextInput label="鍵のファイル" placeholder={base.tls === "files" ? "今のファイルのまま" : "C:\\piep\\key.pem"} value={draft.key} onChange={(event) => set({ key: event.currentTarget.value })} />
            </div>
          )}
          <div className="adm-place">
            <span className="adm-place-label">管理画面の場所</span>
            {place ? (
              <span>
                <span className="adm-code">{place.url}</span>
                {place.thisMachineOnly && <span className="adm-place-note">サーバーの PC のブラウザでだけ開けます</span>}
                {moves && dirty && <span className="adm-place-note" data-tone="move">再起動するとここへ移ります</span>}
              </span>
            ) : <span>出しません</span>}
          </div>
          {problem && <Alert color="red" variant="light">{problem}</Alert>}
          <Group justify="flex-start" gap="sm">
            <Button type="submit" loading={pending} disabled={!dirty}>{state.restartable ? "保存して再起動" : "保存"}</Button>
            <Button variant="default" disabled={!dirty || pending} onClick={() => { setDraft(draftConnection(base)); setProblem(null); }}>元に戻す</Button>
          </Group>
          {!state.restartable && <Text size="sm" c="dimmed">このサーバーは install で置いていないので、画面からは再起動できません。保存したあと、サーバーの PCで動かし直してください。</Text>}
        </Stack>
      </form>
      <RestartDialog restart={restart} onDone={() => setRestart(null)} />
    </>
  );
}

function toConnection(draft: ConnectionDraft): Connection {
  const adminListen = draft.split && draft.adminListen.trim() ? draft.adminListen.trim() : null;
  const publicUrl = draft.publicUrl.trim() || null;
  const listen = draft.listen.trim();
  const localAdmin = !adminListen && publicUrl && !listen.endsWith(":38778") ? "127.0.0.1:38778" : null;
  return { listen, adminListen, publicUrl, localAdmin, tls: draft.tls };
}

function sameDraft(a: ConnectionDraft, b: ConnectionDraft): boolean {
  return a.listen.trim() === b.listen.trim()
    && a.split === b.split
    && (!a.split || a.adminListen.trim() === b.adminListen.trim())
    && a.publicUrl.trim() === b.publicUrl.trim()
    && a.tls === b.tls;
}

export function SettingsPage({ onDisabled }: { onDisabled: () => void }) {
  const { confirmed } = useReauth();
  const connection = useQuery({ queryKey: ["connection"], queryFn: adminApi.connection });
  const warnings = connection.data ? connectionWarnings(connection.data.running) : [];
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
      <PageHead title="設定" lead="サーバーの名前と、携帯や管理画面からつなぐアドレスを決めます。" />
      {warnings.map((warning) => (
        <Alert key={warning} color="red" variant="light" icon={<Icons.warning size={IconSize.action} />}>{warning}</Alert>
      ))}
      <NameCard />
      <Card title="接続" lead="変えたあとは、サーバーを再起動すると切り替わります。">
        {connection.isPending ? <Skeleton height={220} /> : !connection.data ? <Text c="dimmed">読み取れませんでした。</Text> : (
          <Stack gap="lg">
            {connection.data.problem && <Alert color="red" variant="light">{connection.data.problem}</Alert>}
            <ConnectionForm state={connection.data} />
          </Stack>
        )}
      </Card>
      <Card title="管理画面を止める">
        <div className="adm-card-row">
          <Text>使わないときは止めておけます。読むことと送信は続きます。</Text>
          <DangerButton style={{ flex: "none" }} onClick={disable}>管理画面を止める</DangerButton>
        </div>
      </Card>
    </div>
  );
}
