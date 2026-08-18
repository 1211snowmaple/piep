import { useEffect, useState } from "react";
import { Button, Group, Modal, Radio, Stack, Textarea, TextInput } from "@mantine/core";
import type { CollectionKind, WorkCollectionSummary, WorkCollectionInput } from "@/types/collections";

/** Shared by the library tab, which creates collections, and the detail screen,
 *  which renames them. Both need the same name / description / order choice. */
export function CollectionFormModal({ opened, collection, onClose, onSave }: {
  opened: boolean;
  collection?: WorkCollectionSummary | null;
  onClose: () => void;
  onSave: (input: WorkCollectionInput) => void;
}) {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [kind, setKind] = useState<CollectionKind>("ordered");
  useEffect(() => {
    if (!opened) return;
    setName(collection?.name ?? "");
    setDescription(collection?.description ?? "");
    setKind(collection?.collectionKind ?? "ordered");
  }, [collection, opened]);
  return (
    <Modal opened={opened} onClose={onClose} title={collection ? "コレクションを編集" : "コレクションを作成"} centered>
      <Stack>
        <TextInput label="名前" value={name} onChange={(event) => setName(event.currentTarget.value)} maxLength={200} required autoFocus />
        <Textarea label="説明" value={description} onChange={(event) => setDescription(event.currentTarget.value)} autosize minRows={3} maxRows={8} maxLength={10_000} />
        <Radio.Group label="並び方" value={kind} onChange={(value) => setKind(value as CollectionKind)}>
          <Stack gap="xs" mt="xs">
            <Radio value="ordered" label="順序付き — 前編・後編や連載、読む順がある作品" />
            <Radio value="unordered" label="順序なし — テーマや任意のまとまり" />
          </Stack>
        </Radio.Group>
        <Group justify="flex-end"><Button variant="default" onClick={onClose}>キャンセル</Button><Button disabled={!name.trim()} onClick={() => onSave({ id: collection?.id, name: name.trim(), description: description.trim() || null, collectionKind: kind, coverDownloadId: collection?.coverDownloadId ?? null })}>保存</Button></Group>
      </Stack>
    </Modal>
  );
}
