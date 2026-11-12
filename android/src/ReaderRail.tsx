import { useRef, useState, type CSSProperties, type KeyboardEvent, type PointerEvent } from "react";

/**
 * 本文の右端に縦に置く進み具合。
 *
 * 読んでいるあいだは薄い灰色の短い印だけを出し、作品全体のどこにいるかを示す（触れない）。
 * メニューを出しているときは、同じ場所がつまみになり、上下に動かして位置を変えられる。
 * 横向きの棒は本文の幅を取り、右端の印が画面からはみ出した。縦にすれば下の列はボタンだけで済む。
 */
export function ReaderRail({ progress, interactive, label, onPreview, onCommit }: {
  /** 0 から 1。動かしている最中は、指の位置の値。 */
  progress: number;
  interactive: boolean;
  /** 動かしている最中に、つまみの横へ出す文字。 */
  label: string;
  onPreview: (value: number) => void;
  onCommit: (value: number) => void;
}) {
  const trackRef = useRef<HTMLDivElement>(null);
  const [dragging, setDragging] = useState(false);
  // 指を離したあとに届く取りこぼしの知らせで、二度動かさない。
  const active = useRef(false);
  const value = Math.min(1, Math.max(0, progress));

  const valueAt = (clientY: number) => {
    const track = trackRef.current?.getBoundingClientRect();
    if (!track || track.height <= 0) return value;
    return Math.min(1, Math.max(0, (clientY - track.top) / track.height));
  };

  const down = (event: PointerEvent<HTMLDivElement>) => {
    if (!interactive) return;
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    active.current = true;
    setDragging(true);
    onPreview(valueAt(event.clientY));
  };
  const move = (event: PointerEvent<HTMLDivElement>) => {
    if (!active.current) return;
    onPreview(valueAt(event.clientY));
  };
  const up = (event: PointerEvent<HTMLDivElement>) => {
    if (!active.current) return;
    active.current = false;
    setDragging(false);
    onCommit(valueAt(event.clientY));
  };
  const cancel = () => {
    if (!active.current) return;
    active.current = false;
    setDragging(false);
    onCommit(value);
  };
  const key = (event: KeyboardEvent<HTMLDivElement>) => {
    const steps: Record<string, number> = { ArrowDown: 0.01, ArrowRight: 0.01, ArrowUp: -0.01, ArrowLeft: -0.01, PageDown: 0.1, PageUp: -0.1 };
    const step = steps[event.key];
    const next = event.key === "Home" ? 0 : event.key === "End" ? 1 : step === undefined ? null : Math.min(1, Math.max(0, value + step));
    if (next === null) return;
    event.preventDefault();
    event.stopPropagation();
    onCommit(next);
  };

  return <div className="reader-rail" data-interactive={interactive || undefined} data-dragging={dragging || undefined} style={{ "--reader-rail-value": value } as CSSProperties}
    role="slider" aria-orientation="vertical" aria-label="読書位置" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(value * 100)} aria-valuetext={label}
    aria-hidden={!interactive} tabIndex={interactive ? 0 : -1}
    onPointerDown={down} onPointerMove={move} onPointerUp={up} onPointerCancel={cancel} onLostPointerCapture={cancel} onKeyDown={key}>
    <div className="reader-rail-track" ref={trackRef}>
      <span className="reader-rail-fill" />
      <span className="reader-rail-thumb">{dragging && <span className="reader-rail-bubble">{label}</span>}</span>
    </div>
  </div>;
}
