import { useEffect, useState, type ReactNode } from "react";
import { ActionIcon, Alert, Anchor, Button, Code, Group, Loader, Menu, Modal, Progress, Radio, Select, SimpleGrid, Stack, Switch, Text, TextInput, Textarea, Tooltip } from "@mantine/core";
import { modals } from "@mantine/modals";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { QRCodeSVG } from "qrcode.react";
import { DANGER_CONFIRM, DangerButton } from "@/components/DangerButton";
import { errorMessage, formatNumber } from "@/lib/format";
import { Icons, type LucideIcon } from "@/lib/icons";
import { isNewer, relativeTime, windowLabel } from "@/lib/serverFormat";
import { APP_VERSION } from "@/lib/version";
import { openExternalUrl } from "@/services/openerApi";
import {
  serverAdminLink,
  serverCheckRemoteUpdate,
  serverConnectRemote,
  serverCreateReaderPairing,
  serverDisconnectRemote,
  serverEmbeddedStart,
  serverEmbeddedStop,
  serverInterfaces,
  serverRemoteDevices,
  serverRemoteStatus,
  serverRevokeDevice,
  serverSendNow,
  serverSendStop,
  serverSetAutoSend,
  serverStatus,
  serverUpdateRemote,
  type DesktopServerStatus,
  type RemoteServerStatus,
  type ServerDevice,
  type ServerPairing,
} from "@/services/serverApi";
import "@/styles/server.css";
import { SectionIntro } from "./SectionIntro";

const ANDROID_APP_URL = "https://github.com/1211snowmaple/piep-android/releases/latest";
const SERVER_DOWNLOAD_URL = "https://github.com/1211snowmaple/piep/releases/latest";
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
        description="外出先の携帯から、ライブラリを検索して読めるようにします。"
      />
      <div className="sv-stack" style={{ marginTop: 16 }}>
        {!runtime && <Alert color="blue">サーバーの操作はデスクトップアプリで利用できます。</Alert>}
        {problem && <Alert color="red" title="操作できませんでした" withCloseButton onClose={() => setProblem(null)}>{problem}</Alert>}
        {status.error && <Alert color="red" title="サーバーの状態を確認できません">{errorMessage(status.error)}</Alert>}
        {runtime && cardProps && (cardProps.status.remote
          ? <ConnectedRemote {...cardProps} />
          : cardProps.status.embeddedRunning
            ? <EmbeddedRunning {...cardProps} />
            : <ChooseServer {...cardProps} />)}
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
          <Icons.copy size={14} />
        </ActionIcon>
      </Tooltip>
    </Group>
  );
}

function Tile({ icon: Icon, title, description, next, disabled, busy, onClick }: { icon: LucideIcon; title: string; description: string; next?: boolean; disabled?: boolean; busy?: boolean; onClick: () => void }) {
  return (
    <button type="button" className="sv-card sv-tile" data-next={next || undefined} disabled={disabled} onClick={onClick}>
      <span className="sv-icon">{busy ? <Loader size={16} color="currentColor" /> : <Icon size={18} strokeWidth={1.9} aria-hidden />}</span>
      <span className="sv-tile-body">
        <span className="sv-title">{title}</span>
        <span className="sv-desc">{description}</span>
      </span>
    </button>
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
  if (sending) return { tone: "busy", headline: `${name} へ送信しています`, summary: "完了すると、携帯に反映されます。" };
  if (update && hasNewVersion(details)) {
    return {
      tone: "ok",
      headline: "新しいバージョンがあります",
      summary: update.auto
        ? update.window ? `次の ${windowLabel(update.window)} に自動で更新します。` : "夜間に自動で更新します。"
        : "自動更新は切っています。押すと更新します。",
    };
  }
  return { tone: "ok", headline: `${name} に接続しています`, summary: null };
}

function hasNewVersion(details: RemoteServerStatus): boolean {
  const update = details.update;
  if (!update?.latest) return false;
  return isNewer(update.latest, details.version) && !(update.skipped ?? []).includes(update.latest);
}

/** 次にすることを 1 つだけ示す。同時に 2 つは光らせない。 */
export function nextAction(details: RemoteServerStatus | undefined, phones: number): "send" | "phone" | "update" | null {
  if (!details) return null;
  if (details.works === null) return "send";
  if (phones === 0) return "phone";
  if (hasNewVersion(details)) return "update";
  return null;
}

function versionFact(details: RemoteServerStatus | undefined, offline: boolean): string {
  if (!details) return offline ? "読めませんでした" : "確認しています";
  const update = details.update;
  if (!update || offline) return details.version;
  if (hasNewVersion(details)) return `${details.version}（${update.latest} があります）`;
  if (update.latest && update.lastCheckedAt) return `${details.version}・最新・${relativeTime(update.lastCheckedAt)}に確認`;
  return `${details.version}・まだ確認していません`;
}

function phonesFact(phones: ServerDevice[]): string {
  if (phones.length === 0) return "まだありません";
  const latest = phones.map((phone) => phone.lastSeenAt).filter((seen): seen is string => Boolean(seen)).sort().pop();
  return `${phones.length} 台・${latest ? `最後の接続 ${relativeTime(latest)}` : "まだ接続していません"}`;
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
  const [adding, setAdding] = useState(false);
  const details = useQuery({
    queryKey: [REMOTE_STATUS_KEY, remote?.url],
    queryFn: serverRemoteStatus,
    enabled: Boolean(remote),
    refetchInterval: (query) => (query.state.data?.update?.phase.state === "applying" ? 2_000 : 15_000),
    retry: false,
  });
  const remoteDevices = useQuery({
    queryKey: ["server-remote-devices", remote?.url],
    queryFn: serverRemoteDevices,
    enabled: adding,
    refetchInterval: 3_000,
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
  const phones = (data?.devices ?? []).filter((device) => device.role === "reader");
  const next = offline ? null : nextAction(data, phones.length);
  const update = data?.update;
  const fresh = data ? hasNewVersion(data) : false;
  const applying = update?.phase.state === "applying";
  const fraction = send.bytesTotal > 0 ? Math.min(100, Math.floor((send.bytesDone / send.bytesTotal) * 100)) : 0;
  const sendTitle = send.phase === "upload" ? `送っています ${fraction}%` : SEND_PHASE[send.phase] ?? "送っています";
  const lastError = last.error && last.error !== connectionError ? last.error : null;

  const notes = [
    data && isNewer(data.version, APP_VERSION) ? `サーバーの方が新しいバージョンです（${data.version}）。送れないときは、このアプリを更新してください。` : null,
    connectionError && !registrationLost && !connectionError.includes("接続できません") ? connectionError : null,
    lastError,
    update?.lastError ? `更新できませんでした: ${update.lastError}` : null,
    data?.sync?.indexError ? `検索の索引を更新できません: ${data.sync.indexError}` : null,
    last.missingReferenced ? `ライブラリが参照しているファイルのうち ${formatNumber(last.missingReferenced)} 件がサーバーにありません。この PC からも失われているかもしれません。診断で確かめてください。` : null,
    send.outsidePaths > 0 ? `ライブラリの保存先の外にあるファイル ${formatNumber(send.outsidePaths)} 件は送っていません。` : null,
    send.skippedNames > 0 ? `ファイル名を扱えないファイル ${formatNumber(send.skippedNames)} 件は送っていません。` : null,
  ].filter((note): note is string => Boolean(note));

  const checkOrUpdate = () => {
    if (fresh) return void perform(serverUpdateRemote, "update");
    // 管理画面のないサーバーには確認だけの口がない。頼めば確かめて当ててくれる。
    return void perform(data?.adminWeb ? async () => { await serverCheckRemoteUpdate(); } : serverUpdateRemote, "update");
  };
  const openAdmin = () => {
    if (data?.adminWeb !== "available") return void openExternalUrl(`${SERVER_GUIDE_URL}#管理画面`);
    return void perform(async () => { await openExternalUrl(await serverAdminLink()); }, "admin");
  };
  const confirmDisconnect = () => modals.openConfirmModal({
    title: `この PC と ${name} の接続を解除しますか？`,
    children: (
      <Stack gap="xs">
        <Text size="sm">消えるのは、この PC に保存した鍵だけです。自動で送る設定も切ります。サーバーのライブラリと携帯の登録はそのまま残ります。</Text>
        <Text size="sm">接続し直すときは、管理画面の「端末」にある「別の PC へ入れ替える」を使います。管理画面を使えないときは、サーバーで <Code>piep-server admin-pair --replace</Code> を実行します。</Text>
      </Stack>
    ),
    labels: { confirm: "接続を解除", cancel: "キャンセル" },
    confirmProps: DANGER_CONFIRM,
    onConfirm: () => void perform(serverDisconnectRemote),
  });

  return (
    <>
      <section className="sv-card sv-status" aria-label="サーバーの状況">
        <div className="sv-top">
          <div>
            <Group gap={10} wrap="nowrap">
              <span className="sv-dot" data-tone={view.tone} aria-hidden />
              <div className="sv-head">{view.headline}</div>
            </Group>
            {view.summary && <Text size="sm" c="dimmed" mt={6}>{view.summary}</Text>}
          </div>
          {offline && !registrationLost && !applying && (
            <Button size="xs" variant="default" style={{ flex: "none" }} loading={details.isFetching} onClick={() => void details.refetch()}>もう一度確認</Button>
          )}
        </div>
        {registrationLost && <Reconnect pending={pending} active={active} perform={perform} />}
        <div className="sv-facts">
          <Fact label="サーバー">{name}</Fact>
          <Fact label="URL"><CopyValue value={remote?.url ?? ""} label="URL をコピー">{remote?.url}</CopyValue></Fact>
          <Fact label="バージョン">{versionFact(data, offline)}</Fact>
          <Fact label="ライブラリ">
            {!data ? (offline ? "読めませんでした" : "確認しています") : data.works === null ? "まだ送っていません" : `${formatNumber(data.works)} 作品`}
            {data?.sync?.indexPending ? `・検索を準備中（残り ${formatNumber(data.sync.indexPending)} 件）` : ""}
          </Fact>
          <Fact label="前回の送信">{last.finishedAt ? `${relativeTime(last.finishedAt)}${last.error ? "・送れませんでした" : ""}` : "なし"}</Fact>
          <Fact label="携帯">{data ? phonesFact(phones) : offline ? "読めませんでした" : "確認しています"}</Fact>
          <Fact label="自動">
            ライブラリを送る {status.autoSend ? "入" : "切"}
            {update ? `・更新 ${update.auto ? update.window ? windowLabel(update.window) : "入" : "切"}` : ""}
          </Fact>
        </div>
        {notes.length > 0 && (
          <Stack gap={4} mt="md">
            {notes.map((note) => <Text key={note} size="sm" style={{ overflowWrap: "anywhere" }}>{note}</Text>)}
          </Stack>
        )}
      </section>

      <div className="sv-tiles">
        {send.running ? (
          <div className="sv-card sv-tile">
            <span className="sv-icon"><Icons.send size={18} strokeWidth={1.9} aria-hidden /></span>
            <span className="sv-tile-body">
              <span className="sv-title">{sendTitle}</span>
              <span className="sv-tile-progress">
                <Progress size="sm" value={send.phase === "upload" ? fraction : 0} style={{ flex: 1 }} aria-label="送信の進み具合" />
                <Tooltip label="送り終えた分はサーバーに残り、次回はその続きから送ります">
                  <DangerButton size="compact-xs" onClick={() => void perform(serverSendStop)}>中止</DangerButton>
                </Tooltip>
              </span>
            </span>
          </div>
        ) : (
          <Tile icon={Icons.send} title="今すぐ送る" description="変更をサーバーへ送ります" next={next === "send"} disabled={pending || offline} busy={active === "send"} onClick={() => void perform(serverSendNow, "send")} />
        )}
        <Tile
          icon={Icons.updates}
          title={applying ? "更新しています" : fresh ? `${update?.latest} に更新` : "更新を確認"}
          description={applying || fresh ? "数十秒ほど止まります" : update === null ? "このサーバーは自分では更新しません" : "最新かどうかを確かめます"}
          next={next === "update"}
          disabled={pending || offline || applying || update === null}
          busy={active === "update" || applying}
          onClick={checkOrUpdate}
        />
        <Tile icon={Icons.phone} title="携帯を追加" description="QR を読むと登録できます" next={next === "phone"} disabled={pending || offline} onClick={() => setAdding(true)} />
      </div>

      <div className="sv-card sv-bar">
        <Switch
          label="自動で送る"
          description="変更して 5 分後に送ります"
          checked={status.autoSend}
          disabled={pending}
          onChange={(event) => { const enabled = event.currentTarget.checked; void perform(() => serverSetAutoSend(enabled)); }}
        />
        <Group gap={8} wrap="nowrap" style={{ flex: "none" }}>
          <Button size="xs" variant="default" disabled={pending || offline || !data?.adminWeb} loading={active === "admin"} rightSection={<Icons.externalLink size={14} />} onClick={openAdmin}>管理画面を開く</Button>
          <Menu position="bottom-end" width={240}>
            <Menu.Target><ActionIcon size={30} variant="default" aria-label="その他"><Icons.more size={16} /></ActionIcon></Menu.Target>
            <Menu.Dropdown>
              <Menu.Item color="red" disabled={pending} onClick={confirmDisconnect}>この PC との接続を解除</Menu.Item>
            </Menu.Dropdown>
          </Menu>
        </Group>
      </div>

      <Modal opened={adding} onClose={() => setAdding(false)} title="携帯を追加" centered>
        <AddPhoneSteps devices={remoteDevices.data ?? []} ready={remoteDevices.isSuccess} onClose={() => setAdding(false)} />
      </Modal>
    </>
  );
}

/** まだ何も動いていないとき。どちらを選ぶかを先に決めてもらい、選んだ方だけを見せる。 */
function ChooseServer(props: CardProps) {
  const [place, setPlace] = useState("remote");
  return (
    <>
      <Radio.Group value={place} onChange={setPlace} label="サーバーをどこに置きますか">
        <SimpleGrid cols={{ base: 1, sm: 2 }} spacing="sm" mt="xs">
          <PlaceOption value="remote" title="いつも動いている PC" description="この PC を閉じていても読めます。" />
          <PlaceOption value="here" title="この PC" description="準備は要りません。読めるのは、この PC が起動している間だけです。" />
        </SimpleGrid>
      </Radio.Group>
      {place === "remote" ? <ConnectRemoteCard {...props} /> : <EmbeddedSetup {...props} />}
    </>
  );
}

function PlaceOption({ value, title, description }: { value: string; title: string; description: string }) {
  return (
    <Radio.Card value={value} radius="md" p="md" withBorder aria-label={title}>
      <Group wrap="nowrap" align="flex-start" gap="sm">
        <Radio.Indicator />
        <div>
          <Text size="sm" fw={700}>{title}</Text>
          <Text size="xs" c="dimmed" mt={2}>{description}</Text>
        </div>
      </Group>
    </Radio.Card>
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
              <ExternalLink url={SERVER_DOWNLOAD_URL}>ダウンロード</ExternalLink>
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

const CONNECTION_HELP: Record<"lan" | "proxy", string> = {
  lan: "携帯からこの PC に届くネットワークで使えます。通信は自己署名の証明書で暗号化します。",
  proxy: "https の公開 URL で、どこからでも接続できます。転送の仕組みは自分で用意します。",
};

function EmbeddedSetup({ status, pending, active, perform }: CardProps) {
  const interfaces = useQuery({ queryKey: ["server-interfaces"], queryFn: serverInterfaces });
  const [ip, setIp] = useState<string | null>(status.embedded.ip);
  const [mode, setMode] = useState<"lan" | "proxy">(status.embedded.behindProxy ? "proxy" : "lan");
  const [publicUrl, setPublicUrl] = useState(status.embedded.publicUrl ?? "");
  useEffect(() => {
    if (!ip && interfaces.data?.length) setIp(interfaces.data[0].ip);
  }, [interfaces.data, ip]);
  const needsIp = mode === "lan";
  const needsUrl = mode === "proxy";
  return (
    <section className="sv-card sv-status" aria-label="この PC をサーバーにする">
      <Stack gap="md">
        <Select
          label="携帯からの接続方法"
          description={CONNECTION_HELP[mode]}
          data={[
            { value: "lan", label: "この PC に直接接続する" },
            { value: "proxy", label: "トンネルやリバースプロキシを経由する" },
          ]}
          value={mode}
          allowDeselect={false}
          onChange={(value) => setMode(value === "proxy" ? "proxy" : "lan")}
        />
        {needsIp && (
          <Select
            label="使用するネットワーク"
            data={(interfaces.data ?? []).map((item) => ({ value: item.ip, label: `${item.name} · ${item.ip}` }))}
            value={ip}
            onChange={setIp}
            placeholder="ネットワークを選択"
            searchable
          />
        )}
        {needsUrl && (
          <TextInput
            label="公開 URL"
            description="携帯が接続する https の URL です。この PC の 127.0.0.1:38777 へ転送するように設定してください。"
            placeholder="https://example.com/"
            value={publicUrl}
            onChange={(event) => setPublicUrl(event.currentTarget.value)}
          />
        )}
        {interfaces.error && <Text size="sm" c="red">{errorMessage(interfaces.error)}</Text>}
        <Group justify="flex-end">
          <Button
            disabled={pending || (needsIp ? !ip : !publicUrl.trim().startsWith("https://"))}
            loading={active === "start"}
            onClick={() => void perform(() => serverEmbeddedStart({
              enabled: true,
              ip: needsIp ? ip : null,
              behindProxy: needsUrl,
              publicUrl: needsUrl ? publicUrl.trim() : null,
            }), "start")}
          >開始</Button>
        </Group>
      </Stack>
    </section>
  );
}

function EmbeddedRunning({ status, pending, perform }: CardProps) {
  const [adding, setAdding] = useState(false);
  const phones = status.devices.filter((device) => device.role === "reader");
  return (
    <>
      <section className="sv-card sv-status" aria-label="サーバーの状況">
        <div className="sv-top">
          <div>
            <Group gap={10} wrap="nowrap">
              <span className="sv-dot" data-tone="ok" aria-hidden />
              <div className="sv-head">この PC で動いています</div>
            </Group>
            <Text size="sm" c="dimmed" mt={6}>この PC が起動している間だけ、携帯から読めます。</Text>
          </div>
          <Menu position="bottom-end" width={200}>
            <Menu.Target><ActionIcon size={30} variant="default" aria-label="その他" style={{ flex: "none" }}><Icons.more size={16} /></ActionIcon></Menu.Target>
            <Menu.Dropdown>
              <Menu.Item color="red" disabled={pending} onClick={() => void perform(serverEmbeddedStop, "stop")}>サーバーを停止</Menu.Item>
            </Menu.Dropdown>
          </Menu>
        </div>
        <div className="sv-facts">
          <Fact label="URL"><CopyValue value={status.embeddedUrl ?? ""} label="URL をコピー">{status.embeddedUrl}</CopyValue></Fact>
          {status.embeddedFingerprint && <Fact label="証明書">{status.embeddedFingerprint}</Fact>}
          <Fact label="携帯">{phonesFact(phones)}</Fact>
        </div>
      </section>
      <section className="sv-card sv-status" aria-label="登録している端末">
        <div className="sv-top" style={{ alignItems: "center" }}>
          <Text fw={650}>登録している携帯</Text>
          <Button size="xs" variant="default" leftSection={<Icons.add size={14} />} style={{ flex: "none" }} onClick={() => setAdding(true)}>携帯を追加</Button>
        </div>
        {phones.length === 0 && <Text size="sm" c="dimmed" mt="sm">まだありません。</Text>}
        {phones.map((device) => (
          <Group key={device.id} justify="space-between" wrap="nowrap" mt="sm" gap="md">
            <div style={{ minWidth: 0 }}>
              <Text size="sm" fw={600} style={{ overflowWrap: "anywhere" }}>{device.name || "Android 端末"}</Text>
              <Text size="xs" c="dimmed">最後の接続 {relativeTime(device.lastSeenAt)}・登録 {relativeTime(device.createdAt)}</Text>
            </div>
            <Menu position="bottom-end" width={200}>
              <Menu.Target><ActionIcon size={28} variant="subtle" color="gray" aria-label={`${device.name || "Android 端末"}の操作`} style={{ flex: "none" }}><Icons.more size={16} /></ActionIcon></Menu.Target>
              <Menu.Dropdown>
                <Menu.Item
                  color="red"
                  disabled={pending}
                  onClick={() => modals.openConfirmModal({
                    title: `「${device.name || "Android 端末"}」の登録を解除しますか？`,
                    children: <Text size="sm">解除すると、この携帯からライブラリを読めなくなります。解除はすぐに反映されます。</Text>,
                    labels: { confirm: "登録を解除", cancel: "キャンセル" },
                    confirmProps: DANGER_CONFIRM,
                    onConfirm: () => void perform(() => serverRevokeDevice(device.id)),
                  })}
                >登録を解除</Menu.Item>
              </Menu.Dropdown>
            </Menu>
          </Group>
        ))}
      </section>
      <Modal opened={adding} onClose={() => setAdding(false)} title="携帯を追加" centered>
        <AddPhoneSteps devices={status.devices} ready onClose={() => setAdding(false)} />
      </Modal>
    </>
  );
}

/** QR は一度に一つだけ出す。カメラが別の QR を拾わないようにするため。 */
function QrImage({ value, label }: { value: string; label: string }) {
  return (
    <div style={{ alignSelf: "center", padding: 16, background: "#fff", borderRadius: 8, lineHeight: 0 }}>
      <QRCodeSVG value={value} size={264} level="M" aria-label={label} />
    </div>
  );
}

function AddPhoneSteps({ devices, ready, onClose }: { devices: ServerDevice[]; ready: boolean; onClose: () => void }) {
  // 開いた時点の一覧を覚えておき、増えた 1 台を「登録しました」と知らせる。
  const [known, setKnown] = useState<Set<string> | null>(() => (ready ? new Set(devices.map((device) => device.id)) : null));
  useEffect(() => {
    if (ready && known === null) setKnown(new Set(devices.map((device) => device.id)));
  }, [ready, known, devices]);
  const [step, setStep] = useState<"app" | "pairing">("app");
  const [pairing, setPairing] = useState<ServerPairing | null>(null);
  const [issuing, setIssuing] = useState(false);
  const [expired, setExpired] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!pairing) return;
    const timer = window.setTimeout(() => { setPairing(null); setExpired(true); }, pairing.expiresInSeconds * 1_000);
    return () => window.clearTimeout(timer);
  }, [pairing]);

  // 登録の鍵の有効期限は発行した時から進むので、登録の手順に入るまで発行しない。
  const issue = async () => {
    setIssuing(true);
    setProblem(null);
    setCopied(false);
    setExpired(false);
    try {
      setPairing(await serverCreateReaderPairing());
    } catch (error) {
      setProblem(errorMessage(error));
    } finally {
      setIssuing(false);
    }
  };

  const added = known ? devices.find((device) => !known.has(device.id)) : undefined;
  if (added) {
    return (
      <Stack gap="md">
        <Text size="sm">「{added.name || "Android 端末"}」を登録しました。</Text>
        <Group justify="flex-end"><Button onClick={onClose}>閉じる</Button></Group>
      </Stack>
    );
  }
  if (step === "app") {
    return (
      <Stack gap="md">
        <Text fw={600} size="sm">手順 1 / 2　アプリを入れる</Text>
        <Text size="sm">携帯のカメラでこの QR を読むと、Android 版の配布ページが開きます。APK をダウンロードして入れてください。</Text>
        <QrImage value={ANDROID_APP_URL} label="Android 版の配布ページ" />
        <Text size="xs" c="dimmed">すでに入っている場合は、そのまま次へ進んでください。</Text>
        <Group justify="flex-end">
          <Button onClick={() => { setStep("pairing"); if (!pairing) void issue(); }}>次へ</Button>
        </Group>
      </Stack>
    );
  }
  return (
    <Stack gap="md">
      <Text fw={600} size="sm">手順 2 / 2　サーバーに登録する</Text>
      <Text size="sm">携帯のアプリを開き、「設定 → サーバーとの接続 → QR を読む」でこの QR を読み取ってください。</Text>
      {problem && <Alert color="red" title="QR を作成できませんでした">{problem}</Alert>}
      {pairing ? (
        <>
          <QrImage value={pairing.payload} label="登録用の QR" />
          <Text size="xs" c="dimmed">{Math.round(pairing.expiresInSeconds / 60)} 分間有効で、一度だけ使えます。</Text>
          <Group gap="xs">
            <Button
              size="xs"
              variant="default"
              onClick={() => void navigator.clipboard.writeText(pairing.payload).then(() => setCopied(true)).catch(() => setProblem("登録文字列をコピーできませんでした"))}
            >{copied ? "コピーしました" : "登録文字列をコピー"}</Button>
          </Group>
          <Text size="xs" c="dimmed">QR を読めないときは、コピーした文字列を携帯に貼り付けてください。この文字列を知っている人はライブラリを読めるので、人に見せないでください。</Text>
        </>
      ) : (
        <>
          {expired && <Text size="sm" c="dimmed">QR の有効期限が切れました。</Text>}
          {(expired || problem) && <Group><Button variant="default" loading={issuing} onClick={() => void issue()}>QR を作り直す</Button></Group>}
          {issuing && !expired && !problem && <Text size="sm" c="dimmed">QR を作成しています</Text>}
        </>
      )}
      <Group justify="space-between">
        <Button variant="subtle" onClick={() => setStep("app")}>戻る</Button>
        <Button variant="default" onClick={onClose}>閉じる</Button>
      </Group>
    </Stack>
  );
}
