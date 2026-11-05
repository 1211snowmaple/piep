import { useEffect, useState } from "react";
import { ActionIcon, Button, Group, Menu, Skeleton, Stack, Switch, Text, TextInput } from "@mantine/core";
import { modals } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Icons } from "@/lib/icons";
import { DANGER_CONFIRM } from "@/components/DangerButton";
import { adminApi, type UpdateHistoryEntry } from "../api";
import { dayTime, isNewer, relativeTime, windowLabel } from "../format";
import { Card, PageHead, showFailure, useReauth, useStatus } from "../shared";

const RESULT_LABEL: Record<UpdateHistoryEntry["result"], string> = {
  applied: "更新しました",
  failed: "起動せず戻しました",
  rolled_back: "手で戻しました",
};

function downtime(seconds: number | null): string | null {
  if (seconds === null) return null;
  return seconds < 60 ? `${seconds} 秒止まりました` : `${Math.round(seconds / 60)} 分止まりました`;
}

function AutoCard() {
  const status = useStatus();
  const client = useQueryClient();
  const update = status.data?.update;
  const [start, setStart] = useState("03:00");
  const [end, setEnd] = useState("06:00");
  useEffect(() => {
    if (!update) return;
    const [from, to] = update.window.split("-");
    if (from && to) {
      setStart(from);
      setEnd(to);
    }
  }, [update?.window]); // eslint-disable-line react-hooks/exhaustive-deps
  if (!update) return null;
  const save = async (auto: boolean, window: string) => {
    try {
      await adminApi.setUpdateSettings(auto, window);
      await client.invalidateQueries({ queryKey: ["status"] });
    } catch (error) {
      showFailure(error);
    }
  };
  const commitWindow = () => {
    const next = `${start}-${end}`;
    if (start && end && next !== update.window) void save(update.auto, next);
  };
  return (
    <Card
      title="自動で更新する"
      side={<Switch aria-label="自動で更新する" checked={update.auto} onChange={(event) => void save(event.currentTarget.checked, update.window)} />}
    >
      <Text size="sm" c="dimmed" mb="sm">
        {update.auto ? `新しいバージョンがあれば、${windowLabel(update.window)} の間に更新します。` : "押したときしか更新しません。新しいバージョンの確認は続けます。"}
      </Text>
      <Group gap="sm" wrap="nowrap" align="flex-end">
        <TextInput type="time" label="開始" size="sm" w={130} disabled={!update.auto} value={start} onChange={(event) => setStart(event.currentTarget.value)} onBlur={commitWindow} />
        <TextInput type="time" label="終了" size="sm" w={130} disabled={!update.auto} value={end} onChange={(event) => setEnd(event.currentTarget.value)} onBlur={commitWindow} />
      </Group>
    </Card>
  );
}

export function UpdatesPage() {
  const status = useStatus();
  const client = useQueryClient();
  const { confirmed } = useReauth();
  const [checking, setChecking] = useState(false);
  const history = useQuery({ queryKey: ["update-history"], queryFn: adminApi.updateHistory, enabled: Boolean(status.data?.update) });
  const data = status.data;
  const update = data?.update;
  const fresh = data && update ? isNewer(update.latest, data.version) && !update.skipped.includes(update.latest ?? "") : false;
  const check = async () => {
    setChecking(true);
    try {
      await adminApi.checkUpdate();
      await client.invalidateQueries({ queryKey: ["status"] });
    } catch (error) {
      showFailure(error);
    } finally {
      setChecking(false);
    }
  };
  const apply = async () => {
    try {
      await adminApi.applyUpdate();
      notifications.show({ message: "更新を始めました。サーバーは数十秒ほど止まります。" });
      await client.invalidateQueries({ queryKey: ["status"] });
    } catch (error) {
      showFailure(error);
    }
  };
  const retry = async () => {
    try {
      await adminApi.retrySkipped();
      notifications.show({ message: "もう一度試します。" });
      await client.invalidateQueries({ queryKey: ["status"] });
    } catch (error) {
      showFailure(error);
    }
  };
  const rollback = () => {
    modals.openConfirmModal({
      title: `${update?.previous} へ戻しますか`,
      children: <Text size="sm">更新のあとに受け取ったライブラリ、端末の登録、パスワードの変更は、更新の前の状態に戻ります。ライブラリはデスクトップ版から送り直せます。戻したバージョンは、自動では更新し直しません。</Text>,
      labels: { confirm: "前のバージョンへ戻す", cancel: "キャンセル" },
      confirmProps: DANGER_CONFIRM,
      onConfirm: () => {
        void confirmed(() => adminApi.rollback()).then(() => client.invalidateQueries({ queryKey: ["status"] })).catch(showFailure);
      },
    });
  };
  return (
    <div className="adm-section">
      <PageHead title="更新" />
      {!data ? <Skeleton height={200} radius="lg" /> : !update ? (
        <Card>
          <Text size="sm">このサーバーは自分では更新できません。<span className="adm-code">piep-server install</span> で置くと、自動で更新できるようになります。</Text>
        </Card>
      ) : (
        <>
          <section className="sv-card sv-status" aria-label="今の状態">
            <div className="sv-top">
              <div style={{ minWidth: 0 }}>
                <Group gap={10} wrap="nowrap">
                  <span className="sv-dot" data-tone={update.phase.state === "applying" || update.phase.state === "checking" ? "busy" : update.lastError ? "bad" : "ok"} aria-hidden />
                  <div className="sv-head" style={{ fontSize: 22 }}>
                    {update.phase.state === "applying" ? `${update.phase.version} に更新しています` : fresh ? "新しいバージョンがあります" : "最新のバージョンです"}
                  </div>
                </Group>
                {update.phase.state === "waiting" && <Text size="sm" c="dimmed" mt={6}>{update.phase.version} を用意しました。{update.auto ? `${windowLabel(update.window)} に更新します。` : "押すと更新します。"}</Text>}
              </div>
              <Group gap={8} wrap="nowrap" style={{ flex: "none" }}>
                {fresh && <Button size="xs" onClick={() => void apply()} disabled={update.phase.state === "applying"}>今すぐ更新</Button>}
                <Button size="xs" variant="default" loading={checking} onClick={() => void check()}>更新を確認</Button>
                <Menu position="bottom-end" width={220}>
                  <Menu.Target><ActionIcon size={30} variant="default" aria-label="その他"><Icons.more size={16} /></ActionIcon></Menu.Target>
                  <Menu.Dropdown>
                    <Menu.Item color="red" disabled={!update.previous} onClick={rollback}>前のバージョンへ戻す</Menu.Item>
                  </Menu.Dropdown>
                </Menu>
              </Group>
            </div>
            <div className="sv-facts">
              <Text size="sm" c="dimmed">今のバージョン</Text><Text size="sm">{data.version}</Text>
              <Text size="sm" c="dimmed">公開されている最新</Text><Text size="sm">{update.latest ?? "まだ確認していません"}</Text>
              <Text size="sm" c="dimmed">最後に確認</Text><Text size="sm">{relativeTime(update.lastCheckedAt)}</Text>
              {update.lastError && <><Text size="sm" c="dimmed">前回の問題</Text><Text size="sm" style={{ overflowWrap: "anywhere" }}>{update.lastError}</Text></>}
            </div>
          </section>
          {update.skipped.length > 0 && (
            <Card>
              <Group justify="space-between" wrap="nowrap" gap="md">
                <Text size="sm">{update.skipped.join("、")} は起動に失敗したか戻したため、飛ばしています。</Text>
                <Button size="xs" variant="default" style={{ flex: "none" }} onClick={() => void retry()}>もう一度試す</Button>
              </Group>
            </Card>
          )}
          <AutoCard />
          <Card title="履歴">
            {history.isPending ? <Skeleton height={60} /> : (history.data ?? []).length === 0 ? <Text size="sm" c="dimmed">まだ更新していません。</Text> : (
              <Stack gap={0} className="adm-rows">
                {(history.data ?? []).map((entry) => (
                  <div key={`${entry.at}-${entry.to}`} className="adm-row">
                    <div className="adm-row-body">
                      {entry.from ?? "不明"} → {entry.to}・{RESULT_LABEL[entry.result]}
                      {(entry.reason || entry.downtimeSeconds !== null) && <div className="adm-row-sub">{[downtime(entry.downtimeSeconds), entry.reason].filter(Boolean).join("・")}</div>}
                    </div>
                    <div className="adm-row-side">{dayTime(new Date(entry.at))}</div>
                  </div>
                ))}
              </Stack>
            )}
          </Card>
        </>
      )}
    </div>
  );
}
