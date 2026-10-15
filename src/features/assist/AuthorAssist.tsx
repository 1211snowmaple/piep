import { notifications } from "@mantine/notifications";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { errorMessage } from "@/lib/format";
import { AssistNoteBody, AssistPanel } from "@/features/assist/AssistPanel";
import { useAssist } from "@/features/assist/useAssist";
import { deleteNote, describeAuthor, loadNote, toEngine } from "@/services/assistApi";
import { isTauriRuntime } from "@/services/dbApi";

/**
 * 対象作品のタグ件数をまとめる。本文の文体や作者全作品への推測はしない。
 */
export function AuthorAssist({ source, personKey }: { source: string; personKey: string }) {
  return <AuthorAssistForPerson key={`${source}:${personKey}`} source={source} personKey={personKey} />;
}

function AuthorAssistForPerson({ source, personKey }: { source: string; personKey: string }) {
  const { engine, settings } = useAssist("author_style");
  const queryClient = useQueryClient();
  const subjectKey = `${source}:${personKey}`;
  const key = ["assist-note", "person", subjectKey, "style"] as const;
  const noteEngine = settings ? toEngine(settings, "author_style") : null;

  // 保存してあるものを読むだけ。ここではモデルを呼ばない。
  const note = useQuery({
    queryKey: [...key, noteEngine],
    queryFn: () => loadNote("person", subjectKey, "style", noteEngine),
    enabled: isTauriRuntime(),
  });

  const run = useMutation({
    mutationFn: () => {
      if (!engine) return Promise.reject(new Error("作者の題材メモが設定されていません"));
      return describeAuthor(engine, source, personKey);
    },
    onSuccess: () => queryClient.invalidateQueries({ queryKey: key }),
    onError: (error) => notifications.show({ color: "red", title: "題材をまとめられません", message: errorMessage(error) }),
  });
  const discard = useMutation({
    mutationFn: () => deleteNote("person", subjectKey, "style"),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: key }),
  });

  if (!isTauriRuntime()) return null;
  return (
    <AssistPanel
      featureId="author_style"
      native
      title="題材のメモ"
      hint="モデルや通信を使わず、手元の作品タグと件数をまとめます。本文の文体や、この棚にない作品については判断しません。"
      engineReady={Boolean(engine)}
      busy={run.isPending}
      actionLabel={note.data ? "まとめ直す" : "題材をまとめる"}
      onRun={() => run.mutate()}
      placement="header"
      registrationKey={`author-style-${source}-${personKey}`}
    >
      {note.data && (
        <AssistNoteBody
          text={note.data.text}
          modelId={note.data.modelId}
          createdAt={note.data.createdAt}
          promptVersion={note.data.promptVersion}
          promptStale={note.data.promptStale}
          inputStale={note.data.inputStale}
          configStale={note.data.configStale}
          onDiscard={() => discard.mutate()}
        />
      )}
    </AssistPanel>
  );
}
