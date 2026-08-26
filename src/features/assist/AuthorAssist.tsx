import { notifications } from "@mantine/notifications";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { errorMessage } from "@/lib/format";
import { AssistNoteBody, AssistPanel } from "@/features/assist/AssistPanel";
import { useAssist } from "@/features/assist/useAssist";
import { deleteNote, describeAuthor, loadNote } from "@/services/assistApi";
import { isTauriRuntime } from "@/services/dbApi";

/**
 * 作者ひとりぶんの作風メモ。
 *
 * 本文は送らない。この棚の題名はそれ自体があらすじのように書かれているので、
 * 題名とタグだけで作風は言い当てられる。
 */
export function AuthorAssist({ source, personKey }: { source: string; personKey: string }) {
  const { engine } = useAssist();
  const queryClient = useQueryClient();
  const subjectKey = `${source}:${personKey}`;
  const key = ["assist-note", "person", subjectKey, "style"] as const;

  // 保存してあるものを読むだけ。ここではモデルを呼ばない。
  const note = useQuery({
    queryKey: key,
    queryFn: () => loadNote("person", subjectKey, "style"),
    enabled: isTauriRuntime(),
  });

  const run = useMutation({
    mutationFn: () => {
      if (!engine) return Promise.reject(new Error("モデルの手伝いが設定されていません"));
      return describeAuthor(engine, source, personKey);
    },
    onSuccess: () => queryClient.invalidateQueries({ queryKey: key }),
    onError: (error) => notifications.show({ color: "red", title: "作風をまとめられません", message: errorMessage(error) }),
  });
  const discard = useMutation({
    mutationFn: () => deleteNote("person", subjectKey, "style"),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: key }),
  });

  if (!isTauriRuntime()) return null;
  return (
    <AssistPanel
      title="作風のメモ"
      hint="この作者の作品の題名とタグから、よく書いているものを2文でまとめます。本文は送りません。"
      engineReady={Boolean(engine)}
      busy={run.isPending}
      actionLabel={note.data ? "まとめ直す" : "作風をまとめてもらう"}
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
