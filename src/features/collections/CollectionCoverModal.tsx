import { useEffect, useState } from "react";
import { Button, Group, Modal, ScrollArea, Stack, Text, UnstyledButton } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { CollectionCover } from "@/components/CollectionCover";
import { WorkCover } from "@/components/WorkCover";
import { errorMessage } from "@/lib/format";
import { Icons, IconSize } from "@/lib/icons";
import { openSingleDialog } from "@/services/dialogApi";
import type { CollectionCoverMode, WorkCollection, WorkCollectionInput } from "@/types/collections";

/** 作り方と、それが何をするか。順番は「自動で足りる」から「自分で決める」へ。 */
const MODES: { value: CollectionCoverMode; label: string; hint: string }[] = [
  { value: "mosaic", label: "モザイク", hint: "先頭4作の表紙を並べる。メンバーが変われば追従する" },
  { value: "spine", label: "背表紙", hint: "少しずつ重ねて並べる。冊数が形として見える" },
  { value: "single", label: "1作を選ぶ", hint: "看板になる1作の表紙を使う" },
  { value: "sigil", label: "紋", hint: "束の名前から色と形を決める。表紙が無くても成り立つ" },
  { value: "file", label: "画像を選ぶ", hint: "手元の画像ファイルを使う" },
];

/**
 * 表紙の作り方を選ぶ。
 *
 * 一つに決め打ちしないのは、束の性格が一定でないからである。前後編の2作なら
 * 看板は1枚で足りるが、作者をまたぐ20作のテーマ束に代表作は無い。既定は自動で、
 * 気に入らなければ差し替えられる、という順番にする。
 */
export function CollectionCoverModal({
  opened,
  onClose,
  collection,
  busy,
  onSave,
}: {
  opened: boolean;
  onClose: () => void;
  collection: WorkCollection;
  busy: boolean;
  onSave: (input: WorkCollectionInput) => void;
}) {
  const [mode, setMode] = useState<CollectionCoverMode>("mosaic");
  const [coverDownloadId, setCoverDownloadId] = useState<number | null>(null);
  const [imagePath, setImagePath] = useState<string | null>(null);

  useEffect(() => {
    if (!opened) return;
    setMode(collection.coverMode);
    setCoverDownloadId(collection.coverDownloadId);
    setImagePath(collection.coverImagePath);
  }, [collection.coverDownloadId, collection.coverImagePath, collection.coverMode, opened]);

  const pickImage = async () => {
    try {
      const path = await openSingleDialog({
        title: "コレクションの表紙にする画像",
        filters: [{ name: "画像", extensions: ["png", "jpg", "jpeg", "webp"] }],
      });
      if (path) { setImagePath(path); setMode("file"); }
    } catch (error) {
      notifications.show({ color: "red", title: "画像を選べません", message: errorMessage(error) });
    }
  };

  // 選んだ設定でどう見えるかを、その場で出す。保存してから確かめさせない。
  const preview = { ...collection, coverMode: mode, coverImagePath: imagePath, coverPath: coverPathFor(collection, coverDownloadId) };
  const members = collection.members.filter((member) => member.work);

  return (
    <Modal opened={opened} onClose={onClose} title="コレクションの表紙" size="lg">
      <Stack>
        <Group align="flex-start" gap="lg" wrap="nowrap">
          <CollectionCover collection={preview} variant="detail" />
          <Stack gap={6} flex={1} miw={0}>
            {MODES.map((option) => (
              <UnstyledButton
                key={option.value}
                className="collection-pick"
                data-selected={mode === option.value || undefined}
                onClick={() => (option.value === "file" ? pickImage() : setMode(option.value))}
              >
                <span className="collection-pick__body">
                  <Text size="sm" fw={650}>{option.label}</Text>
                  <Text size="xs" c="dimmed">{option.hint}</Text>
                </span>
                {mode === option.value && <Icons.confirm size={IconSize.menu} />}
              </UnstyledButton>
            ))}
          </Stack>
        </Group>

        {mode === "single" && (
          <Stack gap={6}>
            <Text size="sm" fw={650}>表紙にする作品</Text>
            {members.length === 0 ? (
              <Text size="xs" c="dimmed">保存済みの作品がまだありません。</Text>
            ) : (
              <ScrollArea.Autosize mah={200} type="auto">
                <div className="collection-cover-pick">
                  {members.map((member) => (
                    <UnstyledButton
                      key={`${member.source}:${member.sourceId}`}
                      className="collection-cover-pick__item"
                      data-selected={coverDownloadId === member.downloadId || undefined}
                      aria-label={`${member.title}を表紙にする`}
                      onClick={() => setCoverDownloadId(member.downloadId)}
                    >
                      <WorkCover work={member.work!} variant="compact" />
                    </UnstyledButton>
                  ))}
                </div>
              </ScrollArea.Autosize>
            )}
          </Stack>
        )}

        {mode === "file" && (
          <Group gap="xs">
            <Button size="compact-sm" variant="default" onClick={pickImage}>画像を選び直す</Button>
            <Text size="xs" c="dimmed" className="line-clamp-1">{imagePath ?? "まだ選ばれていません"}</Text>
          </Group>
        )}

        <Group justify="flex-end">
          <Button variant="default" onClick={onClose}>キャンセル</Button>
          <Button
            loading={busy}
            disabled={(mode === "single" && !coverDownloadId) || (mode === "file" && !imagePath)}
            onClick={() =>
              onSave({
                id: collection.id,
                name: collection.name,
                description: collection.description,
                collectionKind: collection.collectionKind,
                coverDownloadId,
                coverMode: mode,
                coverImagePath: imagePath,
              })
            }
          >
            この表紙にする
          </Button>
        </Group>
      </Stack>
    </Modal>
  );
}

/** 選び直した作品の表紙を、保存前の見本に反映する。 */
function coverPathFor(collection: WorkCollection, downloadId: number | null): string | null {
  if (downloadId === null) return null;
  const member = collection.members.find((value) => value.downloadId === downloadId);
  return member?.work?.coverPath ?? member?.coverPath ?? null;
}
