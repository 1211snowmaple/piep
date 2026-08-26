import { notifications } from "@mantine/notifications";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { errorMessage } from "@/lib/format";
import { AssistNoteBody, AssistPanel } from "@/features/assist/AssistPanel";
import { useAssist } from "@/features/assist/useAssist";
import { deleteNote, loadNote, recapPrevious } from "@/services/assistApi";
import { isTauriRuntime } from "@/services/dbApi";

/**
 * 前の話の要点を、読み始める前に出す。
 *
 * 束や公式シリーズに**読む順があるときだけ**出す。順序なしのまとまりで
 * 「前回のあらすじ」を出すと、無い連続性をあるように見せてしまう。
 *
 * 一度作れば残り、次に開いたときはモデルを呼ばずに出る。
 */
export function RecapPanel({
  currentId,
  previous,
}: {
  currentId: number;
  /** 直前の話。順序のある文脈でだけ渡す。 */
  previous: { id: number; title: string };
}) {
  const { engine, body } = useAssist();
  const queryClient = useQueryClient();
  const subjectKey = `${currentId}:${previous.id}`;
  const key = ["assist-note", "work", subjectKey, "recap"] as const;

  // 保存してあるものを読むだけ。開いただけではモデルを呼ばない。
  const note = useQuery({
    queryKey: key,
    queryFn: () => loadNote("work", subjectKey, "recap"),
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
      title="前回のあらすじ"
      hint={`「${previous.title}」の要点を3文で。間を空けて続きを読むときに。`}
      engineReady={Boolean(engine)}
      needsBody
      bodyAllowed={body}
      busy={run.isPending}
      actionLabel={note.data ? "作り直す" : "前の話を思い出す"}
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
