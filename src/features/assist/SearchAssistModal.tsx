import { useEffect, useRef, useState } from "react";
import { Badge, Button, Group, Modal, Stack, Text, Textarea } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { useMutation } from "@tanstack/react-query";
import { errorMessage } from "@/lib/format";
import { Icons, IconSize } from "@/lib/icons";
import { useAssist } from "@/features/assist/useAssist";
import { interpretSearch, type SearchIntent } from "@/services/assistApi";

/**
 * 「こういうのが読みたい」を意味検索に渡す。モデルが使える場合は
 * タグと検索文への言い換えも選べる。
 *
 * 意味索引は全作ぶん出来ているのに、入口が語句検索しかなかった。曖昧な記憶
 * から棚を引く入口で、**検索そのものは piep がやる**。
 *
 * 翻訳した結果はそのまま絞り込みになるので、外れていれば見て分かる。
 */
export function SearchAssistModal({
  opened,
  onClose,
  onApply,
}: {
  opened: boolean;
  onClose: () => void;
  onApply: (intent: SearchIntent) => boolean | void;
}) {
  const { engine } = useAssist("search_interpretation");
  const [phrase, setPhrase] = useState("");
  const [intent, setIntent] = useState<SearchIntent | null>(null);
  const revision = useRef(0);

  useEffect(() => {
    if (!opened) {
      revision.current += 1;
      setIntent(null);
    }
  }, [opened]);

  const close = () => {
    revision.current += 1;
    setIntent(null);
    onClose();
  };

  const run = useMutation({
    mutationFn: (request: { phrase: string; revision: number }) => {
      if (!engine) return Promise.reject(new Error("モデルの手伝いが設定されていません"));
      return interpretSearch(engine, request.phrase);
    },
    onSuccess: (result, request) => {
      if (request.revision === revision.current) setIntent(result);
    },
    onError: (error, request) => {
      if (request.revision !== revision.current) return;
      setIntent(null);
      notifications.show({ color: "red", title: "言い換えられません", message: errorMessage(error) });
    },
  });

  const empty = intent && intent.includeTags.length === 0 && intent.excludeTags.length === 0 && !intent.query;

  const applyDirect = () => {
    const query = phrase.trim();
    if (!query) return;
    if (onApply({ includeTags: [], excludeTags: [], query, reading: "入力した文をそのまま探します" }) !== false) close();
  };

  return (
    <Modal opened={opened} onClose={close} title="言葉で探す" size="lg">
      <Stack>
        <Textarea
          label="どんなものが読みたいか"
          description="覚えている範囲で書いてください。そのまま探すか、AIで条件に言い換えられます。"
          placeholder="旅先で出会った二人が、少しずつ仲良くなる話"
          value={phrase}
          onChange={(event) => { revision.current += 1; setPhrase(event.currentTarget.value); setIntent(null); }}
          autosize
          minRows={2}
          maxRows={5}
          maxLength={500}
          autoFocus
        />
        <Text size="xs" c="dimmed">入力した文をそのまま意味検索に渡せます。題名や本文の言葉を覚えている場合は、ライブラリの通常検索が向いています。</Text>
        <Group justify="space-between">
          <Button disabled={!phrase.trim()} onClick={applyDirect}>この文で意味検索</Button>
          <Button
            variant="light"
            color="grape"
            leftSection={<Icons.optimize size={IconSize.action} />}
            loading={run.isPending}
            disabled={!engine || !phrase.trim()}
            onClick={() => run.mutate({ phrase: phrase.trim(), revision: revision.current })}
          >
            AIで条件に言い換える
          </Button>
        </Group>
        {!engine && <Text size="xs" c="dimmed">AIによる言い換えは設定済みの言語モデルがある場合に使えます。</Text>}

        {intent && (
          <Stack gap={8}>
            <Text size="sm" c="dimmed">{intent.reading}</Text>
            {intent.includeTags.length > 0 && (
              <Group gap={6} wrap="wrap">
                <Text size="xs" c="dimmed">含める（{intent.includeTagMode === "or" ? "いずれか" : "すべて"}）</Text>
                {intent.includeTags.map((tag) => <Badge key={tag} variant="light">{tag}</Badge>)}
              </Group>
            )}
            {intent.excludeTags.length > 0 && (
              <Group gap={6} wrap="wrap">
                <Text size="xs" c="dimmed">除く</Text>
                {intent.excludeTags.map((tag) => <Badge key={tag} variant="light" color="red">{tag}</Badge>)}
              </Group>
            )}
            {intent.query && (
              <Group gap={6}>
                <Text size="xs" c="dimmed">意味検索に使う文</Text>
                <Badge variant="outline">{intent.query}</Badge>
              </Group>
            )}
            {empty && (
              <Text size="sm" c="dimmed">棚の言葉には言い換えられませんでした。別の言い方で試してみてください。</Text>
            )}
          </Stack>
        )}

        <Group justify="flex-end">
          <Button variant="default" onClick={close}>キャンセル</Button>
          <Button
            disabled={!intent || Boolean(empty)}
            onClick={() => { if (intent && onApply(intent) !== false) close(); }}
          >
            この条件で棚を絞る
          </Button>
        </Group>
      </Stack>
    </Modal>
  );
}
