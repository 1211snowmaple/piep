import { Skeleton, Text } from "@mantine/core";
import { useQuery } from "@tanstack/react-query";
import { adminApi } from "../api";
import { bytes, shipmentLabel } from "../format";
import { Card, Facts, PageHead, useStatus } from "../shared";

export function LibraryPage() {
  const status = useStatus();
  const storage = useQuery({ queryKey: ["storage"], queryFn: adminApi.storage, staleTime: 60_000 });
  const data = status.data;
  const shipment = data?.shipment;
  return (
    <div className="adm-section">
      <PageHead title="ライブラリ" lead="このサーバーではライブラリを変更しません。変更はデスクトップ版から送ります。" />
      <Card title="配信中のライブラリ">
        {!data ? <Skeleton height={80} /> : (
          <Facts items={[
            ["作品", data.shelf ? `${Number(data.shelf.works).toLocaleString("ja-JP")} 作品` : "まだ受け取っていません"],
            ["受け取り", shipmentLabel(shipment)],
            ...(shipment?.error ? [["前回の問題", shipment.error] as [string, string]] : []),
            ["棚の UUID", data.shelf?.libraryId ?? "なし"],
            ["同期の世代", data.shelf?.syncEpoch ?? "なし"],
          ]} />
        )}
      </Card>
      <Card title="容量">
        {storage.isPending ? <Skeleton height={110} /> : storage.isError || !storage.data ? <Text size="sm" c="dimmed">読み取れませんでした。</Text> : (
          <Facts items={[
            ["本文と画像", bytes(storage.data.contentBytes)],
            ["台帳", bytes(storage.data.databaseBytes)],
            ["検索の索引など", bytes(storage.data.indexBytes)],
            ["ドライブの空き", bytes(storage.data.freeBytes)],
          ]} />
        )}
      </Card>
    </div>
  );
}
