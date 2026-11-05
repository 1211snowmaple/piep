import type { ComponentPropsWithoutRef } from "react";
import { Button, type ButtonProps } from "@mantine/core";

/**
 * 解除・削除・中止のボタン。白地に赤枠と赤文字で、地を赤く塗らない。
 *
 * 消す操作がいちばん目立つと、ふだんの操作の邪魔をして押し間違えも増える。
 * 再試行は赤くしない（`docs/policy/05-frontend.md`）。
 */
export function DangerButton({ className, ...props }: ButtonProps & ComponentPropsWithoutRef<"button">) {
  return <Button {...props} variant="outline" color="red" className={className ? `button-danger ${className}` : "button-danger"} />;
}

/** 確認ダイアログの決定にも同じ形を使う。 */
export const DANGER_CONFIRM = { variant: "outline", color: "red", className: "button-danger" } as const;
