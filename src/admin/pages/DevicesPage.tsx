import { useState } from "react";
import { ActionIcon, Alert, Button, Group, Menu, Modal, Skeleton, Stack, Text } from "@mantine/core";
import { modals } from "@mantine/modals";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Icons } from "@/lib/icons";
import { QRCodeSVG } from "qrcode.react";
import { DANGER_CONFIRM } from "@/components/DangerButton";
import { adminApi, failureMessage, type AdminSession, type Device, type Pairing } from "../api";
import { dayTime, relativeTime } from "../format";
import { Card, CopyButton, PageHead, showFailure, useReauth, useStatus } from "../shared";

/** ブラウザの名前を読める形にする。正確さより見分けやすさを優先する。 */
export function browserLabel(agent: string | null): string {
  if (!agent) return "不明なブラウザ";
  const browser = /Edg\//.test(agent) ? "Edge" : /Firefox\//.test(agent) ? "Firefox" : /Chrome\//.test(agent) ? "Chrome" : /Safari\//.test(agent) ? "Safari" : "ブラウザ";
  const system = /Android/.test(agent) ? "Android" : /iPhone|iPad/.test(agent) ? "iOS" : /Windows/.test(agent) ? "Windows" : /Mac OS/.test(agent) ? "Mac" : /Linux/.test(agent) ? "Linux" : null;
  return system ? `${browser}（${system}）` : browser;
}

function PairingDialog({ title, note, pairing, qr, onClose }: { title: string; note: string; pairing: Pairing | null; qr: boolean; onClose: () => void }) {
  return (
    <Modal opened={pairing !== null} onClose={onClose} title={title} centered>
      {pairing && (
        <Stack gap="md">
          <Text size="sm">{note}</Text>
          {qr && <div className="adm-qr"><QRCodeSVG value={pairing.payload} size={240} level="M" aria-label="登録の QR コード" /></div>}
          <Group gap={6} wrap="nowrap" align="flex-start">
            <Text className="adm-code" style={{ flex: 1, minWidth: 0 }}>{pairing.payload}</Text>
            <CopyButton value={pairing.payload} label="登録文字列をコピー" />
          </Group>
          <Text size="xs" c="dimmed">{Math.round(pairing.expiresInSeconds / 60)} 分有効で、一度だけ使えます。人に見せないでください。</Text>
        </Stack>
      )}
    </Modal>
  );
}

function DeviceRow({ device, onReplace, onRevoke }: { device: Device; onReplace: () => void; onRevoke: () => void }) {
  const admin = device.role === "admin";
  return (
    <div className="adm-row">
      <div className="adm-row-body">
        <div style={{ fontWeight: 600, display: "-webkit-box", WebkitLineClamp: 2, WebkitBoxOrient: "vertical", overflow: "hidden" }}>{device.name || "名前なし"}</div>
        <div className="adm-row-sub">{admin ? "管理者（デスクトップ版）" : "携帯"}・登録 {dayTime(new Date(device.createdAt))}</div>
      </div>
      <div className="adm-row-side">
        <span>{device.lastSeenAt ? `最後の接続 ${relativeTime(device.lastSeenAt)}` : "まだ接続していません"}</span>
        <Menu position="bottom-end" width={220}>
          <Menu.Target><ActionIcon size={28} variant="subtle" color="gray" aria-label={`${device.name}の操作`}><Icons.more size={16} /></ActionIcon></Menu.Target>
          <Menu.Dropdown>
            {admin
              ? <Menu.Item onClick={onReplace}>別の PC へ入れ替える</Menu.Item>
              : <Menu.Item color="red" onClick={onRevoke}>登録を解除</Menu.Item>}
          </Menu.Dropdown>
        </Menu>
      </div>
    </div>
  );
}

function SessionRow({ session, current, onEnd }: { session: AdminSession; current: boolean; onEnd: () => void }) {
  return (
    <div className="adm-row">
      <div className="adm-row-body">
        <div style={{ fontWeight: 600 }}>{browserLabel(session.label)}{current ? "・このブラウザ" : ""}</div>
        <div className="adm-row-sub">{session.ip ? `${session.ip}・` : ""}ログイン {relativeTime(session.createdAt)}</div>
      </div>
      <div className="adm-row-side">
        <span>最後の操作 {relativeTime(session.lastSeenAt)}</span>
        {!current && <Button size="compact-xs" variant="default" onClick={onEnd}>切る</Button>}
      </div>
    </div>
  );
}

export function DevicesPage() {
  const status = useStatus();
  const client = useQueryClient();
  const { confirmed } = useReauth();
  const sessions = useQuery({ queryKey: ["sessions"], queryFn: adminApi.sessions, refetchInterval: 30_000 });
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
  const endSession = (id: string) => {
    void adminApi.endSession(id).then(() => client.invalidateQueries({ queryKey: ["sessions"] })).catch(showFailure);
  };
  return (
    <div className="adm-section">
      <PageHead title="端末">
        <Button size="xs" variant="default" leftSection={<Icons.add size={14} />} loading={adding} onClick={() => void addReader()}>携帯を追加</Button>
      </PageHead>
      {problem && <Alert color="red" variant="light" withCloseButton onClose={() => setProblem(null)}>{problem}</Alert>}
      <Card title="登録している端末">
        {!status.data ? <Skeleton height={80} /> : status.data.devices.length === 0 ? <Text size="sm" c="dimmed">まだありません。</Text> : (
          <div className="adm-rows">
            {status.data.devices.map((device) => <DeviceRow key={device.id} device={device} onReplace={replace} onRevoke={() => revoke(device)} />)}
          </div>
        )}
      </Card>
      <Card title="ログイン中のブラウザ">
        {sessions.isPending ? <Skeleton height={60} /> : (
          <div className="adm-rows">
            {(sessions.data?.sessions ?? []).map((session) => <SessionRow key={session.id} session={session} current={session.id === sessions.data?.currentId} onEnd={() => endSession(session.id)} />)}
          </div>
        )}
      </Card>
      <PairingDialog title="携帯を追加" note="携帯のアプリで QR を読むか、登録文字列を貼り付けてください。" pairing={reader} qr onClose={() => { setReader(null); void client.invalidateQueries({ queryKey: ["status"] }); }} />
      <PairingDialog title="別の PC へ入れ替える" note="新しい PC のデスクトップ版で「サーバーに接続」を開き、貼り付けてください。" pairing={admin} qr={false} onClose={() => setAdmin(null)} />
    </div>
  );
}
