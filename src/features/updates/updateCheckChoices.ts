import { useLocalStorage } from "@mantine/hooks";
import { useCallback } from "react";

/** 「新作・改稿」の画面で、確認を始める前に選ぶもの。 */
export type UpdateCheckChoices = {
  scope: string;
  mode: "check_only" | "auto_save";
  depth: "new" | "full";
};

const STORAGE_KEY = "piep.updates.check-choices";
export const updateCheckChoiceDefaults: UpdateCheckChoices = { scope: "all", mode: "check_only", depth: "new" };
const FIXED_SCOPES = new Set(["all", "work", "author", "series", "favorite", "reading"]);

/** 残っていた値を読む。知らない値は既定へ戻す（壊れた値で画面を止めない）。 */
export const parseUpdateCheckChoices = (raw: unknown): UpdateCheckChoices => {
  const value = raw && typeof raw === "object" ? raw as Record<string, unknown> : {};
  const scope = typeof value.scope === "string" && (FIXED_SCOPES.has(value.scope) || /^group:.{1,200}$/.test(value.scope))
    ? value.scope
    : updateCheckChoiceDefaults.scope;
  return {
    scope,
    mode: value.mode === "auto_save" ? "auto_save" : "check_only",
    depth: value.depth === "full" ? "full" : "new",
  };
};

/**
 * 選んだ範囲・内容・処理方法を、画面を離れても覚えておく。
 * 初めの描画から覚えた値を出す（既定を一度出してから切り替えると、ボタンが動いて見える）。
 */
export function useUpdateCheckChoices(): [UpdateCheckChoices, (choices: UpdateCheckChoices) => void] {
  const [stored, setStored] = useLocalStorage<unknown>({
    key: STORAGE_KEY,
    defaultValue: updateCheckChoiceDefaults,
    getInitialValueInEffect: false,
  });
  const remember = useCallback((choices: UpdateCheckChoices) => setStored(parseUpdateCheckChoices(choices)), [setStored]);
  return [parseUpdateCheckChoices(stored), remember];
}
