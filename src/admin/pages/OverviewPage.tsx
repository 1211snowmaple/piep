import { Button, Group, Skeleton, Text } from "@mantine/core";
import { useQuery } from "@tanstack/react-query";
import { adminApi } from "../api";
import { bytes, findIssues, relativeTime, type AdminPage } from "../format";
import { Card, PageHead, useStatus } from "../shared";

function NumberTile({ label, value }: { label: string; value: string | undefined }) {
  return (
    <div className="adm-number">
      <div className="adm-number-label">{label}</div>
      <div className="adm-number-value">{value ?? <Skeleton height={26} width="70%" />}</div>
    </div>
  );
}

export function OverviewPage({ go }: { go: (page: AdminPage) => void }) {
  const status = useStatus();
  const storage = useQuery({ queryKey: ["storage"], queryFn: adminApi.storage, staleTime: 60_000 });
  const recent = useQuery({ queryKey: ["audit", "recent"], queryFn: () => adminApi.audit(null, false, null), refetchInterval: 30_000 });
  const data = status.data;
  const issues = data ? findIssues(data, storage.data) : [];
  return (
    <div className="adm-section">
      <PageHead title="概要" lead="サーバーの今の状態です。" />
      <section className="sv-card" aria-label="サーバーの状況" style={{ padding: 20 }}>
        {!data ? <Skeleton height={32} width="50%" /> : issues.length === 0 ? (
          <Group gap={10} wrap="nowrap">
            <span className="sv-dot" data-tone="ok" aria-hidden />
            <div className="sv-head" style={{ fontSize: 22 }}>問題はありません</div>
          </Group>
        ) : (
          <div className="adm-rows">
            {issues.map((issue) => (
              <div key={issue.message} className="adm-row">
                <Group gap={10} wrap="nowrap" className="adm-row-body">
                  <span className="sv-dot" data-tone="bad" aria-hidden />
                  <span style={{ fontWeight: 600 }}>{issue.message}</span>
                </Group>
                <Button variant="default" onClick={() => go(issue.page)}>開く</Button>
              </div>
            ))}
          </div>
        )}
      </section>
      <div className="adm-numbers">
        <NumberTile label="バージョン" value={data?.version} />
        <NumberTile label="ライブラリ" value={data ? (data.works === null ? "未受信" : `${data.works.toLocaleString("ja-JP")} 作品`) : undefined} />
        <NumberTile label="空き容量" value={storage.data ? bytes(storage.data.freeBytes) : storage.isError ? "不明" : undefined} />
        <NumberTile label="最後に受け取った" value={data ? (data.sync?.committedAt ? relativeTime(data.sync.committedAt) : "なし") : undefined} />
      </div>
      <Card title="最近の出来事" side={<Button variant="default" onClick={() => go("records")}>記録を開く</Button>} flush>
        {recent.isPending ? <div className="adm-card-pad"><Skeleton height={100} /></div> : (recent.data ?? []).length === 0 ? <Text className="adm-card-pad" c="dimmed">まだ何もありません。</Text> : (
          <div className="adm-table-wrap">
            <table className="adm-table" aria-label="最近の出来事">
              <tbody>
                {(recent.data ?? []).slice(0, 6).map((entry) => (
                  <tr key={entry.id}>
                    <td className="adm-num adm-col-time">{relativeTime(entry.at)}</td>
                    <td>
                      <div className="adm-event">
                        {!entry.ok && <span className="adm-failed">失敗</span>}
                        <span>{entry.action}</span>
                      </div>
                    </td>
                    <td className="adm-dim">{entry.actor}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Card>
    </div>
  );
}
