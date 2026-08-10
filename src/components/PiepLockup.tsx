import piepMark from "@/assets/mark.svg";

/**
 * The lockup is laid out in HTML rather than shipped as one SVG.
 *
 * An SVG has to declare a fixed viewBox, and the wordmark's width depends on
 * which font actually resolves, so the box always carried dead space on one
 * side - which made the brand sit off-centre in the rail no matter how it was
 * aligned. Composed here, the box is exactly as wide as its contents.
 */
/** `size` is the wordmark's type size; the mark and gap scale from it. */
export function PiepLockup({ size = 26, className }: { size?: number; className?: string }) {
  return (
    <span className={["piep-lockup", className].filter(Boolean).join(" ")} style={{ "--lockup-size": `${size}px` } as React.CSSProperties} aria-label="piep">
      <img src={piepMark} alt="" className="piep-lockup__mark" />
      <span className="piep-lockup__word" aria-hidden>
        <span className="piep-lockup__pi">pi</span><span className="piep-lockup__ep">ep</span>
      </span>
    </span>
  );
}
