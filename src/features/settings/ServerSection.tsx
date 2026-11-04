import { useEffect, useState } from "react";
import { Alert, Badge, Button, Card, Code, Divider, Group, Modal, Progress, Select, Stack, Switch, Text, TextInput, Textarea } from "@mantine/core";
import { modals } from "@mantine/modals";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { QRCodeSVG } from "qrcode.react";
import { errorMessage, formatBytes, formatNumber } from "@/lib/format";
import { SectionIntro } from "./SectionIntro";
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
  type RemoteServerUpdate,
  type ServerDevice,
  type ServerPairing,
} from "@/services/serverApi";

const ANDROID_APP_URL = "https://github.com/1211snowmaple/piep-android/releases/latest";
const STATUS_KEY = ["server-status"];

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

export function ServerSection({ runtime }: { runtime: boolean }) {
  const queryClient = useQueryClient();
  const status = useQuery({
    queryKey: STATUS_KEY,
    queryFn: serverStatus,
    enabled: runtime,
    refetchInterval: (query) => (query.state.data?.send.running ? 1_000 : 5_000),
  });
  const [problem, setProblem] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  const perform = async (action: () => Promise<DesktopServerStatus | void>) => {
    setPending(true);
    setProblem(null);
    try {
      const next = await action();
      if (next) queryClient.setQueryData(STATUS_KEY, next);
      await status.refetch();
    } catch (error) {
      setProblem(errorMessage(error));
    } finally {
      setPending(false);
    }
  };

  const data = status.data;
  const remote = data?.remote ?? null;
  return (
    <Stack gap="lg">
      <SectionIntro beta title="サーバー" description="携帯から、外出先でも本棚を検索して読めるようにします。常に動いている別の PC にサーバーを置くか、この PC をサーバーにします。" />
      <Alert color="yellow" title="サーバーはβ版です">
        <Text size="sm">携帯との通信方法や保存の形式は、予告して変更することがあります。</Text>
      </Alert>
      {!runtime && <Alert color="blue">サーバーの操作はデスクトップアプリで利用できます。</Alert>}
      {problem && <Alert color="red" title="操作できませんでした">{problem}</Alert>}
      {status.error && <Alert color="red" title="サーバーの状態を確認できません">{errorMessage(status.error)}</Alert>}
      {runtime && data && (
        <>
          <RemoteCard status={data} pending={pending} perform={perform} />
          {(!remote || data.embeddedRunning) && <EmbeddedCard status={data} pending={pending} perform={perform} />}
          {(remote || data.embeddedRunning) && <DevicesCard status={data} pending={pending} perform={perform} />}
        </>
      )}
    </Stack>
  );
}

interface CardProps {
  status: DesktopServerStatus;
  pending: boolean;
  perform: (action: () => Promise<DesktopServerStatus | void>) => Promise<void>;
}

function updateMessage(update: RemoteServerUpdate): string {
  const version = update.phase.version;
  switch (update.phase.state) {
    case "checking": return "新しい版を確認しています";
    case "waiting": return update.auto ? `版 ${version} へ夜間に自動で更新します` : `版 ${version} に更新できます`;
    case "applying": return `版 ${version} に更新しています。数十秒ほど止まります`;
    default: return update.auto ? "新しい版が出ると、夜間に自動で更新します" : "自動更新はオフです";
  }
}

function RemoteUpdate({ update, pending, perform }: { update: RemoteServerUpdate } & Pick<CardProps, "pending" | "perform">) {
  return (
    <Stack gap={6}>
      <Group justify="space-between" wrap="wrap">
        <Text size="sm" c="dimmed">{updateMessage(update)}</Text>
        <Button size="xs" variant="light" loading={pending} disabled={update.phase.state === "applying"} onClick={() => void perform(serverUpdateRemote)}>今すぐ更新</Button>
      </Group>
      {update.lastError && <Alert color="yellow" title="サーバーを更新できませんでした">{update.lastError}</Alert>}
    </Stack>
  );
}

function RemoteCard({ status, pending, perform }: CardProps) {
  const [text, setText] = useState("");
  const remote = status.remote;
  const details = useQuery({
    queryKey: ["server-remote-status", remote?.url, status.lastSend.finishedAt],
    queryFn: serverRemoteStatus,
    enabled: Boolean(remote),
    refetchInterval: 15_000,
    retry: false,
  });
  if (!remote) {
    return (
      <Card withBorder p="lg">
        <Stack gap="sm">
          <Text fw={700}>別の PC のサーバーに接続</Text>
          <Text size="sm" c="dimmed">サーバーを導入すると、管理者用の登録文字列が表示されます。ここに貼り付けてください。一度だけ使えます。</Text>
          <Textarea aria-label="管理者用の登録文字列" placeholder="登録文字列を貼り付け" autosize minRows={2} maxRows={5} value={text} onChange={(event) => setText(event.currentTarget.value)} />
          <Group>
            <Button disabled={!text.trim()} loading={pending} onClick={() => void perform(async () => { const next = await serverConnectRemote(text); setText(""); return next; })}>接続</Button>
          </Group>
        </Stack>
      </Card>
    );
  }
  const send = status.send;
  const last = status.lastSend;
  const fraction = send.bytesTotal > 0 ? Math.min(100, (send.bytesDone / send.bytesTotal) * 100) : 0;
  const sync = details.data?.sync;
  return (
    <Card withBorder p="lg">
      <Stack gap="md">
        <Group justify="space-between">
          <Text fw={700}>別の PC のサーバー</Text>
          <Badge color={details.error ? "red" : details.data ? "green" : "gray"}>{details.error ? "接続できません" : details.data ? "接続中" : "確認中"}</Badge>
        </Group>
        <Text size="sm">接続先: <Code>{remote.url}</Code></Text>
        {details.error && <Text size="sm" c="red">{errorMessage(details.error)}</Text>}
        {details.data && (
          <Text size="sm" c="dimmed">
            サーバーの版 {details.data.version}・{details.data.works === null ? "本棚はまだ送っていません" : `${formatNumber(details.data.works)} 作品`}
            {sync?.indexPending ? `・検索の準備中（残り ${formatNumber(sync.indexPending)} 件）` : ""}
          </Text>
        )}
        {sync?.indexError && <Alert color="yellow" title="サーバーの検索索引を更新できません">{sync.indexError}</Alert>}
        {details.data?.update && <RemoteUpdate update={details.data.update} perform={perform} pending={pending} />}
        <Divider />
        <Text fw={600} size="sm">本棚の送信</Text>
        {send.running ? (
          <Stack gap={6}>
            <Text size="sm">{SEND_PHASE[send.phase] ?? "送信しています"}</Text>
            {send.phase === "upload" && <Text size="xs" c="dimmed">{formatNumber(send.filesDone)} / {formatNumber(send.filesTotal)} 件・{formatBytes(send.bytesDone)} / {formatBytes(send.bytesTotal)}</Text>}
            <Progress value={fraction} aria-label="送信の進み具合" />
            <Group><Button color="red" variant="light" onClick={() => void perform(serverSendStop)}>中止</Button></Group>
            <Text size="xs" c="dimmed">中止しても、送信済みのファイルはサーバーに残ります。次回はその続きから送ります。</Text>
          </Stack>
        ) : (
          <Group>
            <Button loading={pending} onClick={() => void perform(serverSendNow)}>今すぐ送る</Button>
          </Group>
        )}
        <Switch
          label="自動で送る"
          description="本棚を変更してから 5 分間、ほかの変更がなければ送ります。"
          checked={status.autoSend}
          disabled={pending}
          onChange={(event) => { const enabled = event.currentTarget.checked; void perform(() => serverSetAutoSend(enabled)); }}
        />
        {last.finishedAt && (
          last.error ? (
            <Alert color={send.phase === "canceled" ? "gray" : "red"} title={`前回の送信: ${formatTime(last.finishedAt)}`}>{last.error}</Alert>
          ) : (
            <Text size="sm" c="dimmed">
              前回の送信: {formatTime(last.finishedAt)}（{last.unchanged ? "変更なし" : `${formatNumber(last.works ?? 0)} 作品、追加 ${formatNumber(last.filesAdded ?? 0)} ファイル、削除 ${formatNumber(last.filesRemoved ?? 0)} ファイル`}）
            </Text>
          )
        )}
        {Boolean(last.missingReferenced) && <Alert color="yellow" title="見つからないファイルがあります">本棚が参照しているファイルのうち {formatNumber(last.missingReferenced ?? 0)} 件がサーバーにありません。この PC からも失われている可能性があります。診断で確認してください。</Alert>}
        {send.outsidePaths > 0 && <Text size="xs" c="dimmed">本棚の保存先の外にあるファイル {formatNumber(send.outsidePaths)} 件は送信していません。</Text>}
        {send.skippedNames > 0 && <Text size="xs" c="dimmed">ファイル名を扱えないファイル {formatNumber(send.skippedNames)} 件は送信していません。</Text>}
        <Divider />
        <Group>
          <Button
            size="xs"
            color="red"
            variant="subtle"
            disabled={pending}
            onClick={() => modals.openConfirmModal({
              title: "サーバーとの接続を解除しますか？",
              children: <Text size="sm">この PC に保存した管理者の鍵を削除します。サーバーの本棚と携帯の登録はそのまま残ります。接続し直すには、サーバーで <Code>piep-server admin-pair</Code> を実行してください。</Text>,
              labels: { confirm: "解除", cancel: "キャンセル" },
              confirmProps: { color: "red" },
              onConfirm: () => void perform(serverDisconnectRemote),
            })}
          >接続を解除</Button>
        </Group>
      </Stack>
    </Card>
  );
}

function EmbeddedCard({ status, pending, perform }: CardProps) {
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
        <Group justify="space-between">
          <Text fw={700}>この PC をサーバーにする</Text>
          <Badge color={status.embeddedRunning ? "green" : "gray"}>{status.embeddedRunning ? "稼働中" : "停止中"}</Badge>
        </Group>
        <Text size="sm" c="dimmed">この PC が起動している間だけ、携帯から読めます。</Text>
        {status.embeddedRunning ? (
          <>
            <Text size="sm">接続先: <Code>{status.embeddedUrl}</Code></Text>
            {status.embeddedFingerprint && <Text size="xs" c="dimmed">証明書のフィンガープリント: <Code>{status.embeddedFingerprint}</Code></Text>}
            <Group><Button color="red" variant="light" loading={pending} onClick={() => void perform(serverEmbeddedStop)}>停止</Button></Group>
          </>
        ) : (
          <>
            <Select
              label="携帯からの接続方法"
              data={[
                { value: "lan", label: "同じネットワークから接続する（自己署名の証明書）" },
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
            <Group>
              <Button
                disabled={needsIp ? !ip : !publicUrl.trim().startsWith("https://")}
                loading={pending}
                onClick={() => void perform(() => serverEmbeddedStart({
                  enabled: true,
                  ip: needsIp ? ip : null,
                  behindProxy: needsUrl,
                  publicUrl: needsUrl ? publicUrl.trim() : null,
                }))}
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

function DevicesCard({ status, pending, perform }: CardProps) {
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
  return (
    <Card withBorder p="lg">
      <Stack gap="md">
        <Group justify="space-between">
          <Text fw={700}>登録している端末</Text>
          <Button size="xs" variant="light" onClick={() => setAdding(true)}>携帯を追加</Button>
        </Group>
        <Modal opened={adding} onClose={() => setAdding(false)} title="携帯を追加" centered>
          <AddPhoneSteps devices={devices} onClose={() => setAdding(false)} />
        </Modal>
        {remoteDevices.error && <Text size="sm" c="red">{errorMessage(remoteDevices.error)}</Text>}
        {devices.length ? devices.map((device) => (
          <Group key={device.id} justify="space-between">
            <div>
              <Group gap={6}>
                <Text size="sm" fw={600}>{device.name || "Android 端末"}</Text>
                {device.role === "admin" && <Badge size="xs" variant="light" color="gray">管理者</Badge>}
              </Group>
              <Text size="xs" c="dimmed">登録: {formatTime(device.createdAt)}・最終接続: {formatTime(device.lastSeenAt)}</Text>
            </div>
            {device.role === "reader" && (
              <Button
                size="xs"
                color="red"
                variant="subtle"
                disabled={pending}
                onClick={() => modals.openConfirmModal({
                  title: `「${device.name || "Android 端末"}」の登録を解除しますか？`,
                  children: <Text size="sm">解除すると、この端末から本棚を読めなくなります。解除はすぐに反映されます。</Text>,
                  labels: { confirm: "登録を解除", cancel: "キャンセル" },
                  confirmProps: { color: "red" },
                  onConfirm: () => void perform(async () => {
                    const next = await serverRevokeDevice(device.id);
                    await remoteDevices.refetch();
                    return next;
                  }),
                })}
              >登録を解除</Button>
            )}
          </Group>
        )) : <Text size="sm" c="dimmed">登録している端末はまだありません。</Text>}
      </Stack>
    </Card>
  );
}
