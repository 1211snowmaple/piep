import { useEffect, useState, type ReactNode } from "react";
import { Alert, Anchor, Badge, Button, Card, Code, Divider, Group, Modal, Progress, Radio, Select, SimpleGrid, Stack, Switch, Text, TextInput, Textarea } from "@mantine/core";
import { modals } from "@mantine/modals";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { QRCodeSVG } from "qrcode.react";
import { errorMessage, formatBytes, formatNumber } from "@/lib/format";
import { openExternalUrl } from "@/services/openerApi";
import { SectionIntro } from "./SectionIntro";
import { DANGER_CONFIRM, DangerButton } from "@/components/DangerButton";
import {
  serverConnectRemote,
  serverCreateReaderPairing,
  serverDisconnectRemote,
  serverEmbeddedStart,
  serverEmbeddedStop,
  serverInterfaces,
  serverRemoteDevices,
  serverRemoteStatus,
  serverUpdateRemote,
  serverRevokeDevice,
  serverSendNow,
  serverSendStop,
  serverSetAutoSend,
  serverStatus,
  type DesktopServerStatus,
  type RemoteServerStatus,
  type RemoteServerUpdate,
  type ServerDevice,
  type ServerPairing,
} from "@/services/serverApi";

const ANDROID_APP_URL = "https://github.com/1211snowmaple/piep-android/releases/latest";
const SERVER_DOWNLOAD_URL = "https://github.com/1211snowmaple/piep/releases/latest";
const SERVER_GUIDE_URL = "https://1211snowmaple.github.io/piep/guide/05-server";
const STATUS_KEY = ["server-status"];

/** サーバーが 401 を返したときの文言（`sync_send.rs`）。登録文字列でしか直らない。 */
const REGISTRATION_LOST = "登録が残っていません";

/** 送信の段どりを、そのまま画面の言葉にする。 */
const SEND_PHASE: Record<string, string> = {
  snapshot: "本棚のコピーを作成しています",
  manifest: "サーバーとの差分を確認しています",
  upload: "ファイルを送信しています",
  database: "本棚のデータを送信しています",
  commit: "サーバーに反映しています",
};

function formatTime(value: string | null | undefined): string {
  return value ? new Date(value).toLocaleString("ja-JP", { dateStyle: "short", timeStyle: "short" }) : "—";
}

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
    <Stack gap="md">
      <SectionIntro
        beta
        title="サーバー"
        description="携帯から、外出先でも本棚を検索して読めるようにします。"
      />
      <Alert color="yellow" title="サーバーはβ版です">
        <Text size="sm">携帯との通信方法や保存の形式は、予告して変更することがあります。</Text>
      </Alert>
      {!runtime && <Alert color="blue">サーバーの操作はデスクトップアプリで利用できます。</Alert>}
      {problem && <Alert color="red" title="操作できませんでした">{problem}</Alert>}
      {status.error && <Alert color="red" title="サーバーの状態を確認できません">{errorMessage(status.error)}</Alert>}
      {runtime && cardProps && (cardProps.status.remote
        ? <ConnectedRemote {...cardProps} />
        : cardProps.status.embeddedRunning
          ? <><EmbeddedCard {...cardProps} /><DevicesCard {...cardProps} /></>
          : <ChooseServer {...cardProps} />)}
    </Stack>
  );
}

/** まだ何も動いていないとき。どちらを選ぶかを先に決めてもらい、選んだ方だけを見せる。 */
function ChooseServer(props: CardProps) {
  const [place, setPlace] = useState("remote");
  return (
    <Stack gap="md">
      <Radio.Group value={place} onChange={setPlace} label="どこでサーバーを動かしますか">
        <SimpleGrid cols={{ base: 1, sm: 2 }} spacing="sm" mt="xs">
          <PlaceOption value="remote" title="別の PC" description="常に動いている PC に置きます。この PC を閉じていても読めます。" />
          <PlaceOption value="here" title="この PC" description="準備は要りません。読めるのは、この PC が起動している間だけです。" />
        </SimpleGrid>
      </Radio.Group>
      {place === "remote" ? <ConnectRemoteCard {...props} /> : <EmbeddedCard {...props} />}
    </Stack>
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
    <Card withBorder p="lg">
      <Stack gap="sm">
        <Text fw={700}>別の PC のサーバーに接続</Text>
        <Text size="sm">1. サーバーにする PC に piep-server を入れます。</Text>
        <Group gap="md" pl="md">
          <ExternalLink url={SERVER_DOWNLOAD_URL}>ダウンロード</ExternalLink>
          <ExternalLink url={SERVER_GUIDE_URL}>導入の手順</ExternalLink>
        </Group>
        <Text size="sm">2. 導入の最後に表示される、管理者用の登録文字列を貼り付けます。一度だけ使えます。</Text>
        <Textarea aria-label="管理者用の登録文字列" placeholder="登録文字列を貼り付け" autosize minRows={2} maxRows={5} value={text} onChange={(event) => setText(event.currentTarget.value)} />
        <Group justify="flex-end">
          <Button disabled={!text.trim() || pending} loading={active === "connect"} onClick={() => void perform(async () => { const next = await serverConnectRemote(text); setText(""); return next; }, "connect")}>接続</Button>
        </Group>
      </Stack>
    </Card>
  );
}

function ConnectedRemote(props: CardProps) {
  const remote = props.status.remote;
  const details = useQuery({
    queryKey: ["server-remote-status", remote?.url, props.status.lastSend.finishedAt],
    queryFn: serverRemoteStatus,
    enabled: Boolean(remote),
    refetchInterval: 15_000,
    retry: false,
  });
  const connectionError = details.error ? errorMessage(details.error) : null;
  return (
    <>
      <RemoteCard {...props} details={details.data} connectionError={connectionError} />
      <DevicesCard {...props} connectionError={connectionError} />
      <SendCard {...props} connectionError={connectionError} />
    </>
  );
}

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div>
      <Text size="xs" c="dimmed">{label}</Text>
      <Text size="sm" fw={600} mt={2}>{children}</Text>
    </div>
  );
}

function updateMessage(update: RemoteServerUpdate, version: string): string {
  const target = update.phase.version;
  switch (update.phase.state) {
    case "checking": return "新しい版を確認しています";
    case "waiting": return update.auto ? `版 ${target} へ夜間に自動で更新します` : `版 ${target} に更新できます`;
    case "applying": return `版 ${target} に更新しています。数十秒ほど止まります`;
    default: {
      if (update.latest === version) return update.auto ? "最新の版です。新しい版が出ると、夜間に自動で更新します" : "最新の版です。自動更新はオフです";
      return update.auto ? "新しい版が出ると、夜間に自動で更新します" : "自動更新はオフです";
    }
  }
}

function RemoteUpdate({ update, version, pending, active, perform }: { update: RemoteServerUpdate; version: string } & Pick<CardProps, "pending" | "active" | "perform">) {
  const state = update.phase.state;
  // すでに最新なら押しても何も起きないので、ボタンを出さない。
  const offerable = state === "waiting" || (state === "idle" && update.latest !== version);
  return (
    <Stack gap={6}>
      <Group justify="space-between" wrap="wrap" mih={30}>
        <Text size="sm" c="dimmed">{updateMessage(update, version)}</Text>
        {offerable && (
          <Button size="xs" variant="light" disabled={pending} loading={active === "update"} onClick={() => void perform(serverUpdateRemote, "update")}>
            {state === "waiting" ? `版 ${update.phase.version} に更新` : "更新を確認"}
          </Button>
        )}
      </Group>
      {update.lastError && <Alert color="yellow" title="サーバーを更新できませんでした">{update.lastError}</Alert>}
    </Stack>
  );
}

/** 登録が失われたときは、解除を挟まずにその場で登録し直せる。 */
function Reconnect({ pending, active, perform }: Pick<CardProps, "pending" | "active" | "perform">) {
  const [text, setText] = useState("");
  return (
    <Stack gap="xs">
      <Text size="sm">サーバーで <Code>piep-server admin-pair --replace</Code> を実行し、表示された登録文字列を貼り付けてください。</Text>
      <Textarea aria-label="新しい登録文字列" placeholder="登録文字列を貼り付け" autosize minRows={2} maxRows={5} value={text} onChange={(event) => setText(event.currentTarget.value)} />
      <Group justify="flex-end">
        <Button size="xs" disabled={!text.trim() || pending} loading={active === "reconnect"} onClick={() => void perform(async () => { const next = await serverConnectRemote(text); setText(""); return next; }, "reconnect")}>登録し直す</Button>
      </Group>
    </Stack>
  );
}

function RemoteCard({ status, pending, active, perform, details, connectionError }: CardProps & { details: RemoteServerStatus | undefined; connectionError: string | null }) {
  const sync = details?.sync;
  return (
    <Card withBorder p="lg">
      <Stack gap="md">
        <Group justify="space-between" wrap="nowrap">
          <Text fw={700}>別の PC のサーバー</Text>
          <Badge color={connectionError ? "red" : details ? "green" : "gray"} style={{ flexShrink: 0 }}>{connectionError ? "接続できません" : details ? "接続中" : "確認中"}</Badge>
        </Group>
        <Text size="sm" style={{ overflowWrap: "anywhere" }}>接続先: <Code>{status.remote?.url}</Code></Text>
        {connectionError && (
          <Alert color="red" title="サーバーに接続できません">
            <Stack gap="sm">
              <Text size="sm">{connectionError}</Text>
              {connectionError.includes(REGISTRATION_LOST) && <Reconnect pending={pending} active={active} perform={perform} />}
            </Stack>
          </Alert>
        )}
        {details && (
          <SimpleGrid cols={{ base: 2, sm: 3 }} spacing="sm">
            <Fact label="サーバーの版">{details.version}</Fact>
            <Fact label="本棚">{details.works === null ? "まだ送っていません" : `${formatNumber(details.works)} 作品`}</Fact>
            <Fact label="携帯からの検索">{sync?.indexPending ? `準備中（残り ${formatNumber(sync.indexPending)} 件）` : "使えます"}</Fact>
          </SimpleGrid>
        )}
        {sync?.indexError && <Alert color="yellow" title="サーバーの検索索引を更新できません">{sync.indexError}</Alert>}
        {details?.update && <RemoteUpdate update={details.update} version={details.version} perform={perform} pending={pending} active={active} />}
        <Divider />
        <Group justify="flex-end">
          <DangerButton
            size="xs"
            disabled={pending}
            onClick={() => modals.openConfirmModal({
              title: "サーバーとの接続を解除しますか？",
              children: <Text size="sm">この PC に保存した管理者の鍵を削除し、自動で送る設定もオフにします。サーバーの本棚と携帯の登録はそのまま残ります。接続し直すには、サーバーで <Code>piep-server admin-pair --replace</Code> を実行してください。</Text>,
              labels: { confirm: "解除", cancel: "キャンセル" },
              confirmProps: DANGER_CONFIRM,
              onConfirm: () => void perform(serverDisconnectRemote),
            })}
          >接続を解除</DangerButton>
        </Group>
      </Stack>
    </Card>
  );
}

function SendCard({ status, pending, active, perform, connectionError }: CardProps & { connectionError: string | null }) {
  const send = status.send;
  const last = status.lastSend;
  const fraction = send.bytesTotal > 0 ? Math.min(100, (send.bytesDone / send.bytesTotal) * 100) : 0;
  // 同じ理由で接続できないなら、上の案内と重ねて出さない。
  const lastError = last.error && last.error !== connectionError ? last.error : null;
  return (
    <Card withBorder p="lg">
      <Stack gap="md">
        <Group justify="space-between">
          <Text fw={700}>本棚の送信</Text>
          {!send.running && <Button size="xs" disabled={pending || Boolean(connectionError)} loading={active === "send"} onClick={() => void perform(serverSendNow, "send")}>今すぐ送る</Button>}
        </Group>
        {send.running && (
          <Stack gap={6}>
            <Group justify="space-between" wrap="nowrap">
              <Text size="sm">{SEND_PHASE[send.phase] ?? "送信しています"}</Text>
              {send.phase === "upload" && <Text size="sm" fw={600}>{Math.floor(fraction)}%</Text>}
            </Group>
            <Progress value={fraction} aria-label="送信の進み具合" />
            {send.phase === "upload" && <Text size="xs" c="dimmed">{formatNumber(send.filesDone)} / {formatNumber(send.filesTotal)} 件・{formatBytes(send.bytesDone)} / {formatBytes(send.bytesTotal)}</Text>}
            <Group justify="space-between" wrap="nowrap" align="center">
              <Text size="xs" c="dimmed">中止しても、送信済みのファイルはサーバーに残ります。次回はその続きから送ります。</Text>
              <Button size="xs" variant="default" style={{ flexShrink: 0 }} onClick={() => void perform(serverSendStop)}>中止</Button>
            </Group>
          </Stack>
        )}
        {last.finishedAt && !last.error && (
          <Text size="sm" c="dimmed">
            前回の送信: {formatTime(last.finishedAt)}（{last.unchanged ? "変更なし" : `${formatNumber(last.works ?? 0)} 作品、追加 ${formatNumber(last.filesAdded ?? 0)} ファイル、削除 ${formatNumber(last.filesRemoved ?? 0)} ファイル`}）
          </Text>
        )}
        {last.finishedAt && lastError && (
          <Alert color={send.phase === "canceled" ? "gray" : "red"} title={`前回の送信: ${formatTime(last.finishedAt)}`}>{lastError}</Alert>
        )}
        <Switch
          label="自動で送る"
          description="本棚を変更してから 5 分間、ほかの変更がなければ送ります。"
          checked={status.autoSend}
          disabled={pending}
          onChange={(event) => { const enabled = event.currentTarget.checked; void perform(() => serverSetAutoSend(enabled)); }}
        />
        {Boolean(last.missingReferenced) && <Alert color="yellow" title="見つからないファイルがあります">本棚が参照しているファイルのうち {formatNumber(last.missingReferenced ?? 0)} 件がサーバーにありません。この PC からも失われている可能性があります。診断で確認してください。</Alert>}
        {send.outsidePaths > 0 && <Text size="xs" c="dimmed">本棚の保存先の外にあるファイル {formatNumber(send.outsidePaths)} 件は送信していません。</Text>}
        {send.skippedNames > 0 && <Text size="xs" c="dimmed">ファイル名を扱えないファイル {formatNumber(send.skippedNames)} 件は送信していません。</Text>}
      </Stack>
    </Card>
  );
}

const CONNECTION_HELP: Record<"lan" | "proxy", string> = {
  lan: "携帯からこの PC に届くネットワークで使えます。通信は自己署名の証明書で暗号化します。",
  proxy: "https の公開 URL で、どこからでも接続できます。転送の仕組みは自分で用意します。",
};

function EmbeddedCard({ status, pending, active, perform }: CardProps) {
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
    <Card withBorder p="lg">
      <Stack gap="md">
        <Group justify="space-between" wrap="nowrap">
          <Text fw={700}>この PC をサーバーにする</Text>
          <Badge color={status.embeddedRunning ? "green" : "gray"} style={{ flexShrink: 0 }}>{status.embeddedRunning ? "稼働中" : "停止中"}</Badge>
        </Group>
        {status.embeddedRunning ? (
          <>
            <Text size="sm" style={{ overflowWrap: "anywhere" }}>接続先: <Code>{status.embeddedUrl}</Code></Text>
            {status.embeddedFingerprint && <Text size="xs" c="dimmed" style={{ overflowWrap: "anywhere" }}>証明書のフィンガープリント: <Code>{status.embeddedFingerprint}</Code></Text>}
            <Group justify="space-between" wrap="nowrap">
              <Text size="sm" c="dimmed">この PC が起動している間だけ、携帯から読めます。</Text>
              <Button variant="default" size="xs" style={{ flexShrink: 0 }} disabled={pending} loading={active === "stop"} onClick={() => void perform(serverEmbeddedStop, "stop")}>停止</Button>
            </Group>
          </>
        ) : (
          <>
            <Select
              label="携帯からの接続方法"
              description={CONNECTION_HELP[mode]}
              maw={560}
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
                maw={560}
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
                maw={560}
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
          </>
        )}
      </Stack>
    </Card>
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

function AddPhoneSteps({ devices, onClose }: { devices: ServerDevice[]; onClose: () => void }) {
  const [known] = useState(() => new Set(devices.map((device) => device.id)));
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

  const added = devices.find((device) => !known.has(device.id));
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
      <Text size="sm">携帯で piep を開き、「設定 → サーバーとの接続 → QR を読む」でこの QR を読み取ってください。</Text>
      {problem && <Alert color="red" title="QR を作成できませんでした">{problem}</Alert>}
      {pairing ? (
        <>
          <QrImage value={pairing.payload} label="登録用の QR" />
          <Text size="xs" c="dimmed">{Math.round(pairing.expiresInSeconds / 60)} 分間有効で、一度だけ使えます。</Text>
          <Group gap="xs">
            <Button
              size="xs"
              variant="light"
              onClick={() => void navigator.clipboard.writeText(pairing.payload).then(() => setCopied(true)).catch(() => setProblem("登録文字列をコピーできませんでした"))}
            >{copied ? "コピーしました" : "登録文字列をコピー"}</Button>
          </Group>
          <Text size="xs" c="dimmed">QR を読めないときは、コピーした文字列を携帯に貼り付けてください。この文字列を知っている人は本棚を読めるので、人に見せないでください。</Text>
        </>
      ) : (
        <>
          {expired && <Text size="sm" c="dimmed">QR の有効期限が切れました。</Text>}
          {(expired || problem) && <Group><Button variant="light" loading={issuing} onClick={() => void issue()}>QR を作り直す</Button></Group>}
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

function DevicesCard({ status, pending, perform, connectionError = null }: CardProps & { connectionError?: string | null }) {
  const remote = status.remote;
  const [adding, setAdding] = useState(false);
  const remoteDevices = useQuery({
    queryKey: ["server-remote-devices", remote?.url],
    queryFn: serverRemoteDevices,
    enabled: Boolean(remote),
    refetchInterval: adding ? 3_000 : 15_000,
    retry: false,
  });
  const devices: ServerDevice[] = remote ? remoteDevices.data ?? [] : status.devices;
  const hasPhone = devices.some((device) => device.role === "reader");
  // 接続できないときは上のカードで伝えているので、同じ理由を繰り返さない。
  const listError = remoteDevices.error && !connectionError ? errorMessage(remoteDevices.error) : null;
  return (
    <Card withBorder p="lg">
      <Stack gap="sm">
        <Group justify="space-between">
          <Text fw={700}>登録している端末</Text>
          <Button size="xs" variant={hasPhone ? "light" : "filled"} disabled={Boolean(connectionError)} onClick={() => setAdding(true)}>携帯を追加</Button>
        </Group>
        <Modal opened={adding} onClose={() => setAdding(false)} title="携帯を追加" centered>
          <AddPhoneSteps devices={devices} onClose={() => setAdding(false)} />
        </Modal>
        {listError && <Text size="sm" c="red">{listError}</Text>}
        {connectionError && !devices.length && <Text size="sm" c="dimmed">接続できるようになると、一覧を表示します。</Text>}
        {!connectionError && !listError && !devices.length && <Text size="sm" c="dimmed">登録している端末はまだありません。</Text>}
        {devices.map((device, index) => (
          <Stack key={device.id} gap="sm">
            {index > 0 && <Divider />}
            <Group justify="space-between" wrap="nowrap">
              <div style={{ minWidth: 0 }}>
                <Group gap={6}>
                  <Text size="sm" fw={600}>{device.name || "Android 端末"}</Text>
                  {device.role === "admin" && <Badge size="xs" variant="light" color="gray">管理者</Badge>}
                </Group>
                <Text size="xs" c="dimmed">最後の接続: {formatTime(device.lastSeenAt)}・登録: {formatTime(device.createdAt)}</Text>
              </div>
              {device.role === "reader" && (
                <DangerButton
                  size="xs"
                  style={{ flexShrink: 0 }}
                  disabled={pending}
                  onClick={() => modals.openConfirmModal({
                    title: `「${device.name || "Android 端末"}」の登録を解除しますか？`,
                    children: <Text size="sm">解除すると、この端末から本棚を読めなくなります。解除はすぐに反映されます。</Text>,
                    labels: { confirm: "登録を解除", cancel: "キャンセル" },
                    confirmProps: DANGER_CONFIRM,
                    onConfirm: () => void perform(async () => {
                      const next = await serverRevokeDevice(device.id);
                      await remoteDevices.refetch();
                      return next;
                    }),
                  })}
                >登録を解除</DangerButton>
              )}
            </Group>
          </Stack>
        ))}
      </Stack>
    </Card>
  );
}
