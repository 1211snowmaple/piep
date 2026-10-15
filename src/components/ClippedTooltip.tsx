import { cloneElement, useEffect, useRef, useState, type ReactElement, type Ref } from "react";
import { Tooltip, type TooltipProps } from "@mantine/core";
import { useMergedRef } from "@mantine/hooks";

/** 文字が切れる側の要素。押せるようにするための外側の箱ではない。 */
const CLAMPED = ".line-clamp-1, .line-clamp-2, .line-clamp-3";

function clampedNode(root: HTMLElement) {
  if (root.matches(CLAMPED)) return root;
  return root.querySelector<HTMLElement>(CLAMPED) ?? root;
}

/** 1px は端数。字形の丸めで scrollWidth が clientWidth をわずかに超える。 */
function isClipped(root: HTMLElement) {
  const node = clampedNode(root);
  return node.scrollWidth > node.clientWidth + 1 || node.scrollHeight > node.clientHeight + 1;
}

interface ClippedTooltipProps extends Omit<TooltipProps, "children" | "label"> {
  /** 切れる前の全文。切れていなければ出さないので、常に文字列でよい。 */
  label: string;
  children: ReactElement<{ ref?: Ref<HTMLElement> }>;
}

/**
 * 切れてしまった文字を、切れたときだけ読ませる吹き出し。
 *
 * 以前は題名・シリーズ・あらすじに無条件で付いていた。全部見えている文字の
 * 上に同じ文字をもう一度出しても読む物は増えず、そのうえ吹き出しは真上
 * ——つまりシリーズ名や一つ上のカード——を隠す。`ExpandableText` が
 * 「隠れている物が無いなら続きを読むボタンも出さない」としているのと同じ理由で、
 * ここも隠れている物があるときだけ出す。
 *
 * 的は文字そのものに置くこと。行いっぱいに広げた箱に付けると、短い題名では
 * 吹き出しが文字ではなく箱の中央——何も無い右方——に出る。
 */
export function ClippedTooltip({ label, children, disabled, ...props }: ClippedTooltipProps) {
  const ref = useRef<HTMLElement>(null);
  const mergedRef = useMergedRef(ref, children.props.ref);
  const [clipped, setClipped] = useState(false);

  useEffect(() => {
    const element = ref.current;
    if (!element) return;
    const measure = () => setClipped(isClipped(element));
    measure();
    if (typeof ResizeObserver === "undefined") return;
    // 同じ文字でも、幅が変われば折り返しが変わる。窓の伸縮、サイドバーの
    // 開け閉め、表示の切り替え——どれでも変わる。
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [label]);

  return (
    <Tooltip
      label={label}
      multiline
      maw={480}
      openDelay={350}
      withArrow
      disabled={disabled || !clipped}
      {...props}
    >
      {cloneElement(children, { ref: mergedRef })}
    </Tooltip>
  );
}
