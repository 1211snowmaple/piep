/** デスクトップ版の mark.svg とワードマークの寸法・色をそのまま使う。 */
export function PiepLockup({ size = 24 }: { size?: number }) {
  return <span className="piep-lockup" style={{ "--lockup-size": `${size}px` } as React.CSSProperties} aria-label="piep">
    <img src="/desktop-mark.svg" alt="" className="piep-lockup__mark" />
    <span className="piep-lockup__word" aria-hidden="true"><span className="piep-lockup__pi">pi</span><span className="piep-lockup__ep">ep</span></span>
  </span>;
}
