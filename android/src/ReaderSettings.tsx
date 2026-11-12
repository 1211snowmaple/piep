import { Icons } from "./icons";
import { typeLimits, type ReaderBehavior, type ReaderFont, type ReaderTheme, type ReaderType } from "./readerPrefs";

const themes: { id: ReaderTheme; label: string }[] = [
  { id: "white", label: "白" },
  { id: "paper", label: "紙の色" },
  { id: "night", label: "ナイト" },
];

const fonts: { id: ReaderFont; label: string }[] = [
  { id: "sans", label: "ゴシック" },
  { id: "serif", label: "明朝" },
];

function round(value: number, digits = 2): number {
  const scale = 10 ** digits;
  return Math.round(value * scale) / scale;
}

function SliderRow({ label, value, display, min, max, step, stepBy = step, onChange, decrease, increase, className }: {
  label: string;
  value: number;
  display: string;
  min: number;
  max: number;
  step: number;
  stepBy?: number;
  onChange: (value: number) => void;
  decrease: string;
  increase: string;
  className?: string;
}) {
  const clamp = (next: number) => Math.min(max, Math.max(min, round(next)));
  return <div className={className ? `reader-slider ${className}` : "reader-slider"}>
    <div className="reader-control-heading"><span>{label}</span><strong>{display}</strong></div>
    <div className="reader-slider-row">
      <button type="button" className="reader-step" disabled={value <= min} onClick={() => onChange(clamp(value - stepBy))} aria-label={decrease}><Icons.remove aria-hidden="true" /></button>
      <input type="range" min={min} max={max} step={step} value={value} aria-label={label} onChange={(event) => onChange(Number(event.target.value))} />
      <button type="button" className="reader-step" disabled={value >= max} onClick={() => onChange(clamp(value + stepBy))} aria-label={increase}><Icons.add aria-hidden="true" /></button>
    </div>
  </div>;
}

function Segmented<T extends string>({ label, value, options, onChange, help }: {
  label: string;
  value: T;
  options: { id: T; label: string }[];
  onChange: (value: T) => void;
  help?: string;
}) {
  return <div className="reader-choice">
    <div className="reader-control-heading"><span>{label}</span></div>
    <div className="reader-segmented" role="group" aria-label={label} data-count={options.length}>
      {options.map((option) => <button key={option.id} type="button" aria-pressed={value === option.id} onClick={() => onChange(option.id)}>{option.label}</button>)}
    </div>
    {help && <p className="reader-control-help">{help}</p>}
  </div>;
}

function Toggle({ label, help, checked, onChange }: { label: string; help?: string; checked: boolean; onChange: (checked: boolean) => void }) {
  return <button type="button" className="reader-toggle" role="switch" aria-checked={checked} onClick={() => onChange(!checked)}>
    <span className="reader-toggle-text"><span>{label}</span>{help && <small>{help}</small>}</span>
    <span className="reader-toggle-track" aria-hidden="true"><span /></span>
  </button>;
}

/** 文字と色の設定。読書画面の中でも設定画面でも同じものを使う。 */
export function ReaderTypeControls({ value, onChange }: { value: ReaderType; onChange: (patch: Partial<ReaderType>) => void }) {
  return <div className="reader-controls">
    <div className="reader-choice">
      <div className="reader-control-heading"><span>読書背景</span></div>
      <div className="reader-swatches" role="group" aria-label="読書背景">
        {themes.map((theme) => <button key={theme.id} type="button" className={`reader-swatch reader-swatch--${theme.id}`} aria-pressed={value.readerTheme === theme.id} onClick={() => onChange({ readerTheme: theme.id })}><span aria-hidden="true">あ</span><small>{theme.label}</small></button>)}
      </div>
    </div>
    <div className="reader-choice">
      <div className="reader-control-heading"><span>書体</span></div>
      <div className="reader-font-choices" role="group" aria-label="書体">
        {fonts.map((font) => <button key={font.id} type="button" className={`reader-font--${font.id}`} aria-pressed={value.fontFamily === font.id} onClick={() => onChange({ fontFamily: font.id })}><span aria-hidden="true">永あ</span><small>{font.label}</small></button>)}
      </div>
    </div>
    <SliderRow label="文字サイズ" value={value.fontSize} display={`${value.fontSize}px`} {...typeLimits.fontSize} onChange={(fontSize) => onChange({ fontSize })} decrease="文字を小さくする" increase="文字を大きくする" />
    <SliderRow label="行間" value={value.lineHeight} display={value.lineHeight.toFixed(2)} {...typeLimits.lineHeight} stepBy={0.1} onChange={(lineHeight) => onChange({ lineHeight })} decrease="行間を狭くする" increase="行間を広くする" />
    <SliderRow className="reader-slider--wide-only" label="本文幅" value={value.readerWidth} display={`${value.readerWidth}px`} {...typeLimits.readerWidth} onChange={(readerWidth) => onChange({ readerWidth })} decrease="本文幅を狭くする" increase="本文幅を広くする" />
  </div>;
}

/** 読み方とページの送り方。 */
export function ReaderBehaviorControls({ value, onChange }: { value: ReaderBehavior; onChange: (patch: Partial<ReaderBehavior>) => void }) {
  return <div className="reader-controls">
    <Segmented label="読み方" value={value.layout} options={[{ id: "pages", label: "ページごと" }, { id: "continuous", label: "つなげて読む" }]} onChange={(layout) => onChange({ layout })} help={value.layout === "continuous" ? "ページの区切りなしで、最初から最後まで縦に続けて読みます。" : "ページごとに区切り、ページボタンで移動できます。"} />
    <Segmented label="画面をタップしたとき" value={value.tap} options={[{ id: "sides", label: "左右で送る" }, { id: "updown", label: "上下で送る" }, { id: "off", label: "送らない" }]} onChange={(tap) => onChange({ tap })} help={value.tap === "off" ? "タップするとメニューを出し入れします。" : "画面の端で前後へ送り、真ん中でメニューを出し入れします。"} />
    <Toggle label="左右のスワイプで送る" checked={value.swipe} onChange={(swipe) => onChange({ swipe })} />
    <Toggle label="左右の向きを逆にする" help={value.reverse ? "左をタップ、右へスワイプで次へ進みます。" : "右をタップ、左へスワイプで次へ進みます。"} checked={value.reverse} onChange={(reverse) => onChange({ reverse })} />
    <Toggle label="音量キーで送る" help="読書中だけ、音量を下げるキーで次へ、上げるキーで前へ送ります。" checked={value.volumeKeys} onChange={(volumeKeys) => onChange({ volumeKeys })} />
    <Segmented label="送るときの動き" value={value.motion} options={[{ id: "slide", label: "なめらか" }, { id: "none", label: "すぐ切り替える" }]} onChange={(motion) => onChange({ motion })} />
    <Segmented label="上下のメニュー" value={value.bars} options={[{ id: "auto", label: "読むときは隠す" }, { id: "pinned", label: "常に出す" }]} onChange={(bars) => onChange({ bars })} />
    <Segmented label="ページボタン" value={value.pageButtons} options={[{ id: "always", label: "常に出す" }, { id: "menu", label: "メニューと一緒" }]} onChange={(pageButtons) => onChange({ pageButtons })} help="つなげて読むときは使いません。" />
  </div>;
}
