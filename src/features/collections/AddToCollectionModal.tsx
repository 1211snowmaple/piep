import { useEffect, useMemo, useState } from "react";
import {
  Alert,
  Button,
  Group,
  Modal,
  Radio,
  ScrollArea,
  Stack,
  Text,
  TextInput,
  UnstyledButton,
} from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useAppNavigate } from "@/app/router";
import { EmptyState } from "@/components/AsyncState";
import { CollectionCover } from "@/components/CollectionCover";
import { errorMessage, formatNumber } from "@/lib/format";
import { Icons, IconSize } from "@/lib/icons";
import { addDownloadsToCollection, createCollectionFromDownloads } from "@/services/collectionApi";
import { invalidateCollectionViews, workCollectionsQueryOptions } from "./collectionQueries";
import { isTauriRuntime } from "@/services/dbApi";
import type { CollectionKind } from "@/types/collections";

/**
 * 選んだ作品を束にする。
 *
 * 棚の複数選択から呼ぶことを主に想定している。ここまで、束に作品を入れるには
 * 詳細画面を開いて題名で検索するしかなかった — つまり**入れたい作品の題名を
 * 思い出せることが前提**になっていた。2,000作の棚で成り立つ前提ではない。
 *
 * 渡すのは行の id だけで、識別子への解決と投稿日順の整列は保存側がやる。
 */
export function AddToCollectionModal({
  opened,
  onClose,
  downloadIds,
  defaultName = "",
}: {
  opened: boolean;
  onClose: () => void;
  downloadIds: number[];
  /** 新規作成の名前の初期値。作者ページなら作者名など。 */
  defaultName?: string;
}) {
  const runtime = isTauriRuntime();
  const navigate = useAppNavigate();
  const queryClient = useQueryClient();
  const [target, setTarget] = useState<string>("__new__");
  const [name, setName] = useState("");
  const [kind, setKind] = useState<CollectionKind>("ordered");

  const collectionsQuery = useQuery({ ...workCollectionsQueryOptions(), enabled: opened });
  const collections = collectionsQuery.data ?? [];

  useEffect(() => {
    if (!opened) return;
    setName(defaultName);
    setKind("ordered");
    // 既定は必ず新規。既存のどれかを既定にすると、一覧の読み込みが終わる前と
    // 後で選択が動く — 開いた直後に押した人だけ違う束へ入ることになる。
    setTarget("__new__");
  }, [opened, defaultName]);

  const invalidate = () => invalidateCollectionViews(queryClient);

  const mutation = useMutation({
    mutationFn: async () => {
      if (target === "__new__") {
        return createCollectionFromDownloads(name.trim(), kind, downloadIds);
      }
      return addDownloadsToCollection(target, downloadIds);
    },
    onSuccess: (collection) => {
      invalidate();
      onClose();
      notifications.show({
        color: "green",
        message: `「${collection.name}」に${formatNumber(downloadIds.length)}作品を入れました`,
        onClick: () => navigate(`/collections/${collection.id}`),
      });
      if (target === "__new__") navigate(`/collections/${collection.id}`);
    },
    onError: (error) =>
      notifications.show({ color: "red", title: "コレクションに入れられません", message: errorMessage(error) }),
  });

  const blocked = useMemo(() => {
    if (downloadIds.length === 0) return "作品が選ばれていません";
    if (target === "__new__" && !name.trim()) return "名前を入れてください";
    return null;
  }, [downloadIds.length, name, target]);

  return (
    <Modal opened={opened} onClose={onClose} title={`${formatNumber(downloadIds.length)}作品をコレクションにまとめる`} size="lg">
      <Stack>
        {!runtime && <Alert color="gray">プレビューではコレクションの変更は保存されません。</Alert>}
        <Radio.Group value={target} onChange={setTarget}>
          <Stack gap={8}>
            <UnstyledButton
              className="collection-pick"
              data-selected={target === "__new__" || undefined}
              onClick={() => setTarget("__new__")}
            >
              <Radio value="__new__" aria-label="新しいコレクションを作る" />
              <Icons.add size={IconSize.action} />
              <Text size="sm" fw={650}>新しいコレクションを作る</Text>
            </UnstyledButton>

            {collectionsQuery.isLoading ? (
              <Text size="sm" c="dimmed" p="sm">コレクションを読み込んでいます…</Text>
            ) : collections.length === 0 ? (
              <EmptyState
                icon={Icons.collection}
                title="まだコレクションはありません"
                description="上の「新しいコレクションを作る」から始められます。"
              />
            ) : (
              <ScrollArea.Autosize mah={320} type="auto">
                <Stack gap={6}>
                  {collections.map((collection) => (
                    <UnstyledButton
                      key={collection.id}
                      className="collection-pick"
                      data-selected={target === collection.id || undefined}
                      onClick={() => setTarget(collection.id)}
                    >
                      <Radio value={collection.id} aria-label={`${collection.name}に入れる`} />
                      <CollectionCover collection={collection} variant="card" className="collection-pick__cover" />
                      <span className="collection-pick__body">
                        <Text size="sm" fw={650} className="line-clamp-1">{collection.name}</Text>
                        <Text size="xs" c="dimmed">
                          {formatNumber(collection.memberCount)}作品 · {formatNumber(collection.totalTextLength)}字
                        </Text>
                      </span>
                    </UnstyledButton>
                  ))}
                </Stack>
              </ScrollArea.Autosize>
            )}
          </Stack>
        </Radio.Group>

        {target === "__new__" && (
          <Stack gap="xs">
            <TextInput
              label="名前"
              value={name}
              onChange={(event) => setName(event.currentTarget.value)}
              maxLength={200}
              required
              autoFocus
              placeholder="前後編、非公式の連載、テーマなど"
            />
            <Radio.Group value={kind} onChange={(value) => setKind(value as CollectionKind)} label="並び方">
              <Group gap="lg" mt={4}>
                <Radio value="ordered" label="順序付き" />
                <Radio value="unordered" label="順序なし" />
              </Group>
            </Radio.Group>
            <Text size="xs" c="dimmed">
              並びは投稿日順で入ります。あとから詳細画面で入れ替えられます。
            </Text>
          </Stack>
        )}

        <Group justify="flex-end">
          <Button variant="default" onClick={onClose}>キャンセル</Button>
          <Button
            disabled={Boolean(blocked) || !runtime}
            loading={mutation.isPending}
            onClick={() => mutation.mutate()}
          >
            {blocked ?? `${formatNumber(downloadIds.length)}作品を入れる`}
          </Button>
        </Group>
      </Stack>
    </Modal>
  );
}
