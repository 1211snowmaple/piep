import { useState } from "react";
import { Badge, Checkbox, Group, Stack, Text, Tooltip } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { errorMessage } from "@/lib/format";
import { AssistNoteBody, AssistPanel } from "@/features/assist/AssistPanel";
import { useAssist } from "@/features/assist/useAssist";
import {
  acceptTags,
  deleteNote,
  loadNote,
  removeAssistedTag,
  suggestTags,
  summarizeWork,
  workTags,
  type TagProposal,
} from "@/services/assistApi";
import { isTauriRuntime } from "@/services/dbApi";

/**
 * 作品ひとつに対する手伝い。あらすじと、タグの補完。
 *
 * どちらも押したときだけ動く。設定していなければ、この節そのものが出ない。
 */
export function WorkAssist({ downloadId }: { downloadId: number }) {
  const { body } = useAssist();
  if (!isTauriRuntime()) return null;
  return (
    <Stack gap="sm">
      <SynopsisPanel downloadId={downloadId} bodyAllowed={body} />
      <TagPanel downloadId={downloadId} />
    </Stack>
  );
}

/** 本文から作ったあらすじ。作れば残り、次からは呼ばずに出る。 */
function SynopsisPanel({ downloadId, bodyAllowed }: { downloadId: number; bodyAllowed: boolean }) {
  const { engine } = useAssist();
  const queryClient = useQueryClient();
  const key = ["assist-note", "work", String(downloadId), "synopsis"] as const;

  // 保存してあるものを読むだけ。**ここではモデルを呼ばない。**
  const note = useQuery({
    queryKey: key,
    queryFn: () => loadNote("work", String(downloadId), "synopsis"),
  });

  const run = useMutation({
    mutationFn: () => {
      if (!engine) return Promise.reject(new Error("モデルの手伝いが設定されていません"));
      return summarizeWork(engine, downloadId);
    },
    onSuccess: () => queryClient.invalidateQueries({ queryKey: key }),
    onError: (error) => notifications.show({ color: "red", title: "あらすじを作れません", message: errorMessage(error) }),
  });
  const discard = useMutation({
    mutationFn: () => deleteNote("work", String(downloadId), "synopsis"),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: key }),
  });

  return (
    <AssistPanel
      title="あらすじ"
      hint="本文から、あとで思い出すための3文を作ります。取得元の紹介文は宣伝であることが多いので、その代わりに。"
      engineReady={Boolean(engine)}
      needsBody
      bodyAllowed={bodyAllowed}
      busy={run.isPending}
      actionLabel={note.data ? "作り直す" : "あらすじを作る"}
      onRun={() => run.mutate()}
    >
      {note.data && (
        <AssistNoteBody
          text={note.data.text}
          modelId={note.data.modelId}
          createdAt={note.data.createdAt}
          onDiscard={() => discard.mutate()}
        />
      )}
    </AssistPanel>
  );
}

/**
 * タグの補完。
 *
 * 棚にある語からしか選ばせないので、付いたタグはそのまま検索にも束ねにも効く。
 * **採ったタグは取得元のものと区別して保存**され、あとから外せる。
 */
function TagPanel({ downloadId }: { downloadId: number }) {
  const { engine } = useAssist();
  const queryClient = useQueryClient();
  const [proposals, setProposals] = useState<TagProposal[] | null>(null);
  const [picked, setPicked] = useState<ReadonlySet<string>>(new Set());
  const tagsKey = ["assist-tags", downloadId] as const;

  const tags = useQuery({ queryKey: tagsKey, queryFn: () => workTags(downloadId) });
  const assisted = (tags.data ?? []).filter((tag) => tag.source === "llm");

  const run = useMutation({
    mutationFn: () => {
      if (!engine) return Promise.reject(new Error("モデルの手伝いが設定されていません"));
      return suggestTags(engine, downloadId);
    },
    onSuccess: (found) => {
      setProposals(found);
      // 既定では何も選ばない。押さなければ何も変わらない。
      setPicked(new Set());
      if (found.length === 0) {
        notifications.show({ message: "足せそうなタグは見つかりませんでした" });
      }
    },
    onError: (error) => notifications.show({ color: "red", title: "タグを提案できません", message: errorMessage(error) }),
  });

  const accept = useMutation({
    mutationFn: () => acceptTags(downloadId, [...picked]),
    onSuccess: (next) => {
      setProposals(null);
      setPicked(new Set());
      queryClient.setQueryData(tagsKey, next);
      queryClient.invalidateQueries({ queryKey: ["library"] });
      // The tag row in the work hero directly above this panel is served by
      // ["reader-metadata"], not by tagsKey, so it keeps the pre-accept tags
      // until the page is reopened unless it is invalidated here too.
      queryClient.invalidateQueries({ queryKey: ["reader-metadata", downloadId] });
      notifications.show({ color: "green", message: "タグを付けました" });
    },
    onError: (error) => notifications.show({ color: "red", title: "タグを付けられません", message: errorMessage(error) }),
  });

  const remove = useMutation({
    mutationFn: (tag: string) => removeAssistedTag(downloadId, tag),
    onSuccess: (next) => {
      queryClient.setQueryData(tagsKey, next);
      queryClient.invalidateQueries({ queryKey: ["library"] });
      queryClient.invalidateQueries({ queryKey: ["reader-metadata", downloadId] });
    },
  });

  return (
    <AssistPanel
      title="タグの補完"
      hint="棚にある語の中から、この作品に足りていないものを挙げてもらいます。新しい語は作りません。"
      engineReady={Boolean(engine)}
      busy={run.isPending}
      actionLabel="タグを提案してもらう"
      onRun={() => run.mutate()}
    >
      {assisted.length > 0 && (
        <Group gap={6} wrap="wrap">
          <Text size="xs" c="dimmed">モデルの案から採ったタグ</Text>
          {assisted.map((tag) => (
            <Tooltip key={tag.name} label="外す">
              <Badge
                variant="light"
                color="grape"
                size="sm"
                style={{ cursor: "pointer" }}
                onClick={() => remove.mutate(tag.name)}
              >
                {tag.name}
              </Badge>
            </Tooltip>
          ))}
        </Group>
      )}

      {proposals && proposals.length > 0 && (
        <Stack gap={6}>
          {proposals.map((proposal) => (
            <Checkbox
              key={proposal.tag}
              checked={picked.has(proposal.tag)}
              onChange={(event) => {
                const on = event.currentTarget.checked;
                setPicked((current) => {
                  const next = new Set(current);
                  if (on) next.add(proposal.tag); else next.delete(proposal.tag);
                  return next;
                });
              }}
              label={
                <span>
                  <Text component="span" size="sm" fw={650}>{proposal.tag}</Text>
                  <Text size="xs" c="dimmed">{proposal.reason}</Text>
                </span>
              }
            />
          ))}
          <Group justify="flex-end" gap="xs">
            <Text size="xs" c="dimmed" style={{ marginRight: "auto" }}>
              選んだものだけが付きます。取得元のタグとは区別して保存されます。
            </Text>
            <Group gap={6}>
              <Badge variant="light" color="gray" size="sm">{picked.size}件</Badge>
              <Tooltip label={picked.size === 0 ? "タグを選んでください" : "選んだタグを付ける"}>
                <Badge
                  component="button"
                  variant="filled"
                  color="grape"
                  size="lg"
                  style={{ cursor: picked.size === 0 ? "default" : "pointer", border: 0 }}
                  onClick={() => picked.size > 0 && accept.mutate()}
                >
                  付ける
                </Badge>
              </Tooltip>
            </Group>
          </Group>
        </Stack>
      )}
    </AssistPanel>
  );
}
