import { Button, Checkbox, Group, SegmentedControl, Skeleton, Text } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { useInfiniteQuery } from "@tanstack/react-query";
import { useState } from "react";
import { adminApi, type AuditEntry } from "../api";
import { dayTime } from "../format";
import { Card, PageHead } from "../shared";

const KINDS = [
  { value: "all", label: "すべて" },
  { value: "update", label: "更新" },
  { value: "login", label: "ログイン" },
  { value: "device", label: "端末" },
  { value: "library", label: "ライブラリ" },
];

const PAGE = 200;

function line(entry: AuditEntry): string {
  return [new Date(entry.at).toISOString(), entry.action, entry.ok ? "成功" : "失敗", entry.actor, entry.ip ?? "", entry.detail ?? ""].join("\t");
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
    void navigator.clipboard.writeText(entries.map(line).join("\n")).then(
      () => notifications.show({ message: `${entries.length} 行をコピーしました` }),
      () => notifications.show({ color: "red", message: "コピーできませんでした" }),
    );
  };
  return (
    <div className="adm-section">
      <PageHead title="記録">
        <Button size="xs" variant="default" disabled={entries.length === 0} onClick={copy}>コピー</Button>
      </PageHead>
      <Group justify="space-between" gap="sm">
        <SegmentedControl size="xs" value={kind} onChange={setKind} data={KINDS} />
        <Checkbox size="sm" label="失敗だけ" checked={failures} onChange={(event) => setFailures(event.currentTarget.checked)} />
      </Group>
      <Card>
        {records.isPending ? <Skeleton height={200} /> : entries.length === 0 ? <Text size="sm" c="dimmed">該当するものはありません。</Text> : (
          <div className="adm-rows">
            {entries.map((entry) => (
              <div key={entry.id} className="adm-row">
                <div className="adm-row-body">
                  {entry.ok ? entry.action : <>{entry.action}（失敗）</>}
                  <div className="adm-row-sub">{[entry.actor, entry.ip, entry.detail].filter(Boolean).join("・")}</div>
                </div>
                <div className="adm-row-side">{dayTime(new Date(entry.at))}</div>
              </div>
            ))}
          </div>
        )}
        {records.hasNextPage && (
          <Group justify="center" mt="md">
            <Button size="xs" variant="default" loading={records.isFetchingNextPage} onClick={() => void records.fetchNextPage()}>続きを読む</Button>
          </Group>
        )}
      </Card>
    </div>
  );
}
