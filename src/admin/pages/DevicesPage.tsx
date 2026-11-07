import { useState } from "react";
import { Alert, Button, Group, Modal, Skeleton, Stack, Text } from "@mantine/core";
import { modals } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { useQueryClient } from "@tanstack/react-query";
import { Icons, IconSize } from "@/lib/icons";
import { QRCodeSVG } from "qrcode.react";
import { DANGER_CONFIRM, DangerButton } from "@/components/DangerButton";
import { adminApi, failureMessage, type Device, type Pairing } from "../api";
import { dayTime, relativeTime } from "../format";
import { Card, PageHead, showFailure, useReauth, useStatus } from "../shared";

function PairingDialog({ title, note, pairing, qr, onClose }: { title: string; note: string; pairing: Pairing | null; qr: boolean; onClose: () => void }) {
  return (
    <Modal opened={pairing !== null} onClose={onClose} title={title} centered>
      {pairing && (
        <Stack gap="md">
          <Text size="sm">{note}</Text>
          {qr && <div className="adm-qr"><QRCodeSVG value={pairing.payload} size={240} level="M" aria-label="登録の QR コード" /></div>}
          <Group justify="center">
            <Button
              variant="default"
              leftSection={<Icons.copy size={IconSize.menu} />}
              onClick={() => {
                void navigator.clipboard.writeText(pairing.payload).then(
                  () => notifications.show({ message: "コピーしました" }),
                  () => notifications.show({ color: "red", message: "コピーできませんでした" }),
                );
              }}
            >登録文字列をコピー</Button>
          </Group>
          <Text size="xs" c="dimmed">{Math.round(pairing.expiresInSeconds / 60)} 分有効で、一度だけ使えます。人に見せないでください。</Text>
        </Stack>
      )}
    </Modal>
  );
}

function DeviceTable({ devices, onReplace, onRevoke }: { devices: Device[]; onReplace: () => void; onRevoke: (device: Device) => void }) {
  return (
    <div className="adm-table-wrap">
      <table className="adm-table">
        <thead>
          <tr>
            <th>名前</th>
            <th>種類</th>
            <th>登録</th>
            <th>最後の接続</th>
            <th aria-label="操作" />
          </tr>
        </thead>
        <tbody>
          {devices.map((device) => {
            const admin = device.role === "admin";
            return (
              <tr key={device.id}>
                <td data-label="名前"><span className="adm-strong adm-clamp">{device.name || "名前なし"}</span></td>
                <td data-label="種類" className="adm-nowrap">{admin ? "管理者（デスクトップ版）" : "携帯"}</td>
                <td data-label="登録" className="adm-num">{dayTime(new Date(device.createdAt))}</td>
                <td data-label="最後の接続" className="adm-num">{device.lastSeenAt ? relativeTime(device.lastSeenAt) : "まだ接続していません"}</td>
                <td className="adm-cell-action">
                  {admin
                    ? <Button size="compact-sm" variant="default" onClick={onReplace}>別の PC へ入れ替える</Button>
                    : <DangerButton size="compact-sm" onClick={() => onRevoke(device)}>登録を解除</DangerButton>}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

export function DevicesPage() {
  const status = useStatus();
  const client = useQueryClient();
  const { confirmed } = useReauth();
  const [reader, setReader] = useState<Pairing | null>(null);
  const [admin, setAdmin] = useState<Pairing | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const url = window.location.origin;
  const addReader = async () => {
    setAdding(true);
    setProblem(null);
    try {
      setReader(await adminApi.addReader(url));
    } catch (error) {
      setProblem(failureMessage(error));
    } finally {
      setAdding(false);
    }
  };
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
  return (
    <div className="adm-section">
      <PageHead title="端末" lead="このサーバーを読む携帯と、本棚を送る管理者の PC です。">
        <Button leftSection={<Icons.add size={IconSize.action} />} loading={adding} onClick={() => void addReader()}>携帯を追加</Button>
      </PageHead>
      {problem && <Alert color="red" variant="light" withCloseButton onClose={() => setProblem(null)}>{problem}</Alert>}
      <Card title="登録している端末" lead={status.data ? `${status.data.devices.length} 台` : undefined} flush>
        {!status.data ? <div className="adm-card-pad"><Skeleton height={80} /></div> : status.data.devices.length === 0 ? <Text className="adm-card-pad" c="dimmed">まだありません。</Text> : (
          <DeviceTable devices={status.data.devices} onReplace={replace} onRevoke={revoke} />
        )}
      </Card>
      <PairingDialog title="携帯を追加" note="携帯のアプリで QR を読むか、登録文字列を貼り付けてください。" pairing={reader} qr onClose={() => { setReader(null); void client.invalidateQueries({ queryKey: ["status"] }); }} />
      <PairingDialog title="別の PC へ入れ替える" note="新しい PC のデスクトップ版で「サーバーに接続」を開き、貼り付けてください。" pairing={admin} qr={false} onClose={() => setAdmin(null)} />
    </div>
  );
}
