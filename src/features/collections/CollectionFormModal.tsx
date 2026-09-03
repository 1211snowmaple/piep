import { useEffect, useState } from "react";
import { Button, Group, Modal, Radio, Stack, Textarea, TextInput } from "@mantine/core";
import type { CollectionKind, WorkCollectionInput } from "@/types/collections";

/**
 * 新しいコレクションを作る。
 *
 * 以前は編集も兼ねる作りだった（`collection` を渡すと題が変わり、値が入った
 * 状態で開く）。ところが編集の入口は「名前を付け直す」へ移っており、この枝へ
 * 渡す呼び出しはどこにも無くなっていた。**通らない道を残しておくと、次に
 * 読む人はそれが使われていると思って直しにいく。**
 *
 * 並び方はここで選ぶが、あとから詳細画面のメニューで変えられる。作るときに
 * 決めきらなくてよい。
 */
export function CollectionFormModal({ opened, onClose, onSave }: {
  opened: boolean;
  onClose: () => void;
  onSave: (input: WorkCollectionInput) => void;
}) {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [kind, setKind] = useState<CollectionKind>("ordered");
  useEffect(() => {
    if (!opened) return;
    setName("");
    setDescription("");
    setKind("ordered");
  }, [opened]);
  return (
    <Modal opened={opened} onClose={onClose} title="コレクションを作成" centered>
      <Stack>
        <TextInput label="名前" value={name} onChange={(event) => setName(event.currentTarget.value)} maxLength={200} required autoFocus />
        <Textarea label="説明" value={description} onChange={(event) => setDescription(event.currentTarget.value)} autosize minRows={3} maxRows={8} maxLength={10_000} />
        <Radio.Group label="並び方" description="あとから変えられます" value={kind} onChange={(value) => setKind(value as CollectionKind)}>
          <Stack gap="xs" mt="xs">
            <Radio value="ordered" label="順序付き — 前編・後編や連載、読む順がある作品" />
            <Radio value="unordered" label="順序なし — テーマや任意のまとまり" />
          </Stack>
        </Radio.Group>
        <Group justify="flex-end">
          <Button variant="default" onClick={onClose}>キャンセル</Button>
          <Button
            disabled={!name.trim()}
            onClick={() => onSave({ name: name.trim(), description: description.trim() || null, collectionKind: kind, coverDownloadId: null })}
          >
            保存
          </Button>
        </Group>
      </Stack>
    </Modal>
  );
}
