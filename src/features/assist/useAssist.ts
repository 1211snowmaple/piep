import { useQuery } from "@tanstack/react-query";
import { assistFeatureReady, loadAssistSettings, toEngine, type AssistEngine, type AssistFeatureId, type AssistSettings } from "@/services/assistApi";

/**
 * モデルの手伝いが使える状態か。
 *
 * **使えないときは `engine` が `null` になる。** 画面側は入口を消さず、
 * 共通ランチャー内で不足条件を説明する。
 *
 * `body` は本文を送ってよいかで、別に許可が要る。あらすじと前回のあらすじだけが
 * これを見る。
 */
export function useAssist(featureId: AssistFeatureId): {
  settings: AssistSettings | undefined;
  engine: AssistEngine | null;
  body: boolean;
  loading: boolean;
} {
  const query = useQuery({ queryKey: ["naming-settings"], queryFn: loadAssistSettings });
  const settings = query.data;
  const ready = assistFeatureReady(settings, featureId);
  return {
    settings,
    engine: ready ? toEngine(settings, featureId) : null,
    body: ready ? settings.allowBody : false,
    loading: query.isLoading,
  };
}
