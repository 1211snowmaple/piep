import { useEffect, useState } from "react";
import { Button, Group, Modal, ScrollArea, Stack, Text, UnstyledButton } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { ActionRow } from "@/components/ActionRow";
import { CollectionCover } from "@/components/CollectionCover";
import { WorkCover } from "@/components/WorkCover";
import { errorMessage } from "@/lib/format";
import { Icons, IconSize } from "@/lib/icons";
import { openSingleDialog } from "@/services/dialogApi";
import type { CollectionCoverMode, WorkCollection, WorkCollectionInput } from "@/types/collections";

/** 作り方と、それが何をするか。順番は「自動で足りる」から「自分で決める」へ。 */
const MODES: { value: CollectionCoverMode; label: string; hint: string }[] = [
  { value: "mosaic", label: "モザイク", hint: "先頭の4作の表紙を並べます。中身が変わると表紙も変わります" },
  { value: "spine", label: "背表紙", hint: "表紙を少しずつ重ねて並べます。作品の数が厚みで分かります" },
  { value: "single", label: "1作を選ぶ", hint: "代表の1作の表紙を使います" },
  { value: "sigil", label: "紋", hint: "コレクションの名前から色と形を決めます。表紙が無くても使えます" },
  { value: "file", label: "画像を選ぶ", hint: "手元の画像ファイルを使います" },
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
  importImage,
  onSave,
}: {
  opened: boolean;
  onClose: () => void;
  collection: WorkCollection;
  busy: boolean;
  /**
   * 選んだ画像を棚へ取り込み、ファイルの識別子を返す（棚）。渡さなければ画像の場所を
   * そのまま保存する（旧棚）。取り込んでから見本に出すので、見本も保存後と同じ画像になる。
   */
  importImage?: (path: string) => Promise<string>;
  /** `coverWork` は「1作を選ぶ」で選んだ作品（取得元と作品の番号）。 */
  onSave: (input: WorkCollectionInput, coverWork: { source: string; sourceId: string } | null) => void;
}) {
  const [mode, setMode] = useState<CollectionCoverMode>("mosaic");
  // 表紙の作品は取得元と作品の番号で選ぶ（棚の作品は旧棚の番号を持たないことがある）。
  const [coverKey, setCoverKey] = useState<string | null>(null);
  const [imagePath, setImagePath] = useState<string | null>(null);
  const [importing, setImporting] = useState(false);

  useEffect(() => {
    if (!opened) return;
    setMode(collection.coverMode);
    const current = collection.members.find((member) => member.downloadId !== null && member.downloadId === collection.coverDownloadId);
    setCoverKey(current ? memberKey(current) : null);
    setImagePath(collection.coverImagePath);
  }, [collection.coverDownloadId, collection.coverImagePath, collection.coverMode, collection.members, opened]);

  const pickImage = async () => {
    try {
      const path = await openSingleDialog({
        title: "コレクションの表紙にする画像",
        filters: [{ name: "画像", extensions: ["png", "jpg", "jpeg", "webp"] }],
      });
      if (!path) return;
      if (importImage) {
        setImporting(true);
        setImagePath(await importImage(path));
      } else {
        setImagePath(path);
      }
      setMode("file");
    } catch (error) {
      notifications.show({ color: "red", title: "画像を選べません", message: errorMessage(error) });
    } finally {
      setImporting(false);
    }
  };

  // 選んだ設定でどう見えるかを、その場で出す。保存してから確かめさせない。
  const coverMember = collection.members.find((member) => memberKey(member) === coverKey) ?? null;
  const preview = { ...collection, coverMode: mode, coverImagePath: imagePath, coverPath: coverMember?.coverPath ?? null };
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
                      data-selected={coverKey === memberKey(member) || undefined}
                      aria-label={`${member.title}を表紙にする`}
                      onClick={() => setCoverKey(memberKey(member))}
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

        <ActionRow>
          <Button variant="default" onClick={onClose}>キャンセル</Button>
          <Button
            loading={busy || importing}
            disabled={(mode === "single" && !coverMember) || (mode === "file" && !imagePath)}
            onClick={() =>
              onSave({
                id: collection.id,
                name: collection.name,
                description: collection.description,
                collectionKind: collection.collectionKind,
                coverDownloadId: coverMember?.downloadId ?? null,
                coverMode: mode,
                coverImagePath: imagePath,
              }, coverMember ? { source: coverMember.source, sourceId: coverMember.sourceId } : null)
            }
          >
            この表紙にする
          </Button>
        </ActionRow>
      </Stack>
    </Modal>
  );
}

function memberKey(member: { source: string; sourceId: string }): string {
  return `${member.source}:${member.sourceId}`;
}
