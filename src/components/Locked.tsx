import { cloneElement, type MouseEvent, type ReactElement } from "react";
import { Tooltip } from "@mantine/core";

type Lockable = ReactElement<{
  onClick?: (event: MouseEvent) => void;
  disabled?: boolean;
  "aria-label"?: string;
}>;

/**
 * 押せない操作を、押せない理由を添えて同じ場所に置く。
 *
 * Mantine の `disabled` はポインターの事象ごと消すので、ツールチップが出ず、押すと下の
 * カードやリンクに抜ける。ここでは見た目だけ押せない形（`data-disabled`）にして、押しても
 * 何も起きないようにし、理由をツールチップと読み上げに出す。`reason` が無ければそのまま。
 */
export function Locked({ reason, label, children }: { reason: string | null | undefined; label: string; children: Lockable }) {
  if (!reason) return children;
  const text = `${label}（${reason}）`;
  return (
    <Tooltip label={text}>
      {cloneElement(children, {
        disabled: false,
        "data-disabled": true,
        "aria-disabled": true,
        "aria-label": text,
        onClick: (event: MouseEvent) => {
          event.preventDefault();
          event.stopPropagation();
        },
      } as Record<string, unknown>)}
    </Tooltip>
  );
}
