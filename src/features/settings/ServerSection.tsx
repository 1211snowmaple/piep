import { useEffect, useState, type ReactNode } from "react";
import { ActionIcon, Alert, Anchor, Button, Code, Group, Menu, Progress, Stack, Switch, Text, Textarea, Tooltip } from "@mantine/core";
import { modals } from "@mantine/modals";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { DANGER_CONFIRM, DangerButton } from "@/components/DangerButton";
import { errorMessage, formatNumber } from "@/lib/format";
import { Icons, IconSize } from "@/lib/icons";
import { isNewer, relativeTime, windowLabel } from "@/lib/serverFormat";
import { APP_VERSION } from "@/lib/version";
import { openExternalUrl } from "@/services/openerApi";
import {
  serverAdminLink,
  serverConnectRemote,
  serverDisconnectRemote,
  serverRemoteStatus,
  serverSendNow,
  serverSendStop,
  serverSetAutoSend,
  serverStatus,
  type DesktopServerStatus,
  type RemoteServerStatus,
} from "@/services/serverApi";
import "@/styles/server.css";
import { SectionIntro } from "./SectionIntro";

// 単独サーバーも Android 版も、デスクトップ版と同じリリースに載る。
const RELEASES_URL = "https://github.com/1211snowmaple/piep/releases/latest";
const SERVER_GUIDE_URL = "https://1211snowmaple.github.io/piep/guide/05-server";
const STATUS_KEY = ["server-status"];
const REMOTE_STATUS_KEY = "server-remote-status";

/** サーバーが 401 を返したときの文言（`sync_send.rs`）。登録文字列でしか直らない。 */
const REGISTRATION_LOST = "登録が残っていません";

/** 送信の段どりを、そのまま画面の言葉にする。 */
const SEND_PHASE: Record<string, string> = {
  snapshot: "準備しています",
  manifest: "差分を確認しています",
  upload: "送っています",
  database: "データを送っています",
  commit: "反映しています",
  assemble: "サーバーで確かめています",
};

type Perform = (action: () => Promise<DesktopServerStatus | void>, name?: string) => Promise<void>;

interface CardProps {
  status: DesktopServerStatus;
  /** 何かの操作が走っている間は、ほかの操作を止める。 */
  pending: boolean;
  /** 走っている操作の名前。そのボタンだけ回す。 */
  active: string | null;
  perform: Perform;
}

export function ServerSection({ runtime }: { runtime: boolean }) {
  const queryClient = useQueryClient();
  const status = useQuery({
    queryKey: STATUS_KEY,
    queryFn: serverStatus,
    enabled: runtime,
    refetchInterval: (query) => (query.state.data?.send.running ? 1_000 : 5_000),
  });
  const [problem, setProblem] = useState<string | null>(null);
  const [active, setActive] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  const perform: Perform = async (action, name) => {
    setPending(true);
    setActive(name ?? null);
    setProblem(null);
    try {
      const next = await action();
      if (next) queryClient.setQueryData(STATUS_KEY, next);
      await status.refetch();
      await queryClient.invalidateQueries({ queryKey: [REMOTE_STATUS_KEY] });
    } catch (error) {
      setProblem(errorMessage(error));
    } finally {
      setPending(false);
      setActive(null);
    }
  };

  const data = status.data;
  const cardProps = data && { status: data, pending, active, perform };
  return (
    <div className="sv">
      <SectionIntro
        beta
        title="サーバー"
        description="この PC のライブラリをサーバーに送ります。"
      />
      <div className="sv-stack" style={{ marginTop: 16 }}>
        {!runtime && <Alert color="blue">サーバーへの接続と送信はデスクトップアプリで利用できます。</Alert>}
        {problem && <Alert color="red" title="操作できませんでした" withCloseButton onClose={() => setProblem(null)}>{problem}</Alert>}
        {status.error && <Alert color="red" title="サーバーの状態を確認できません">{errorMessage(status.error)}</Alert>}
        {runtime && cardProps && (cardProps.status.remote
          ? <ConnectedRemote {...cardProps} />
          : <ConnectRemoteCard {...cardProps} />)}
      </div>
    </div>
  );
}

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <Text size="sm" c="dimmed">{label}</Text>
      <Text size="sm" component="div" style={{ minWidth: 0, overflowWrap: "anywhere" }}>{children}</Text>
    </>
  );
}

function CopyValue({ value, label, children }: { value: string; label: string; children: ReactNode }) {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), 1_500);
    return () => window.clearTimeout(timer);
  }, [copied]);
  return (
    <Group gap={2} wrap="nowrap" align="center">
      <span style={{ overflowWrap: "anywhere", minWidth: 0 }}>{children}</span>
      <Tooltip label={copied ? "コピーしました" : label}>
        <ActionIcon
          variant="subtle"
          color="gray"
          size="sm"
          className="button-plain"
          aria-label={label}
          onClick={() => void navigator.clipboard.writeText(value).then(() => setCopied(true)).catch(() => undefined)}
        >
          <Icons.copy size={IconSize.menu} />
        </ActionIcon>
      </Tooltip>
    </Group>
  );
}

function hostOf(url: string | undefined): string {
  if (!url) return "サーバー";
  try {
    return new URL(url).hostname;
  } catch {
    return url;
  }
}

interface RemoteView {
  tone: "ok" | "busy" | "bad";
  headline: string;
  summary: string | null;
}

/** 状況の見出し。確かめたことだけを書く。接続しているだけでは、携帯から読めるとは言えない。 */
export function remoteView({ name, details, offline, registrationLost, sending, checkedAt }: {
  name: string;
  details: RemoteServerStatus | undefined;
  offline: boolean;
  registrationLost: boolean;
  sending: boolean;
  checkedAt: number;
}): RemoteView {
  const update = details?.update;
  const applying = update?.phase.state === "applying" ? update.phase.version ?? "" : null;
  if (registrationLost) {
    return { tone: "bad", headline: `${name} の登録が残っていません`, summary: "もう一度登録すると、今の設定のまま続けられます。" };
  }
  if (applying !== null) {
    return { tone: "busy", headline: `${applying} に更新しています`, summary: "数十秒ほど止まります。" };
  }
  if (offline) {
    return {
      tone: "bad",
      headline: `${name} に接続できません`,
      summary: `サーバーが止まっているか、ネットワークに届いていません。${details ? `下は ${relativeTime(checkedAt)} の内容です。` : ""}`,
    };
  }
  if (!details) return { tone: "busy", headline: `${name} を確認しています`, summary: null };
  if (sending) return { tone: "busy", headline: `${name} へ送信しています`, summary: null };
  if (update && hasNewVersion(details)) {
    return {
      tone: "ok",
      headline: "新しいバージョンがあります",
      summary: update.auto
        ? update.window ? `次の ${windowLabel(update.window)} に自動で更新します。` : "夜間に自動で更新します。"
        : "管理画面の「更新」から更新できます。",
    };
  }
  return { tone: "ok", headline: `${name} に接続しています`, summary: null };
}

function hasNewVersion(details: RemoteServerStatus): boolean {
  const update = details.update;
  if (!update?.latest) return false;
  return isNewer(update.latest, details.version) && !(update.skipped ?? []).includes(update.latest);
}

function versionFact(details: RemoteServerStatus | undefined, offline: boolean): string {
  if (!details) return offline ? "読めませんでした" : "確認しています";
  const update = details.update;
  if (!update || offline) return details.version;
  if (hasNewVersion(details)) return `${details.version}（${update.latest} があります）`;
  if (update.latest && update.lastCheckedAt) return `${details.version}・最新・${relativeTime(update.lastCheckedAt)}に確認`;
  return `${details.version}・まだ確認していません`;
}

function Reconnect({ pending, active, perform }: Pick<CardProps, "pending" | "active" | "perform">) {
  const [text, setText] = useState("");
  return (
    <Stack gap="xs" mt="md">
      <Text size="sm">サーバーで <Code>piep-server admin-pair --replace</Code> を実行し、表示された登録文字列を貼り付けてください。</Text>
      <Textarea aria-label="新しい登録文字列" placeholder="登録文字列を貼り付け" autosize minRows={2} maxRows={5} value={text} onChange={(event) => setText(event.currentTarget.value)} />
      <Group justify="flex-end">
        <Button size="xs" disabled={!text.trim() || pending} loading={active === "reconnect"} onClick={() => void perform(async () => { const next = await serverConnectRemote(text); setText(""); return next; }, "reconnect")}>登録し直す</Button>
      </Group>
    </Stack>
  );
}

function ConnectedRemote(props: CardProps) {
  const { status, pending, active, perform } = props;
  const remote = status.remote;
  const details = useQuery({
    queryKey: [REMOTE_STATUS_KEY, remote?.url],
    queryFn: serverRemoteStatus,
    enabled: Boolean(remote),
    refetchInterval: (query) => (query.state.data?.update?.phase.state === "applying" ? 2_000 : 15_000),
    retry: false,
  });
  const { refetch: refetchDetails } = details;
  const lastFinished = status.lastSend.finishedAt;
  useEffect(() => {
    if (lastFinished) void refetchDetails();
  }, [lastFinished, refetchDetails]);

  const connectionError = details.error ? errorMessage(details.error) : null;
  const offline = Boolean(connectionError);
  const registrationLost = connectionError?.includes(REGISTRATION_LOST) ?? false;
  const data = details.data;
  const name = data?.serverName ?? hostOf(remote?.url);
  const send = status.send;
  const last = status.lastSend;
  const view = remoteView({ name, details: data, offline, registrationLost, sending: send.running, checkedAt: details.dataUpdatedAt });
  const update = data?.update;
  const applying = update?.phase.state === "applying";
  const fraction = send.bytesTotal > 0 ? Math.min(100, Math.floor((send.bytesDone / send.bytesTotal) * 100)) : 0;
  const sendTitle = send.phase === "upload" ? `送っています ${fraction}%` : send.message ?? SEND_PHASE[send.phase] ?? "送っています";
  const lastError = last.error && last.error !== connectionError ? last.error : null;

  const notes = [
    data && isNewer(data.version, APP_VERSION) ? `サーバーの方が新しいバージョンです（${data.version}）。送れないときは、このアプリを更新してください。` : null,
    connectionError && !registrationLost && !connectionError.includes("接続できません") ? connectionError : null,
    lastError,
    data?.shelfPending && !send.running ? "送ったライブラリは、サーバーを再起動すると読めるようになります。" : null,
  ].filter((note): note is string => Boolean(note));

  const adminAvailable = data?.adminWeb === "available";
  const openAdmin = () => {
    return void perform(async () => { await openExternalUrl(await serverAdminLink()); }, "admin");
  };
  const confirmDisconnect = () => modals.openConfirmModal({
    title: `この PC と ${name} の接続を解除しますか？`,
    children: (
      <Stack gap="xs">
        <Text size="sm">この PC に保存した接続情報を消し、自動送信を停止します。</Text>
        <Text size="sm">接続し直すときは、管理画面の「端末」にある「別の PC へ入れ替える」を使います。管理画面を使えないときは、サーバーで <Code>piep-server admin-pair --replace</Code> を実行します。</Text>
      </Stack>
    ),
    labels: { confirm: "接続を解除", cancel: "キャンセル" },
    confirmProps: DANGER_CONFIRM,
    onConfirm: () => void perform(serverDisconnectRemote),
  });

  return (
    <>
      <section className="sv-card sv-status" aria-label="接続先のサーバー">
        <div className="sv-top">
          <div>
            <Group gap={10} wrap="nowrap">
              <span className="sv-dot" data-tone={view.tone} aria-hidden />
              <div className="sv-head">{view.headline}</div>
            </Group>
            {view.summary && <Text size="sm" c="dimmed" mt={6}>{view.summary}</Text>}
          </div>
          <Group gap={8} wrap="nowrap">
            {offline && !registrationLost && !applying && (
              <Button size="xs" variant="default" loading={details.isFetching} onClick={() => void details.refetch()}>もう一度確認</Button>
            )}
            <Menu position="bottom-end" width={240}>
              <Menu.Target><ActionIcon size={30} variant="default" aria-label="接続の操作"><Icons.more size={IconSize.menu} /></ActionIcon></Menu.Target>
              <Menu.Dropdown>
                <Menu.Item color="red" disabled={pending} onClick={confirmDisconnect}>この PC との接続を解除</Menu.Item>
              </Menu.Dropdown>
            </Menu>
          </Group>
        </div>
        {registrationLost && <Reconnect pending={pending} active={active} perform={perform} />}
        <div className="sv-facts">
          <Fact label="サーバー">{name}</Fact>
          <Fact label="URL"><CopyValue value={remote?.url ?? ""} label="URL をコピー">{remote?.url}</CopyValue></Fact>
          <Fact label="バージョン">{versionFact(data, offline)}</Fact>
          <Fact label="配信中のライブラリ">
            {!data ? (offline ? "確認できませんでした" : "確認しています") : data.shelf === null ? "未配信" : `${formatNumber(Number(data.shelf.works))} 作品`}
          </Fact>
        </div>
        {notes.length > 0 && (
          <Stack gap={4} mt="md">
            {notes.map((note) => <Text key={note} size="sm" style={{ overflowWrap: "anywhere" }}>{note}</Text>)}
          </Stack>
        )}
      </section>

      <section className="sv-card sv-status" aria-labelledby="server-send-title">
        <div className="sv-action">
          <div>
            <Text id="server-send-title" component="h3" fw={650}>ライブラリの送信</Text>
            <Text size="sm" c="dimmed" role={send.running ? "status" : undefined}>
              {send.running ? sendTitle : last.finishedAt ? `前回の送信 ${relativeTime(last.finishedAt)}${last.error ? "・送れませんでした" : ""}` : "送信すると、サーバーで配信できるようになります。"}
            </Text>
          </div>
          {send.running ? (
            <Tooltip label="送り終えた分はサーバーに残り、次回はその続きから送ります">
              <DangerButton size="sm" disabled={pending} onClick={() => void perform(serverSendStop)}>中止</DangerButton>
            </Tooltip>
          ) : (
            <Button size="sm" disabled={pending || offline || applying || !data} loading={active === "send"} onClick={() => void perform(serverSendNow, "send")}>今すぐ送る</Button>
          )}
        </div>
        {send.running && <Progress mt="sm" size="sm" value={send.phase === "upload" ? fraction : 0} aria-label="送信の進み具合" />}
        <Switch
          mt="md"
          label="自動で送る"
          description="変更後、次の変更がないまま 5 分経つと送ります"
          checked={status.autoSend}
          disabled={pending}
          onChange={(event) => { const enabled = event.currentTarget.checked; void perform(() => serverSetAutoSend(enabled)); }}
        />
      </section>

      <section className="sv-card sv-status sv-action" aria-labelledby="server-admin-title">
        <div>
          <Text id="server-admin-title" component="h3" fw={650}>サーバー管理</Text>
          <Text size="sm" c="dimmed">携帯の登録と閲覧許可、サーバーの更新や設定はブラウザの管理画面で行います。</Text>
          {data?.adminWeb === "public-only" && <Text size="sm" c="dimmed" mt={4}>現在の接続先では、サーバーの PC で管理画面を開けます。</Text>}
          {data?.adminWeb === "disabled" && <Text size="sm" c="dimmed" mt={4}>管理画面はサーバーで停止されています。</Text>}
        </div>
        {adminAvailable ? (
          <Button size="sm" variant="default" disabled={pending || offline} loading={active === "admin"} rightSection={<Icons.externalLink size={IconSize.menu} />} onClick={openAdmin}>管理画面を開く</Button>
        ) : (
          <Button size="sm" variant="default" rightSection={<Icons.externalLink size={IconSize.menu} />} onClick={() => void openExternalUrl(`${SERVER_GUIDE_URL}#管理画面`)}>開き方を見る</Button>
        )}
      </section>
    </>
  );
}

function ExternalLink({ url, children }: { url: string; children: ReactNode }) {
  return <Anchor size="sm" component="button" type="button" onClick={() => void openExternalUrl(url)}>{children}</Anchor>;
}

function ConnectRemoteCard({ pending, active, perform }: CardProps) {
  const [text, setText] = useState("");
  return (
    <section className="sv-card sv-status" aria-label="サーバーに接続">
      <Stack gap="lg">
        <div className="sv-step">
          <span className="sv-step-number" aria-hidden>1</span>
          <div className="sv-step-body">
            <Text size="sm" fw={650}>サーバーを入れる</Text>
            <Text size="sm" c="dimmed" mt={2}>いつも動いている PC に piep-server を入れます。</Text>
            <Group gap="md" mt={6}>
              <ExternalLink url={SERVER_GUIDE_URL}>導入の手順</ExternalLink>
              <ExternalLink url={RELEASES_URL}>ダウンロード</ExternalLink>
            </Group>
          </div>
        </div>
        <div className="sv-step">
          <span className="sv-step-number" aria-hidden>2</span>
          <div className="sv-step-body">
            <Text size="sm" fw={650}>登録文字列を貼り付ける</Text>
            <Text size="sm" c="dimmed" mt={2} mb={8}>導入の最後に表示されます。一度だけ使えます。</Text>
            <Textarea aria-label="管理者用の登録文字列" placeholder="登録文字列を貼り付け" autosize minRows={2} maxRows={5} value={text} onChange={(event) => setText(event.currentTarget.value)} />
            <Group justify="flex-end" mt="sm">
              <Button disabled={!text.trim() || pending} loading={active === "connect"} onClick={() => void perform(async () => { const next = await serverConnectRemote(text); setText(""); return next; }, "connect")}>接続</Button>
            </Group>
          </div>
        </div>
      </Stack>
    </section>
  );
}
