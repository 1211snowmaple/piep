import piepMark from "@shared/brand/mark.svg";
import "@shared/brand/lockup.css";

/** 図とワードマークの寸法・色は、デスクトップ版と同じ定義（shared/brand）を使う。 */
export function PiepLockup({ size = 24 }: { size?: number }) {
  return <span className="piep-lockup" style={{ "--lockup-size": `${size}px` } as React.CSSProperties} aria-label="piep">
    <img src={piepMark} alt="" className="piep-lockup__mark" />
    <span className="piep-lockup__word" aria-hidden="true"><span className="piep-lockup__pi">pi</span><span className="piep-lockup__ep">ep</span></span>
  </span>;
}
