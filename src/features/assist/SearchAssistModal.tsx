import { useState } from "react";
import { Badge, Button, Group, Modal, Stack, Text, Textarea } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { useMutation } from "@tanstack/react-query";
import { errorMessage } from "@/lib/format";
import { Icons, IconSize } from "@/lib/icons";
import { useAssist } from "@/features/assist/useAssist";
import { interpretSearch, type SearchIntent } from "@/services/assistApi";

/**
 * 「こういうのが読みたい」を、棚の言葉に翻訳する。
 *
 * 意味索引は全作ぶん出来ているのに、入口が語句検索しかなかった。曖昧な記憶
 * から棚を引くための翻訳で、**検索そのものは piep がやる** — モデルは
 * タグと検索語を選ぶだけで、結果を作らない。
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
  onApply: (intent: SearchIntent) => void;
}) {
  const { engine } = useAssist();
  const [phrase, setPhrase] = useState("");
  const [intent, setIntent] = useState<SearchIntent | null>(null);

  const run = useMutation({
    mutationFn: () => {
      if (!engine) return Promise.reject(new Error("モデルの手伝いが設定されていません"));
      return interpretSearch(engine, phrase);
    },
    onSuccess: setIntent,
    onError: (error) => {
      setIntent(null);
      notifications.show({ color: "red", title: "言い換えられません", message: errorMessage(error) });
    },
  });

  const empty = intent && intent.includeTags.length === 0 && intent.excludeTags.length === 0 && !intent.query;

  return (
    <Modal opened={opened} onClose={onClose} title="言葉で探す" size="lg">
      <Stack>
        <Textarea
          label="どんなものが読みたいか"
          description="覚えている範囲で構いません。棚のタグと検索語に言い換えます。"
          placeholder="旅先で出会った二人が、少しずつ仲良くなる話"
          value={phrase}
          onChange={(event) => { setPhrase(event.currentTarget.value); setIntent(null); }}
          autosize
          minRows={2}
          maxRows={5}
          maxLength={500}
          autoFocus
        />
        <Group justify="flex-end">
          <Button
            variant="light"
            color="grape"
            leftSection={<Icons.optimize size={IconSize.action} />}
            loading={run.isPending}
            disabled={!phrase.trim()}
            onClick={() => run.mutate()}
          >
            言い換えてもらう
          </Button>
        </Group>

        {intent && (
          <Stack gap={8}>
            <Text size="sm" c="dimmed">{intent.reading}</Text>
            {intent.includeTags.length > 0 && (
              <Group gap={6} wrap="wrap">
                <Text size="xs" c="dimmed">含める</Text>
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
                <Text size="xs" c="dimmed">本文から探す語</Text>
                <Badge variant="outline">{intent.query}</Badge>
              </Group>
            )}
            {empty && (
              <Text size="sm" c="dimmed">棚の言葉には言い換えられませんでした。別の言い方で試してみてください。</Text>
            )}
          </Stack>
        )}

        <Group justify="flex-end">
          <Button variant="default" onClick={onClose}>キャンセル</Button>
          <Button
            disabled={!intent || Boolean(empty)}
            onClick={() => { if (intent) { onApply(intent); onClose(); } }}
          >
            この条件で棚を絞る
          </Button>
        </Group>
      </Stack>
    </Modal>
  );
}
