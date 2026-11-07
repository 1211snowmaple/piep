import { useEffect, useRef, useState } from "react";
import { ActionIcon, Button, Group, Menu, Skeleton, Switch, Text, TextInput } from "@mantine/core";
import { modals } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Icons, IconSize } from "@/lib/icons";
import { DANGER_CONFIRM } from "@/components/DangerButton";
import { adminApi, type UpdateHistoryEntry } from "../api";
import { isNewer, relativeTime, stamp, windowLabel } from "../format";
import { Card, PageHead, showFailure, useReauth, useStatus } from "../shared";

const RESULT_LABEL: Record<UpdateHistoryEntry["result"], string> = {
  applied: "更新しました",
  failed: "起動せず戻しました",
  rolled_back: "手で戻しました",
};

function downtime(seconds: number | null): string | null {
  if (seconds === null) return null;
  return seconds < 60 ? `${seconds} 秒` : `${Math.round(seconds / 60)} 分`;
}

/** 時刻の欄。時計は左に置き、欄のどこを押しても選べるようにする。 */
function TimeField({ label, value, disabled, onChange, onCommit }: { label: string; value: string; disabled: boolean; onChange: (value: string) => void; onCommit: () => void }) {
  const input = useRef<HTMLInputElement>(null);
  return (
    <TextInput
      ref={input}
      type="time"
      label={label}
      w={150}
      className="adm-time"
      leftSection={<Icons.pending size={IconSize.action} />}
      disabled={disabled}
      value={value}
      onChange={(event) => onChange(event.currentTarget.value)}
      onClick={() => {
        try {
          input.current?.showPicker();
        } catch {
          // 選ぶ窓を出せないブラウザでは、打って入れる。
        }
      }}
      onBlur={onCommit}
    />
  );
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
      <Text c="dimmed" mb="md">
        {update.auto ? `新しいバージョンがあれば、${windowLabel(update.window)} の間に更新します。` : "押したときしか更新しません。新しいバージョンの確認は続けます。"}
      </Text>
      <Group gap="sm" wrap="nowrap" align="flex-end">
        <TimeField label="開始" disabled={!update.auto} value={start} onChange={setStart} onCommit={commitWindow} />
        <span className="adm-time-sep" aria-hidden>〜</span>
        <TimeField label="終了" disabled={!update.auto} value={end} onChange={setEnd} onCommit={commitWindow} />
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
      <PageHead title="更新" lead="サーバーのバージョンと、自動で更新する時間帯です。" />
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
                {fresh && <Button onClick={() => void apply()} disabled={update.phase.state === "applying"}>今すぐ更新</Button>}
                <Button variant="default" loading={checking} onClick={() => void check()}>更新を確認</Button>
                <Menu position="bottom-end" width={220}>
                  <Menu.Target><ActionIcon size={36} variant="default" aria-label="その他"><Icons.more size={IconSize.menu} /></ActionIcon></Menu.Target>
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
                <Button variant="default" style={{ flex: "none" }} onClick={() => void retry()}>もう一度試す</Button>
              </Group>
            </Card>
          )}
          <AutoCard />
          <Card title="履歴" flush>
            {history.isPending ? <div className="adm-card-pad"><Skeleton height={60} /></div> : (history.data ?? []).length === 0 ? <Text className="adm-card-pad" c="dimmed">まだ更新していません。</Text> : (
              <div className="adm-table-wrap">
                <table className="adm-table">
                  <thead>
                    <tr>
                      <th className="adm-col-time">日時</th>
                      <th>バージョン</th>
                      <th>結果</th>
                      <th>止まった時間</th>
                      <th>理由</th>
                    </tr>
                  </thead>
                  <tbody>
                    {(history.data ?? []).map((entry) => (
                      <tr key={`${entry.at}-${entry.to}`}>
                        <td data-label="日時" className="adm-num adm-col-time">{stamp(new Date(entry.at))}</td>
                        <td data-label="バージョン" className="adm-num">{entry.from ?? "不明"} → {entry.to}</td>
                        <td data-label="結果">{entry.result === "applied" ? RESULT_LABEL[entry.result] : <span className="adm-event"><span className="adm-failed">戻した</span>{RESULT_LABEL[entry.result]}</span>}</td>
                        <td data-label="止まった時間" className="adm-num">{downtime(entry.downtimeSeconds) ?? "—"}</td>
                        <td data-label="理由">{entry.reason ?? "—"}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </Card>
        </>
      )}
    </div>
  );
}
