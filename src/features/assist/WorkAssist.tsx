import { useState } from "react";
import { Badge, Button, Checkbox, Divider, Group, Modal, Stack, Text, Tooltip } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { AssistLauncher } from "@/features/assist/AssistLauncher";
import { AssistNoteBody } from "@/features/assist/AssistPanel";
import { useAssist } from "@/features/assist/useAssist";
import { errorMessage } from "@/lib/format";
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
import { usePageAssist } from "@/app/PageAssistContext";

type WorkAssistAction = "synopsis" | "tags";

/** One contextual launcher owns every optional model action on a work page. */
export function WorkAssist({ downloadId }: { downloadId: number }) {
  const runtime = isTauriRuntime();
  const synopsis = useAssist("work_synopsis");
  const tagging = useAssist("work_tagging");
  const queryClient = useQueryClient();
  const [active, setActive] = useState<WorkAssistAction | null>(null);
  const [proposals, setProposals] = useState<TagProposal[] | null>(null);
  const [picked, setPicked] = useState<ReadonlySet<string>>(new Set());
  const noteKey = ["assist-note", "work", String(downloadId), "synopsis"] as const;
  const tagsKey = ["assist-tags", downloadId] as const;

  const note = useQuery({ queryKey: noteKey, queryFn: () => loadNote("work", String(downloadId), "synopsis") });
  const tags = useQuery({ queryKey: tagsKey, queryFn: () => workTags(downloadId) });
  const assisted = (tags.data ?? []).filter((tag) => tag.source === "llm");

  const summarize = useMutation({
    mutationFn: () => synopsis.engine
      ? summarizeWork(synopsis.engine, downloadId)
      : Promise.reject(new Error("あらすじ機能が設定されていません")),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: noteKey }),
    onError: (error) => notifications.show({ color: "red", title: "あらすじを作れません", message: errorMessage(error) }),
  });
  const discard = useMutation({
    mutationFn: () => deleteNote("work", String(downloadId), "synopsis"),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: noteKey }),
  });
  const propose = useMutation({
    mutationFn: () => tagging.engine
      ? suggestTags(tagging.engine, downloadId)
      : Promise.reject(new Error("タグ補完が設定されていません")),
    onSuccess: (found) => {
      setProposals(found);
      setPicked(new Set());
      if (found.length === 0) notifications.show({ message: "足せそうなタグは見つかりませんでした" });
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

  const synopsisReady = Boolean(synopsis.engine && synopsis.body);
  const launcherItems = [
    {
      id: "work_synopsis",
      label: note.data ? "あらすじを確認・作り直す" : "あらすじを作る",
      description: "本文から、再読時に思い出すための3文を作ります",
      enabled: synopsisReady,
      unavailableReason: synopsis.engine ? "設定で本文の送信を許可してください" : "設定で「あらすじ」を有効にしてください",
      onSelect: () => setActive("synopsis" as const),
      badge: note.data ? "保存済み" : undefined,
    },
    {
      id: "work_tagging",
      label: "タグを補完する",
      description: "棚にあるタグだけから不足候補を挙げます",
      enabled: Boolean(tagging.engine),
      unavailableReason: "設定で「タグの補完」を有効にしてください",
      onSelect: () => setActive("tags" as const),
      badge: assisted.length ? `${assisted.length}件採用済み` : undefined,
    },
  ];
  const headerHosted = usePageAssist(
    `work-assist-${downloadId}`,
    "この作品で使えるAIの手伝い",
    launcherItems,
    runtime,
  );

  if (!runtime) return null;

  return (
    <Stack gap="sm">
      {!headerHosted && <Group justify="flex-end"><AssistLauncher label="この作品で使える手伝い" items={launcherItems} /></Group>}

      {note.data && (
        <AssistNoteBody
          text={note.data.text}
          modelId={note.data.modelId}
          createdAt={note.data.createdAt}
          promptVersion={note.data.promptVersion}
          promptStale={note.data.promptStale}
          inputStale={note.data.inputStale}
          onDiscard={() => discard.mutate()}
        />
      )}
      {assisted.length > 0 && (
        <Group gap={6} wrap="wrap">
          <Text size="xs" c="dimmed">モデルの案から採ったタグ</Text>
          {assisted.map((tag) => (
            <Tooltip key={tag.name} label="外す">
              <Badge variant="light" color="grape" style={{ cursor: "pointer" }} onClick={() => remove.mutate(tag.name)}>{tag.name}</Badge>
            </Tooltip>
          ))}
        </Group>
      )}

      <Modal opened={active !== null} onClose={() => setActive(null)} title={active === "tags" ? "タグの補完" : "あらすじ"} size="lg">
        {active === "synopsis" ? (
          <Stack>
            <Text size="sm" c="dimmed">本文から、あとで思い出すための3文を作ります。結果にはモデル・指示版・入力の鮮度を記録します。</Text>
            {note.data && <AssistNoteBody text={note.data.text} modelId={note.data.modelId} createdAt={note.data.createdAt} promptVersion={note.data.promptVersion} promptStale={note.data.promptStale} inputStale={note.data.inputStale} />}
            <Group justify="flex-end"><Button loading={summarize.isPending} disabled={!synopsisReady} onClick={() => summarize.mutate()}>{note.data ? "作り直す" : "あらすじを作る"}</Button></Group>
          </Stack>
        ) : active === "tags" ? (
          <Stack>
            <Text size="sm" c="dimmed">新しい語は作らず、棚にすでにあるタグだけを候補にします。採用するまでは作品を変更しません。</Text>
            <Group justify="flex-end"><Button loading={propose.isPending} onClick={() => propose.mutate()}>タグを提案してもらう</Button></Group>
            {proposals && proposals.length > 0 && <Divider />}
            {proposals?.map((proposal) => (
              <Checkbox
                key={proposal.tag}
                checked={picked.has(proposal.tag)}
                onChange={(event) => setPicked((current) => {
                  const next = new Set(current);
                  if (event.currentTarget.checked) next.add(proposal.tag); else next.delete(proposal.tag);
                  return next;
                })}
                label={<span><Text component="span" size="sm" fw={650}>{proposal.tag}</Text><Text size="xs" c="dimmed">{proposal.reason}</Text></span>}
              />
            ))}
            {proposals && proposals.length > 0 && <Group justify="flex-end"><Button disabled={!picked.size} loading={accept.isPending} onClick={() => accept.mutate()}>選んだ{picked.size}件を付ける</Button></Group>}
          </Stack>
        ) : null}
      </Modal>
    </Stack>
  );
}
