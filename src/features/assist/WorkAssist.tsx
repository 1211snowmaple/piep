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
  acceptRelatedTags,
  deleteNote,
  loadNote,
  relatedTagCandidates,
  removeManualTag,
  removeAssistedTag,
  suggestTags,
  summarizeWork,
  toEngine,
  workTags,
  type TagProposal,
  type RelatedTagCandidate,
} from "@/services/assistApi";
import { isTauriRuntime } from "@/services/dbApi";
import { usePageAssist } from "@/app/PageAssistContext";

type WorkAssistAction = "synopsis" | "tags" | "related-tags";

/** One contextual launcher owns every optional model action on a work page. */
export function WorkAssist({ downloadId }: { downloadId: number }) {
  // 作品を切り替えたら選択状態と未完了要求の受け口を分離する。
  return <WorkAssistForWork key={downloadId} downloadId={downloadId} />;
}

function WorkAssistForWork({ downloadId }: { downloadId: number }) {
  const runtime = isTauriRuntime();
  const synopsis = useAssist("work_synopsis");
  const tagging = useAssist("work_tagging");
  const queryClient = useQueryClient();
  const [active, setActive] = useState<WorkAssistAction | null>(null);
  const [proposals, setProposals] = useState<TagProposal[] | null>(null);
  const [picked, setPicked] = useState<ReadonlySet<string>>(new Set());
  const [relatedPicked, setRelatedPicked] = useState<ReadonlySet<string>>(new Set());
  const noteKey = ["assist-note", "work", String(downloadId), "synopsis"] as const;
  const noteEngine = synopsis.settings ? toEngine(synopsis.settings, "work_synopsis") : null;
  const tagsKey = ["assist-tags", downloadId] as const;
  const refreshTagQueries = async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: tagsKey }),
      queryClient.invalidateQueries({ queryKey: ["library"] }),
      queryClient.invalidateQueries({ queryKey: ["reader-metadata", downloadId] }),
    ]);
  };
  const reportTagUpdateError = async (error: unknown) => {
    await refreshTagQueries();
    notifications.show({ color: "red", title: "タグの更新を確認してください", message: errorMessage(error) });
  };

  const note = useQuery({ queryKey: [...noteKey, noteEngine], queryFn: () => loadNote("work", String(downloadId), "synopsis", noteEngine) });
  const tags = useQuery({ queryKey: tagsKey, queryFn: () => workTags(downloadId) });
  const assisted = (tags.data ?? []).filter((tag) => tag.source === "llm");
  const manuallyAdded = (tags.data ?? []).filter((tag) => tag.source === "manual");
  const existingTagNames = new Set((tags.data ?? []).map((tag) => tag.name));

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
    onError: reportTagUpdateError,
  });
  const remove = useMutation({
    mutationFn: (tag: string) => removeAssistedTag(downloadId, tag),
    onSuccess: (next) => {
      queryClient.setQueryData(tagsKey, next);
      queryClient.invalidateQueries({ queryKey: ["library"] });
      queryClient.invalidateQueries({ queryKey: ["reader-metadata", downloadId] });
    },
    onError: reportTagUpdateError,
  });
  const related = useMutation({
    mutationFn: () => relatedTagCandidates(downloadId),
    onMutate: () => setRelatedPicked(new Set()),
    onSuccess: (found) => {
      if (found.length === 0) notifications.show({ message: "一緒に付いているタグからは、候補が見つかりませんでした" });
    },
    onError: (error) => notifications.show({ color: "red", title: "一緒に付くタグの候補を読み込めません", message: errorMessage(error) }),
  });
  const newRelatedCandidates = related.data?.filter((candidate) => !existingTagNames.has(candidate.tag));
  const visibleRelatedTags = new Set(newRelatedCandidates?.map((candidate) => candidate.tag) ?? []);
  const validRelatedPicked = [...relatedPicked].filter((tag) => visibleRelatedTags.has(tag));
  const acceptRelated = useMutation({
    mutationFn: () => acceptRelatedTags(downloadId, validRelatedPicked),
    onSuccess: (next) => {
      related.reset();
      setRelatedPicked(new Set());
      queryClient.setQueryData(tagsKey, next);
      queryClient.invalidateQueries({ queryKey: ["library"] });
      queryClient.invalidateQueries({ queryKey: ["reader-metadata", downloadId] });
      notifications.show({ color: "green", message: "選んだタグを手動で付けました" });
    },
    onError: reportTagUpdateError,
  });
  const removeManual = useMutation({
    mutationFn: (tag: string) => removeManualTag(downloadId, tag),
    onSuccess: (next) => {
      queryClient.setQueryData(tagsKey, next);
      queryClient.invalidateQueries({ queryKey: ["library"] });
      queryClient.invalidateQueries({ queryKey: ["reader-metadata", downloadId] });
    },
    onError: reportTagUpdateError,
  });

  const synopsisReady = Boolean(synopsis.engine && synopsis.body);
  const launcherItems = [
    {
      id: "work_synopsis",
      label: note.data ? "あらすじを確認・作り直す" : "あらすじを作る",
      description: "本文から、読み返すためのあらすじを作ります",
      enabled: synopsisReady,
      unavailableReason: synopsis.engine ? "設定で本文の送信を許可してください" : "設定で「あらすじ」を有効にしてください",
      onSelect: () => setActive("synopsis" as const),
      badge: note.data ? "保存済み" : undefined,
    },
    {
      id: "work_tagging",
      label: "タグを補完する",
      description: "棚にあるタグから、足りなそうなものを挙げます",
      enabled: Boolean(tagging.engine),
      unavailableReason: "設定で「タグの補完」を有効にしてください",
      onSelect: () => setActive("tags" as const),
      badge: assisted.length ? `${assisted.length}件採用済み` : undefined,
    },
    {
      id: "related_tags",
      label: "一緒に付くタグ",
      description: "ほかの作品で一緒に付いているタグから候補を出します。モデルは使いません",
      enabled: true,
      onSelect: () => setActive("related-tags"),
      badge: manuallyAdded.length ? `手動で${manuallyAdded.length}件` : undefined,
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
          sourceTail={note.data.sourceTail}
          claims={note.data.claims}
          modelId={note.data.modelId}
          createdAt={note.data.createdAt}
          promptVersion={note.data.promptVersion}
          promptStale={note.data.promptStale}
          inputStale={note.data.inputStale}
          configStale={note.data.configStale}
          onDiscard={() => discard.mutate()}
        />
      )}
      {assisted.length > 0 && (
        <Group gap={6} wrap="wrap">
          <Text size="xs" c="dimmed">モデルの案から採ったタグ</Text>
          {assisted.map((tag) => (
            <Tooltip key={tag.name} label="モデルの案から付けたタグを外す">
              <Badge component="button" type="button" aria-label={`モデルの案から付けたタグ「${tag.name}」を外す`} variant="light" color="grape" style={{ cursor: "pointer" }} onClick={() => remove.mutate(tag.name)}>{tag.name}</Badge>
            </Tooltip>
          ))}
        </Group>
      )}
      {manuallyAdded.length > 0 && (
        <Group gap={6} wrap="wrap">
          <Text size="xs" c="dimmed">手動で追加したタグ</Text>
          {manuallyAdded.map((tag) => (
            <Tooltip key={tag.name} label="手動タグを外す">
              <Badge component="button" type="button" aria-label={`手動タグ「${tag.name}」を外す`} variant="light" color="teal" style={{ cursor: "pointer" }} onClick={() => removeManual.mutate(tag.name)}>{tag.name}</Badge>
            </Tooltip>
          ))}
        </Group>
      )}

      <Modal opened={active !== null} onClose={() => setActive(null)} title={active === "tags" ? "タグの補完" : active === "related-tags" ? "一緒に付くタグ" : "あらすじ"} size="lg">
        {active === "synopsis" ? (
          <Stack>
            <Text size="sm" c="dimmed">本文を読んで、結末までのあらすじを400字以内で書きます。文ごとに原文の根拠を付け、別の確認で原文と照らします。文脈の小さいモデルでは、本文の文を選ぶ抜粋になります。</Text>
            {note.data && <AssistNoteBody text={note.data.text} sourceTail={note.data.sourceTail} claims={note.data.claims} modelId={note.data.modelId} createdAt={note.data.createdAt} promptVersion={note.data.promptVersion} promptStale={note.data.promptStale} inputStale={note.data.inputStale} configStale={note.data.configStale} />}
            <Group justify="flex-end"><Button loading={summarize.isPending} disabled={!synopsisReady} onClick={() => summarize.mutate()}>{note.data ? "作り直す" : "あらすじを作る"}</Button></Group>
          </Stack>
        ) : active === "tags" ? (
          <Stack>
            <Text size="sm" c="dimmed">新しい語は作らず、棚にすでにあるタグだけを候補にします。採用するまでは作品を変更しません。</Text>
            {tagging.settings?.featureProfiles.work_tagging.inputPolicy.includeExcerpt === false && (
              <Text size="sm" c="dimmed">いまは概要を送らない設定なので、題名と付いているタグだけで候補を探します。作品の内容も使いたいときは、設定の「AIの手伝い」で「タグの補完」の送る項目に概要を入れてください。</Text>
            )}
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
        ) : active === "related-tags" ? (
          <Stack>
            <Text size="sm" c="dimmed">候補は、ほかの作品で取得元のタグが一緒に付いている数から作ります。候補作成時はこの作品の本文や概要を読みません。外部へ通信せず、候補が内容に合う保証もありません。選んで採用するまでタグは変わりません。</Text>
            <Group justify="flex-end">
              <Button loading={related.isPending} onClick={() => related.mutate()}>{related.data ? "候補を再読み込み" : "候補を読み込む"}</Button>
            </Group>
            {related.isError && <Text role="alert" size="sm" c="red">{errorMessage(related.error)}。再試行できます。</Text>}
            {related.data && newRelatedCandidates?.length === 0 && !related.isPending && !related.isError && <Text size="sm" c="dimmed">新しく追加できる候補はありません。再読み込みできます。</Text>}
            {newRelatedCandidates && newRelatedCandidates.length > 0 && <Divider />}
            {newRelatedCandidates?.map((candidate: RelatedTagCandidate) => (
              <Checkbox
                key={candidate.tag}
                checked={relatedPicked.has(candidate.tag)}
                disabled={related.isPending || related.isError}
                onChange={(event) => setRelatedPicked((current) => {
                  const next = new Set(current);
                  if (event.currentTarget.checked) next.add(candidate.tag); else next.delete(candidate.tag);
                  return next;
                })}
                label={<span><Text component="span" size="sm" fw={650}>{candidate.tag}</Text><Text size="xs" c="dimmed">{candidate.reason}</Text></span>}
              />
            ))}
            {newRelatedCandidates && newRelatedCandidates.length > 0 && <Group justify="flex-end"><Button disabled={!validRelatedPicked.length || related.isPending || related.isError} loading={acceptRelated.isPending} onClick={() => acceptRelated.mutate()}>選んだ{validRelatedPicked.length}件を手動で付ける</Button></Group>}
          </Stack>
        ) : null}
      </Modal>
    </Stack>
  );
}
