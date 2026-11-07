import { Button, Checkbox, Group, SegmentedControl, Skeleton, Text } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { useInfiniteQuery } from "@tanstack/react-query";
import { useState } from "react";
import { Icons, IconSize } from "@/lib/icons";
import { adminApi, type AuditEntry } from "../api";
import { stamp } from "../format";
import { Card, PageHead } from "../shared";

const KINDS = [
  { value: "all", label: "すべて" },
  { value: "update", label: "更新" },
  { value: "login", label: "ログイン" },
  { value: "device", label: "端末" },
  { value: "library", label: "ライブラリ" },
];

const KIND_LABEL: Record<string, string> = {
  update: "更新",
  login: "ログイン",
  device: "端末",
  library: "ライブラリ",
  system: "サーバー",
};

const PAGE = 200;

function line(entry: AuditEntry): string {
  return [new Date(entry.at).toISOString(), KIND_LABEL[entry.kind] ?? entry.kind, entry.action, entry.ok ? "成功" : "失敗", entry.actor, entry.ip ?? "", entry.detail ?? ""].join("\t");
}

export function RecordsPage() {
  const [kind, setKind] = useState("all");
  const [failures, setFailures] = useState(false);
  const records = useInfiniteQuery({
    queryKey: ["audit", kind, failures],
    initialPageParam: null as number | null,
    queryFn: ({ pageParam }) => adminApi.audit(kind === "all" ? null : kind, failures, pageParam),
    getNextPageParam: (last) => (last.length === PAGE ? last[last.length - 1]?.id ?? null : null),
  });
  const entries = records.data?.pages.flat() ?? [];
  const copy = () => {
    const head = ["日時", "種類", "出来事", "結果", "操作した人", "接続元", "詳しく"].join("\t");
    void navigator.clipboard.writeText([head, ...entries.map(line)].join("\n")).then(
      () => notifications.show({ message: `${entries.length} 行をコピーしました。表計算に貼り付けられます。` }),
      () => notifications.show({ color: "red", message: "コピーできませんでした" }),
    );
  };
  return (
    <div className="adm-section">
      <PageHead title="記録" lead="ログイン、更新、端末の登録など、このサーバーで起きたことです。新しい順に並びます。" />
      <Card flush>
        <div className="adm-toolbar">
          <Group gap="md" wrap="wrap" className="adm-toolbar-filters">
            <SegmentedControl value={kind} onChange={setKind} data={KINDS} aria-label="種類" />
            <Checkbox label="失敗だけ" checked={failures} onChange={(event) => setFailures(event.currentTarget.checked)} />
          </Group>
          <Button variant="default" leftSection={<Icons.copy size={IconSize.menu} />} disabled={entries.length === 0} onClick={copy}>表をコピー</Button>
        </div>
        {records.isPending ? <div className="adm-card-pad"><Skeleton height={200} /></div> : entries.length === 0 ? <Text className="adm-card-pad" c="dimmed">該当するものはありません。</Text> : (
          <div className="adm-table-wrap">
            <table className="adm-table">
              <thead>
                <tr>
                  <th className="adm-col-time">日時</th>
                  <th>出来事</th>
                  <th>種類</th>
                  <th>操作した人</th>
                  <th>接続元</th>
                </tr>
              </thead>
              <tbody>
                {entries.map((entry) => (
                  <tr key={entry.id}>
                    <td data-label="日時" className="adm-num adm-col-time">{stamp(new Date(entry.at))}</td>
                    <td data-label="出来事">
                      <div className="adm-event">
                        {!entry.ok && <span className="adm-failed">失敗</span>}
                        <span className="adm-strong">{entry.action}</span>
                      </div>
                      {entry.detail && <div className="adm-cell-sub">{entry.detail}</div>}
                    </td>
                    <td data-label="種類" className="adm-nowrap">{KIND_LABEL[entry.kind] ?? entry.kind}</td>
                    <td data-label="操作した人" className="adm-nowrap">{entry.actor}</td>
                    <td data-label="接続元" className="adm-num">{entry.ip ?? "—"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
        {records.hasNextPage && (
          <div className="adm-table-foot">
            <Button variant="default" loading={records.isFetchingNextPage} onClick={() => void records.fetchNextPage()}>さらに古い記録を読む</Button>
          </div>
        )}
      </Card>
    </div>
  );
}
