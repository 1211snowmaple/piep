import { useEffect, useState } from "react";
import { Alert, Badge, Box, Button, Card, Code, Group, Progress, Select, Stack, Switch, Text, TextInput, Textarea, Title } from "@mantine/core";
import { modals } from "@mantine/modals";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { QRCodeSVG } from "qrcode.react";
import { errorMessage, formatBytes, formatNumber } from "@/lib/format";
import {
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
  type DesktopServerStatus,
  type ServerDevice,
  type ServerPairing,
} from "@/services/serverApi";

const STATUS_KEY = ["server-status"];

/** 送信の段どりを、そのまま画面の言葉にする。 */
const SEND_PHASE: Record<string, string> = {
  snapshot: "本棚の写しを作っています",
  manifest: "サーバーとの差を調べています",
  upload: "ファイルを送っています",
  database: "本棚の写しを送っています",
  commit: "サーバーで入れ替えています",
};

function formatTime(value: string | null | undefined): string {
  return value ? new Date(value).toLocaleString("ja-JP") : "—";
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
  const [pairing, setPairing] = useState<ServerPairing | null>(null);

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
      <Box>
        <Group gap="xs"><Title order={2}>サーバー</Title><Badge variant="light">β</Badge></Group>
        <Text c="dimmed" size="sm" mt={5}>携帯から、家の外でも本棚を読めるようにします。常時動いている機械に単独サーバーを置くか、この PC をサーバーにします。</Text>
      </Box>
      {!runtime && <Alert color="blue">サーバーの操作はデスクトップアプリで利用できます。</Alert>}
      {problem && <Alert color="red" title="操作できませんでした">{problem}</Alert>}
      {status.error && <Alert color="red" title="サーバーの状態を確認できません">{errorMessage(status.error)}</Alert>}
      {runtime && data && (
        <>
          <RemoteCard status={data} pending={pending} perform={perform} />
          {(!remote || data.embeddedRunning) && <EmbeddedCard status={data} pending={pending} perform={perform} />}
          {(remote || data.embeddedRunning) && (
            <DevicesCard
              status={data}
              pending={pending}
              pairing={pairing}
              setPairing={setPairing}
              perform={perform}
              onCopyFailed={() => setProblem("登録文字列をコピーできませんでした")}
            />
          )}
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
          <Text fw={700}>別の機械のサーバーにつなぐ</Text>
          <Text size="sm" c="dimmed">サーバーで <Code>piep-server init</Code> を実行すると、管理者用の登録文字列が出ます。一度だけ使えます。</Text>
          <Textarea aria-label="管理者用の登録文字列" placeholder="登録文字列を貼り付け" autosize minRows={2} maxRows={5} value={text} onChange={(event) => setText(event.currentTarget.value)} />
          <Group>
            <Button disabled={!text.trim()} loading={pending} onClick={() => void perform(async () => { const next = await serverConnectRemote(text); setText(""); return next; })}>つなぐ</Button>
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
          <Text fw={700}>単独サーバー</Text>
          <Badge color={details.error ? "red" : details.data ? "green" : "gray"}>{details.error ? "つながりません" : details.data ? "つながっています" : "確認中"}</Badge>
        </Group>
        <Text size="sm">接続先: <Code>{remote.url}</Code></Text>
        {details.error && <Text size="sm" c="red">{errorMessage(details.error)}</Text>}
        {details.data && (
          <Text size="sm" c="dimmed">
            サーバーの版 {details.data.version}・{details.data.works === null ? "本棚はまだありません" : `${formatNumber(details.data.works)} 作品`}
            {sync?.indexPending ? `・索引の残り ${formatNumber(sync.indexPending)} 件` : ""}
          </Text>
        )}
        {sync?.indexError && <Alert color="yellow" title="サーバーの索引を更新できません">{sync.indexError}</Alert>}
        {send.running ? (
          <Stack gap={6}>
            <Text size="sm">{SEND_PHASE[send.phase] ?? "送っています"}</Text>
            {send.phase === "upload" && <Text size="xs" c="dimmed">{formatNumber(send.filesDone)} / {formatNumber(send.filesTotal)} 件・{formatBytes(send.bytesDone)} / {formatBytes(send.bytesTotal)}</Text>}
            <Progress value={fraction} aria-label="送信の進み具合" />
            <Group><Button color="red" variant="light" onClick={() => void perform(serverSendStop)}>止める</Button></Group>
            <Text size="xs" c="dimmed">止めても、送った分はサーバーに残ります。次の送信が続きから使います。</Text>
          </Stack>
        ) : (
          <Group>
            <Button loading={pending} onClick={() => void perform(serverSendNow)}>今すぐ送る</Button>
          </Group>
        )}
        <Switch
          label="自動で送る"
          description="本棚が変わってから 5 分静かになったときに送ります。"
          checked={status.autoSend}
          disabled={pending}
          onChange={(event) => { const enabled = event.currentTarget.checked; void perform(() => serverSetAutoSend(enabled)); }}
        />
        {last.finishedAt && (
          last.error ? (
            <Alert color={send.phase === "canceled" ? "gray" : "red"} title={`前回の送信 ${formatTime(last.finishedAt)}`}>{last.error}</Alert>
          ) : (
            <Text size="sm" c="dimmed">
              前回の送信 {formatTime(last.finishedAt)}：{last.unchanged ? "変更はありませんでした" : `${formatNumber(last.works ?? 0)} 作品を反映（ファイル 追加 ${formatNumber(last.filesAdded ?? 0)} 件・退避 ${formatNumber(last.filesRemoved ?? 0)} 件）`}
            </Text>
          )
        )}
        {Boolean(last.missingReferenced) && <Alert color="yellow" title="ファイルの欠けがあります">本棚が指しているファイルのうち {formatNumber(last.missingReferenced ?? 0)} 件が、サーバーにありません。この PC でも欠けている可能性があります。診断で確かめてください。</Alert>}
        {send.outsidePaths > 0 && <Text size="xs" c="dimmed">本棚の置き場の外を指すパス {formatNumber(send.outsidePaths)} 件は、サーバーへ送っていません。</Text>}
        {send.skippedNames > 0 && <Text size="xs" c="dimmed">名前を送れないファイル {formatNumber(send.skippedNames)} 件を飛ばしました。</Text>}
        <Group>
          <Button
            size="xs"
            color="red"
            variant="subtle"
            disabled={pending}
            onClick={() => modals.openConfirmModal({
              title: "サーバーとのつながりを解除しますか？",
              children: <Text size="sm">この PC の管理者の鍵を消します。サーバーの本棚と携帯の登録はそのまま残ります。つなぎ直すには、サーバーで <Code>piep-server admin-pair</Code> を実行してください。</Text>,
              labels: { confirm: "解除", cancel: "キャンセル" },
              confirmProps: { color: "red" },
              onConfirm: () => void perform(serverDisconnectRemote),
            })}
          >つながりを解除</Button>
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
          <Badge color={status.embeddedRunning ? "green" : "gray"}>{status.embeddedRunning ? "動いています" : "止まっています"}</Badge>
        </Group>
        <Text size="sm" c="dimmed">この PC が動いている間だけ、携帯から読めます。</Text>
        {status.embeddedRunning ? (
          <>
            <Text size="sm">接続先: <Code>{status.embeddedUrl}</Code></Text>
            {status.embeddedFingerprint && <Text size="xs" c="dimmed">証明書の指紋: <Code>{status.embeddedFingerprint}</Code></Text>}
            <Group><Button color="red" variant="light" loading={pending} onClick={() => void perform(serverEmbeddedStop)}>止める</Button></Group>
          </>
        ) : (
          <>
            <Select
              label="出す先"
              data={[
                { value: "lan", label: "同じネットワークの携帯へ（自己署名の証明書）" },
                { value: "proxy", label: "トンネルやリバースプロキシの裏" },
              ]}
              value={mode}
              allowDeselect={false}
              onChange={(value) => setMode(value === "proxy" ? "proxy" : "lan")}
            />
            {needsIp && (
              <Select
                label="待ち受けるアドレス"
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
                description="携帯がつなぐ https の URL。この PC の 127.0.0.1:38777 へ転送するように設定してください。"
                placeholder="https://example.ts.net/"
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
              >始める</Button>
            </Group>
          </>
        )}
      </Stack>
    </Card>
  );
}

function DevicesCard({ status, pending, pairing, setPairing, perform, onCopyFailed }: CardProps & {
  pairing: ServerPairing | null;
  setPairing: (pairing: ServerPairing | null) => void;
  onCopyFailed: () => void;
}) {
  const remote = status.remote;
  const remoteDevices = useQuery({
    queryKey: ["server-remote-devices", remote?.url],
    queryFn: serverRemoteDevices,
    enabled: Boolean(remote),
    refetchInterval: 15_000,
    retry: false,
  });
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!pairing) return;
    const timer = window.setTimeout(() => setPairing(null), pairing.expiresInSeconds * 1_000);
    return () => window.clearTimeout(timer);
  }, [pairing, setPairing]);
  const devices: ServerDevice[] = remote ? remoteDevices.data ?? [] : status.devices;
  const minutes = pairing ? Math.round(pairing.expiresInSeconds / 60) : 0;
  return (
    <Card withBorder p="lg">
      <Stack gap="md">
        <Group justify="space-between">
          <Text fw={700}>携帯</Text>
          <Button size="xs" variant="light" loading={pending} onClick={() => void perform(async () => { setCopied(false); setPairing(await serverCreateReaderPairing()); })}>携帯を追加</Button>
        </Group>
        {pairing && (
          <Stack gap="sm">
            <Text size="sm" c="dimmed">{minutes} 分だけ有効で、一度だけ使えます。Android 版で QR を読み取るか、登録文字列を貼り付けてください。</Text>
            <div style={{ alignSelf: "flex-start", padding: 12, background: "#fff", borderRadius: 8 }}><QRCodeSVG value={pairing.payload} size={188} level="M" /></div>
            <Button
              variant="light"
              onClick={() => void navigator.clipboard.writeText(pairing.payload).then(() => setCopied(true)).catch(onCopyFailed)}
            >{copied ? "コピーしました" : "登録文字列をコピー"}</Button>
            <Text size="xs" c="dimmed">この文字列には登録鍵が含まれます。人に見せないでください。</Text>
          </Stack>
        )}
        {remoteDevices.error && <Text size="sm" c="red">{errorMessage(remoteDevices.error)}</Text>}
        {devices.length ? devices.map((device) => (
          <Group key={device.id} justify="space-between">
            <div>
              <Text size="sm" fw={600}>{device.name || "Android 端末"}{device.role === "admin" ? "（管理者）" : ""}</Text>
              <Text size="xs" c="dimmed">登録 {formatTime(device.createdAt)}・最後の接続 {formatTime(device.lastSeenAt)}{device.lastUser ? `・${device.lastUser}` : ""}</Text>
            </div>
            {device.role === "reader" && (
              <Button
                size="xs"
                color="red"
                variant="subtle"
                disabled={pending}
                onClick={() => modals.openConfirmModal({
                  title: "端末の登録を解除しますか？",
                  children: <Text size="sm">解除すると、その端末から本棚を読めなくなります。すぐに効きます。</Text>,
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
        )) : <Text size="sm" c="dimmed">登録した端末はまだありません。</Text>}
      </Stack>
    </Card>
  );
}
