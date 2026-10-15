import { notifications } from "@mantine/notifications";
import { Alert } from "@mantine/core";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { errorMessage } from "@/lib/format";
import { AssistNoteBody, AssistPanel } from "@/features/assist/AssistPanel";
import { useAssist } from "@/features/assist/useAssist";
import { deleteNote, loadNote, recapPrevious, toEngine } from "@/services/assistApi";
import { isTauriRuntime } from "@/services/dbApi";

/**
 * 前の話の要点を、読み始める前に出す。
 *
 * 束や公式シリーズに**読む順があるときだけ**出す。順序なしのまとまりで
 * 「前回のあらすじ」を出すと、無い連続性をあるように見せてしまう。
 *
 * 一度作れば残り、次に開いたときはモデルを呼ばずに出る。
 */
type RecapProps = {
  currentId: number;
  previous: { id: number; title: string };
};

export function RecapPanel(props: RecapProps) {
  return <RecapForPair key={`${props.currentId}:${props.previous.id}`} {...props} />;
}

function RecapForPair({
  currentId,
  previous,
}: RecapProps) {
  const { engine, body, settings } = useAssist("reader_recap");
  const queryClient = useQueryClient();
  const subjectKey = `${currentId}:${previous.id}`;
  const key = ["assist-note", "work", subjectKey, "recap"] as const;
  const noteEngine = settings ? toEngine(settings, "reader_recap") : null;

  // 保存してあるものを読むだけ。開いただけではモデルを呼ばない。
  const note = useQuery({
    queryKey: [...key, noteEngine],
    queryFn: () => loadNote("work", subjectKey, "recap", noteEngine),
    enabled: isTauriRuntime(),
  });

  const run = useMutation({
    mutationFn: () => {
      if (!engine) return Promise.reject(new Error("モデルの手伝いが設定されていません"));
      return recapPrevious(engine, previous.id, currentId);
    },
    onSuccess: () => queryClient.invalidateQueries({ queryKey: key }),
    onError: (error) =>
      notifications.show({ color: "red", title: "前回のあらすじを作れません", message: errorMessage(error) }),
  });
  const discard = useMutation({
    mutationFn: () => deleteNote("work", subjectKey, "recap"),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: key }),
  });

  if (!isTauriRuntime()) return null;
  return (
    <AssistPanel
      featureId="reader_recap"
      title="前回のあらすじ"
      hint={`「${previous.title}」の本文から要点の文を選びます。間を空けて続きを読むときに。`}
      engineReady={Boolean(engine)}
      needsBody
      bodyAllowed={body}
      busy={run.isPending}
      actionLabel={note.data ? "作り直す" : "前の話を思い出す"}
      onRun={() => run.mutate()}
    >
      {note.data && (
        (note.data.promptStale || note.data.inputStale || note.data.configStale === true) && (
          <Alert color="yellow" title="保存済みのあらすじは古い可能性があります" p="xs">
            本文、指示、またはモデル設定が作成後に変わっています。「作り直す」で更新できます。
          </Alert>
        )
      )}
      {note.data && (
        <AssistNoteBody
          text={note.data.text}
          sourceTail={note.data.sourceTail}
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
