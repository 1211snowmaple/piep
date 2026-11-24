import { useEffect, useState } from "react";
import { Alert, Anchor, Button, Group, Modal, Skeleton, Stack, Switch, Text, TextInput } from "@mantine/core";
import { modals } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Icons, IconSize } from "@/lib/icons";
import { QRCodeSVG } from "qrcode.react";
import { DANGER_CONFIRM, DangerButton } from "@/components/DangerButton";
import { adminApi, failureMessage, type Device, type Pairing } from "../api";
import { dayTime, relativeTime } from "../format";
import { notifyIfExpired } from "../queryClient";
import { Card, PageHead, showFailure, useReauth, useStatus } from "../shared";

const ANDROID_DOWNLOAD_URL = "https://github.com/1211snowmaple/piep/releases/latest";

function CopyPairingButton({ pairing }: { pairing: Pairing }) {
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(pairing.payload);
      notifications.show({ message: "コピーしました" });
    } catch {
      notifications.show({ color: "red", message: "コピーできませんでした" });
    }
  };
  return <Button variant="default" leftSection={<Icons.copy size={IconSize.menu} />} onClick={() => void copy()}>登録文字列をコピー</Button>;
}

function PairingDialog({ pairing, onClose }: { pairing: Pairing | null; onClose: () => void }) {
  return (
    <Modal opened={pairing !== null} onClose={onClose} title="別の PC へ入れ替える" centered>
      {pairing && (
        <Stack gap="md">
          <Text size="sm">新しい PC のデスクトップ版で「サーバーに接続」を開き、貼り付けてください。</Text>
          <Group justify="flex-end"><CopyPairingButton pairing={pairing} /></Group>
          <Text size="xs" c="dimmed">{Math.round(pairing.expiresInSeconds / 60)} 分有効で、一度だけ使えます。人に見せないでください。</Text>
        </Stack>
      )}
    </Modal>
  );
}

function ReaderPairing({ publicUrl, readers, onClose }: { publicUrl: string | null; readers: Device[]; onClose: () => void }) {
  const [register, setRegister] = useState(false);
  const [known, setKnown] = useState<Set<string> | null>(null);
  const [pairing, setPairing] = useState<Pairing | null>(null);
  const [expired, setExpired] = useState(false);
  const [url, setUrl] = useState(window.location.protocol === "https:" ? window.location.origin : "");
  const [problem, setProblem] = useState<string | null>(null);
  const issue = useMutation({
    mutationFn: (address: string) => adminApi.addReader(address),
    onMutate: () => {
      setKnown(new Set(readers.map(({ id }) => id)));
      setProblem(null);
      setExpired(false);
    },
    onSuccess: setPairing,
    onError: (error) => {
      notifyIfExpired(error);
      setProblem(failureMessage(error));
    },
  });
  const issuePairing = () => {
    const address = publicUrl ?? url.trim();
    const target = URL.canParse(address) ? new URL(address) : null;
    if (!target || target.protocol !== "https:" || target.username || target.password || target.search || target.hash) {
      setProblem("携帯が接続できる HTTPS のアドレスを入力してください。");
      return;
    }
    issue.mutate(target.href);
  };
  useEffect(() => {
    if (!pairing) return;
    const timer = window.setTimeout(() => { setPairing(null); setExpired(true); }, pairing.expiresInSeconds * 1_000);
    return () => window.clearTimeout(timer);
  }, [pairing]);
  const added = (pairing || expired) && known && readers.find(({ id }) => !known.has(id));

  return (
    <Modal opened onClose={onClose} title="携帯を追加" centered>
      <Stack gap="md">
        {added ? (
          <>
            <Text size="sm">「{added.name || "名前なし"}」が端末一覧に表示されました。{added.readsShelf === undefined ? "ライブラリの配信が始まったら、一覧で閲覧を許可してください。" : "本棚を読めるようにするには、一覧で閲覧を許可してください。"}</Text>
            <Group justify="flex-end"><Button onClick={onClose}>端末一覧へ</Button></Group>
          </>
        ) : !register ? (
          <>
            <Text size="sm" fw={650}>Android 版を入れる</Text>
            <Text size="sm">携帯のブラウザで配布ページを開き、APK をダウンロードしてください。</Text>
            <div className="adm-qr"><QRCodeSVG value={ANDROID_DOWNLOAD_URL} size={240} level="M" aria-label="Android 版の配布ページ" /></div>
            <Anchor href={ANDROID_DOWNLOAD_URL} target="_blank" rel="noreferrer" size="sm">Android 版の配布ページを開く</Anchor>
            <Group justify="flex-end"><Button onClick={() => { setRegister(true); if (publicUrl && !pairing) issuePairing(); }}>登録に進む</Button></Group>
          </>
        ) : (
          <>
            <Text size="sm">Android 版の「設定 → サーバーとの接続 → QR を読む」で登録します。同じ端末で管理画面を開いている場合は、登録文字列をコピーして貼り付けてください。</Text>
            {!publicUrl && !pairing && (
              <TextInput type="url" label="携帯が接続する HTTPS アドレス" value={url} onChange={(event) => setUrl(event.currentTarget.value)} />
            )}
            {problem && <Alert color="red" role="alert">{problem}</Alert>}
            {expired && <Text size="sm" role="status">登録文字列の有効期限が切れました。</Text>}
            {pairing && (
              <>
                <div className="adm-qr"><QRCodeSVG value={pairing.payload} size={240} level="M" aria-label="登録用の QR" /></div>
                <Text size="xs" c="dimmed">{Math.round(pairing.expiresInSeconds / 60)} 分間有効で、一度だけ使えます。人に見せないでください。</Text>
              </>
            )}
            <Group justify="space-between">
              <Button variant="subtle" disabled={issue.isPending} onClick={() => setRegister(false)}>戻る</Button>
              {pairing ? <CopyPairingButton pairing={pairing} /> : (
                <Button loading={issue.isPending} disabled={!publicUrl && !url.trim()} onClick={issuePairing}>
                  {expired || problem ? "登録文字列を作り直す" : "登録文字列を作る"}
                </Button>
              )}
            </Group>
          </>
        )}
      </Stack>
    </Modal>
  );
}

interface DeviceTableProps {
  devices: Device[];
  reader?: boolean;
  accessPending?: boolean;
  onAccess: (device: Device, read: boolean) => void;
  onReplace: () => void;
  onRevoke: (device: Device) => void;
}

function DeviceTable({ devices, reader, accessPending, onAccess, onReplace, onRevoke }: DeviceTableProps) {
  return (
    <div className="adm-table-wrap">
      <table className="adm-table adm-device-table">
        <thead>
          <tr>
            <th>名前</th>
            {reader && <th>ライブラリの閲覧</th>}
            <th>登録</th>
            <th>最後の接続</th>
            <th aria-label="操作" />
          </tr>
        </thead>
        <tbody>
          {devices.map((device) => (
              <tr key={device.id}>
                <td><span className="adm-strong adm-clamp">{device.name || "名前なし"}</span></td>
                {reader && (
                  <td data-label="閲覧">
                    {device.readsShelf === undefined ? <Text span size="sm" c="dimmed">配信待ち</Text> : (
                      <Switch
                        className="adm-reader-access"
                        size="sm"
                        aria-label={`「${device.name || "名前なし"}」のライブラリ閲覧を許可`}
                        label={device.readsShelf ? "許可" : "未許可"}
                        checked={device.readsShelf}
                        disabled={accessPending}
                        onChange={(event) => onAccess(device, event.currentTarget.checked)}
                      />
                    )}
                  </td>
                )}
                <td data-label="登録" className="adm-num">{dayTime(new Date(device.createdAt))}</td>
                <td data-label="最後の接続" className="adm-num">{device.lastSeenAt ? relativeTime(device.lastSeenAt) : "まだ接続していません"}</td>
                <td className="adm-cell-action">
                  {reader
                    ? <DangerButton size="compact-sm" onClick={() => onRevoke(device)}>登録を解除</DangerButton>
                    : <Button size="compact-sm" variant="default" onClick={onReplace}>別の PC へ入れ替える</Button>}
                </td>
              </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function DevicesPage() {
  const [adding, setAdding] = useState(false);
  const status = useStatus(adding ? 3_000 : undefined);
  const client = useQueryClient();
  const { confirmed } = useReauth();
  const [admin, setAdmin] = useState<Pairing | null>(null);
  const readers = status.data?.devices.filter((device) => device.role === "reader") ?? [];
  const computers = status.data?.devices.filter((device) => device.role === "admin") ?? [];
  const access = useMutation({
    mutationFn: ({ id, read }: { id: string; read: boolean }) => adminApi.setReaderAccess(id, read),
    onSuccess: () => client.invalidateQueries({ queryKey: ["status"] }),
    onError: showFailure,
  });
  const url = window.location.origin;
  const replace = () => {
    modals.openConfirmModal({
      title: "別の PC へ入れ替えますか",
      children: <Text size="sm">新しい登録文字列を出します。それを別の PC のデスクトップ版に貼り付けると、今の PC は接続できなくなります。使わなければ何も変わりません。</Text>,
      labels: { confirm: "登録文字列を出す", cancel: "キャンセル" },
      onConfirm: () => {
        void confirmed(() => adminApi.replaceAdmin(url)).then((pairing) => { if (pairing) setAdmin(pairing); }).catch(showFailure);
      },
    });
  };
  const revoke = (device: Device) => {
    modals.openConfirmModal({
      title: `${device.name} の登録を解除しますか`,
      children: <Text size="sm">この携帯はすぐにサーバーを読めなくなります。もう一度使うには、登録し直します。</Text>,
      labels: { confirm: "登録を解除", cancel: "キャンセル" },
      confirmProps: DANGER_CONFIRM,
      onConfirm: () => {
        void adminApi.revokeDevice(device.id).then(() => client.invalidateQueries({ queryKey: ["status"] })).catch(showFailure);
      },
    });
  };
  const tableActions = {
    onAccess: (device: Device, read: boolean) => access.mutate({ id: device.id, read }),
    onReplace: replace,
    onRevoke: revoke,
  };
  return (
    <div className="adm-section">
      <PageHead title="端末" lead="携帯の登録と閲覧許可、ライブラリを送る PC の登録を管理します。">
        <Button leftSection={<Icons.add size={IconSize.action} />} disabled={!status.data || status.isError} onClick={() => setAdding(true)}>携帯を追加</Button>
      </PageHead>
      {status.isError && <Alert color="red" role="alert">端末の状況を確認できませんでした。</Alert>}
      <Card title="携帯" lead={status.data ? `${readers.length} 台` : undefined} flush>
        {status.data && <Text className="adm-card-pad adm-device-help" size="sm" c="dimmed">
          {status.data.shelf === null
            ? "配信を始めるには、デスクトップ版の「設定 → サーバー」で「今すぐ送る」を押してください。"
            : "本棚を読めるようにするには、登録した携帯の閲覧を許可してください。"}
        </Text>}
        {!status.data ? <div className="adm-card-pad"><Skeleton height={64} /></div> : readers.length === 0 ? <Text className="adm-card-pad" c="dimmed">登録した携帯はありません。</Text> : (
          <DeviceTable {...tableActions} devices={readers} reader accessPending={access.isPending || status.isError} />
        )}
      </Card>
      <Card title="送信元の PC" lead={status.data ? `${computers.length} 台` : undefined} flush>
        {!status.data ? <div className="adm-card-pad"><Skeleton height={64} /></div> : computers.length === 0 ? <Text className="adm-card-pad" c="dimmed">登録した PC はありません。</Text> : (
          <DeviceTable {...tableActions} devices={computers} />
        )}
      </Card>
      {adding && status.data && <ReaderPairing publicUrl={status.data.publicUrl} readers={readers} onClose={() => { setAdding(false); void client.invalidateQueries({ queryKey: ["status"] }); }} />}
      <PairingDialog pairing={admin} onClose={() => setAdmin(null)} />
    </div>
  );
}
