import { useEffect, useState } from "react";
import { Alert, Badge, Button, Group, Loader, Modal, Stack, Text, Textarea, TextInput, UnstyledButton } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { useMutation, useQuery } from "@tanstack/react-query";
import { errorMessage } from "@/lib/format";
import { Icons, IconSize } from "@/lib/icons";
import { proposeCollectionNames } from "@/services/collectionApi";
import { isTauriRuntime } from "@/services/dbApi";
import { nameCollectionWithModel } from "@/services/assistApi";
import { useAssist } from "@/features/assist/useAssist";
import type { CollectionNameCandidate, WorkCollection, WorkCollectionInput } from "@/types/collections";

const SOURCE_LABEL: Record<string, string> = {
  manual: "いまの名前",
  title: "題名の共通部分",
  series: "公式シリーズ",
  tags: "共有タグ",
  author: "作者",
  llm: "モデルの案",
};

/**
 * すでにあるコレクションの名前と説明を付け直す。
 *
 * 束は作ったあとで中身が変わる。作品を足せば共有タグが変わり、外せば題名の
 * 共通部分も変わる。**作ったときの名前に縛られる理由は無い。**
 *
 * 「自動ひな型から作成」という説明が付いた古い束や、検索用の正規化キーが
 * そのまま名前になってしまった束を、ここで直せる。
 */
export function CollectionRenameModal({
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
  const runtime = isTauriRuntime();
  const [name, setName] = useState(collection.name);
  const [description, setDescription] = useState(collection.description ?? "");
  const [source, setSource] = useState(collection.nameSource);
  const [modelOption, setModelOption] = useState<CollectionNameCandidate | null>(null);
  const [modelSubtitle, setModelSubtitle] = useState<string | null>(null);

  const proposals = useQuery({
    queryKey: ["collection-name-proposals", collection.id],
    queryFn: () => (runtime ? proposeCollectionNames(collection.id) : Promise.resolve([])),
    enabled: opened,
  });
  const { engine } = useAssist();

  useEffect(() => {
    if (!opened) return;
    setName(collection.name);
    setDescription(collection.description ?? "");
    setSource(collection.nameSource);
    setModelOption(null);
    setModelSubtitle(null);
  }, [collection.description, collection.name, collection.nameSource, opened]);

  const askModel = useMutation({
    mutationFn: () => {
      if (!engine) return Promise.reject(new Error("モデルの手伝いが設定されていません"));
      return nameCollectionWithModel(collection.id, engine);
    },
    onSuccess: (named) => {
      setModelOption({ source: "llm", label: SOURCE_LABEL.llm, name: named.name });
      setName(named.name);
      setSource("llm");
      // 書いてある説明を黙って消さない。ただし黙って捨てもしない —
      // 「名前と説明を考えてもらう」と書いておいて、既定のひな型文が入っている
      // だけで説明が何も変わらないのは、考えていないのと見分けがつかない。
      setModelSubtitle(named.subtitle || null);
      if (named.subtitle && !description.trim()) setDescription(named.subtitle);
    },
    onError: (error) =>
      notifications.show({ color: "red", title: "モデルが名前を返しません", message: errorMessage(error) }),
  });

  const options = [...(proposals.data ?? []), ...(modelOption ? [modelOption] : [])];

  return (
    <Modal opened={opened} onClose={onClose} title="名前と説明を付け直す" size="lg">
      <Stack>
        <TextInput
          label="名前"
          value={name}
          onChange={(event) => { setName(event.currentTarget.value); setSource("manual"); }}
          maxLength={200}
          required
          autoFocus
        />
        <Textarea
          label="説明"
          description="空のままでもかまいません。何がまとまっているかを一行で。"
          value={description}
          onChange={(event) => setDescription(event.currentTarget.value)}
          autosize
          minRows={2}
          maxRows={6}
          maxLength={10_000}
        />
        {modelSubtitle && modelSubtitle !== description && (
          <Group gap="xs" wrap="nowrap" align="flex-start" mt={-8}>
            <Text size="xs" c="dimmed" style={{ flex: 1, minWidth: 0 }}>
              モデルの説明の案：{modelSubtitle}
            </Text>
            <Button size="compact-xs" variant="light" onClick={() => setDescription(modelSubtitle)}>
              これにする
            </Button>
          </Group>
        )}

        <Stack gap={6}>
          <Group gap="xs" justify="space-between">
            <Text size="sm" fw={650}>名前の案</Text>
            {proposals.isFetching && <Loader size="xs" />}
          </Group>
          {options.length === 0 && !proposals.isFetching ? (
            <Text size="xs" c="dimmed">
              作品が入っていないので、題名やタグから案を作れません。
            </Text>
          ) : (
            options.map((option) => (
              <UnstyledButton
                key={`${option.source}-${option.name}`}
                className="collection-pick"
                data-selected={name === option.name || undefined}
                onClick={() => { setName(option.name); setSource(option.source); }}
              >
                <span className="collection-pick__body">
                  <Text size="sm" fw={650} className="line-clamp-2">{option.name}</Text>
                  <Text size="xs" c="dimmed">{option.label || SOURCE_LABEL[option.source] || option.source}</Text>
                </span>
                {name === option.name && <Icons.confirm size={IconSize.menu} />}
              </UnstyledButton>
            ))
          )}
        </Stack>

        {engine ? (
          <Button
            variant="light"
            color="grape"
            leftSection={<Icons.optimize size={IconSize.action} />}
            loading={askModel.isPending}
            disabled={busy || collection.availableCount === 0}
            onClick={() => askModel.mutate()}
          >
            モデルに名前と説明を考えてもらう
          </Button>
        ) : (
          <Alert color="gray" icon={<Icons.info size={IconSize.action} />}>
            <Text size="sm">
              手元の言語モデルにも案を出させたい場合は、設定の「AIの手伝い」で
              つなぎ先を選んでください。切ったままでも上の案から選べます。
            </Text>
          </Alert>
        )}

        <Group justify="space-between">
          <Badge variant="light" color="gray">{SOURCE_LABEL[source] ?? source}</Badge>
          <Group gap="xs">
            <Button variant="default" onClick={onClose}>キャンセル</Button>
            <Button
              loading={busy}
              disabled={!name.trim()}
              onClick={() =>
                onSave({
                  id: collection.id,
                  name: name.trim(),
                  description: description.trim() || null,
                  collectionKind: collection.collectionKind,
                  coverDownloadId: collection.coverDownloadId,
                  // どこから来た名前かを覚える。自動命名が後から上書きしない。
                  nameSource: source,
                })
              }
            >
              保存
            </Button>
          </Group>
        </Group>
      </Stack>
    </Modal>
  );
}
