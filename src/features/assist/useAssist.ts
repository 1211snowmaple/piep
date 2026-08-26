import { useQuery } from "@tanstack/react-query";
import { assistReady, loadAssistSettings, toEngine, type AssistEngine, type AssistSettings } from "@/services/assistApi";

/**
 * モデルの手伝いが使える状態か。
 *
 * **使えないときは `engine` が `null` になる。** 画面側はそれを見て、
 * 手伝いのボタンそのものを出さない。設定していない人の画面に、押しても
 * 断られるだけのボタンを置かないための約束である。
 *
 * `body` は本文を送ってよいかで、別に許可が要る。あらすじと前回のあらすじだけが
 * これを見る。
 */
export function useAssist(): {
  settings: AssistSettings | undefined;
  engine: AssistEngine | null;
  body: boolean;
  loading: boolean;
} {
  const query = useQuery({ queryKey: ["naming-settings"], queryFn: loadAssistSettings });
  const settings = query.data;
  const ready = assistReady(settings);
  return {
    settings,
    engine: ready ? toEngine(settings) : null,
    body: ready ? settings.allowBody : false,
    loading: query.isLoading,
  };
}
