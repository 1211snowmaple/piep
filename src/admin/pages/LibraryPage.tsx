import { Button, Skeleton, Text } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { adminApi } from "../api";
import { bytes, relativeTime } from "../format";
import { Card, Facts, PageHead, showFailure, useStatus } from "../shared";

export function LibraryPage() {
  const status = useStatus();
  const client = useQueryClient();
  const storage = useQuery({ queryKey: ["storage"], queryFn: adminApi.storage, staleTime: 60_000 });
  const data = status.data;
  const sync = data?.sync;
  const reindex = async () => {
    try {
      await adminApi.reindex();
      notifications.show({ message: "索引を確かめて、足りない分を作ります。" });
      await client.invalidateQueries({ queryKey: ["status"] });
    } catch (error) {
      showFailure(error);
    }
  };
  return (
    <div className="adm-section">
      <PageHead title="ライブラリ" lead="このサーバーではライブラリを変更しません。変更はデスクトップ版から送ります。" />
      <Card title="受け取った内容">
        {!data ? <Skeleton height={80} /> : (
          <Facts items={[
            ["作品", data.works === null ? "まだ受け取っていません" : `${data.works.toLocaleString("ja-JP")} 作品`],
            ["受け取った回数", sync ? `${sync.generation.toLocaleString("ja-JP")} 回` : "なし"],
            ["最後に受け取った", relativeTime(sync?.committedAt)],
            ["今の受信", sync?.sessionOpen ? "受信しています" : "なし"],
          ]} />
        )}
      </Card>
      <Card title="容量">
        {storage.isPending ? <Skeleton height={110} /> : storage.isError || !storage.data ? <Text size="sm" c="dimmed">読み取れませんでした。</Text> : (
          <Facts items={[
            ["本文と画像", bytes(storage.data.contentBytes)],
            ["データベース", bytes(storage.data.databaseBytes)],
            ["検索の索引", bytes(storage.data.indexBytes)],
            ["削除待ちのファイル", bytes(storage.data.trashBytes)],
            ["ドライブの空き", bytes(storage.data.freeBytes)],
          ]} />
        )}
      </Card>
      <Card title="検索の索引" side={<Button variant="default" fw={500} onClick={() => void reindex()}>確かめて作る</Button>}>
        {!data ? <Skeleton height={50} /> : (
          <Facts items={[
            ["残り", sync?.indexPending === null || sync?.indexPending === undefined ? "確認中" : sync.indexPending === 0 ? "なし" : `${sync.indexPending.toLocaleString("ja-JP")} 件`],
            ["前回の問題", sync?.indexError ?? "なし"],
          ]} />
        )}
      </Card>
    </div>
  );
}
