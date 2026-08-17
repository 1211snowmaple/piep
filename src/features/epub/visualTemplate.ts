/**
 * The bridge between the visual editor and the stylesheet a template ships.
 *
 * Everything the visual controls touch is written into one managed block at the
 * end of `style.css.j2`, as custom properties plus the rules that consume them.
 * Storing the values as real CSS - rather than beside the file - means a hand
 * edit inside the block is read back by the controls, and everything outside it
 * survives untouched.
 */

export interface VisualTemplateValues {
  fontFamily: "serif" | "sans";
  fontSize: number;
  lineHeight: number;
  /** Space between paragraphs, in em. */
  paragraphSpacing: number;
  /** First-line indent, in em. Japanese prose usually wants 1. */
  textIndent: number;
  pagePadding: number;
  justify: boolean;
  /** Vertical Japanese typesetting. Pairs with a right-to-left reading order. */
  verticalWriting: boolean;
  textColor: string;
  backgroundColor: string;
  accentColor: string;
  mutedColor: string;
  titleAlign: "left" | "center";
  titleSize: number;
  headingAlign: "left" | "center";
  headingSize: number;
  headingRule: boolean;
  coverWidth: number;
  coverRadius: number;
  illustrationWidth: number;
  rubySize: number;
}

export const visualTemplateDefaults: VisualTemplateValues = {
  fontFamily: "serif",
  fontSize: 16,
  lineHeight: 1.8,
  paragraphSpacing: 0.2,
  textIndent: 0,
  pagePadding: 16,
  justify: false,
  verticalWriting: false,
  textColor: "#202020",
  backgroundColor: "#ffffff",
  accentColor: "#0073bb",
  mutedColor: "#767676",
  titleAlign: "left",
  titleSize: 1.5,
  headingAlign: "left",
  headingSize: 1.2,
  headingRule: true,
  coverWidth: 60,
  coverRadius: 10,
  illustrationWidth: 100,
  rubySize: 0.5,
};

const START = "/* piep-visual:start */";
const END = "/* piep-visual:end */";

const SERIF = '"Yu Mincho", "Hiragino Mincho ProN", "Noto Serif JP", serif';
const SANS = '"Yu Gothic", "Hiragino Sans", "Noto Sans JP", sans-serif';

/** Reads the managed block back into control values, falling back per property. */
export function readVisualSettings(content: string): VisualTemplateValues {
  const block = content.match(/\/\* piep-visual:start \*\/([\s\S]*?)\/\* piep-visual:end \*\//)?.[1];
  if (!block) return { ...visualTemplateDefaults };
  const number = (property: string, fallback: number) => {
    const raw = block.match(new RegExp(`--piep-${property}:\\s*(-?[0-9.]+)`))?.[1];
    const parsed = raw === undefined ? Number.NaN : Number(raw);
    return Number.isFinite(parsed) ? parsed : fallback;
  };
  const color = (property: string, fallback: string) =>
    block.match(new RegExp(`--piep-${property}:\\s*(#[0-9a-fA-F]{3,8})`))?.[1] ?? fallback;
  const flag = (property: string, truthy: string, fallback: boolean) => {
    const raw = block.match(new RegExp(`--piep-${property}:\\s*([a-z-]+)`))?.[1];
    return raw === undefined ? fallback : raw === truthy;
  };
  return {
    fontFamily: /--piep-font-family:[^;]*(sans-serif|Gothic)/i.test(block) ? "sans" : "serif",
    fontSize: number("font-size", visualTemplateDefaults.fontSize),
    lineHeight: number("line-height", visualTemplateDefaults.lineHeight),
    paragraphSpacing: number("paragraph-spacing", visualTemplateDefaults.paragraphSpacing),
    textIndent: number("text-indent", visualTemplateDefaults.textIndent),
    pagePadding: number("page-padding", visualTemplateDefaults.pagePadding),
    justify: flag("text-align", "justify", visualTemplateDefaults.justify),
    verticalWriting: flag("writing-mode", "vertical-rl", visualTemplateDefaults.verticalWriting),
    textColor: color("text-color", visualTemplateDefaults.textColor),
    backgroundColor: color("background-color", visualTemplateDefaults.backgroundColor),
    accentColor: color("accent-color", visualTemplateDefaults.accentColor),
    mutedColor: color("muted-color", visualTemplateDefaults.mutedColor),
    titleAlign: flag("title-align", "center", false) ? "center" : "left",
    titleSize: number("title-size", visualTemplateDefaults.titleSize),
    headingAlign: flag("heading-align", "center", false) ? "center" : "left",
    headingSize: number("heading-size", visualTemplateDefaults.headingSize),
    headingRule: flag("heading-rule", "solid", visualTemplateDefaults.headingRule),
    coverWidth: number("cover-width", visualTemplateDefaults.coverWidth),
    coverRadius: number("cover-radius", visualTemplateDefaults.coverRadius),
    illustrationWidth: number("illustration-width", visualTemplateDefaults.illustrationWidth),
    rubySize: number("ruby-size", visualTemplateDefaults.rubySize),
  };
}

/** Replaces the managed block, leaving the rest of the template code alone. */
export function writeVisualSettings(content: string, values: VisualTemplateValues): string {
  const clean = content.replace(/\n?\/\* piep-visual:start \*\/[\s\S]*?\/\* piep-visual:end \*\/\n?/g, "\n").trimEnd();
  return `${clean}\n\n${START}\n${managedBlock(values)}${END}\n`;
}

function managedBlock(values: VisualTemplateValues): string {
  const font = values.fontFamily === "serif" ? SERIF : SANS;
  return `:root {
    --piep-font-family: ${font};
    --piep-font-size: ${values.fontSize}px;
    --piep-line-height: ${values.lineHeight};
    --piep-paragraph-spacing: ${values.paragraphSpacing}em;
    --piep-text-indent: ${values.textIndent}em;
    --piep-page-padding: ${values.pagePadding}px;
    --piep-text-align: ${values.justify ? "justify" : "start"};
    --piep-writing-mode: ${values.verticalWriting ? "vertical-rl" : "horizontal-tb"};
    --piep-text-color: ${values.textColor};
    --piep-background-color: ${values.backgroundColor};
    --piep-accent-color: ${values.accentColor};
    --piep-muted-color: ${values.mutedColor};
    --piep-title-align: ${values.titleAlign};
    --piep-title-size: ${values.titleSize}em;
    --piep-heading-align: ${values.headingAlign};
    --piep-heading-size: ${values.headingSize}em;
    --piep-heading-rule: ${values.headingRule ? "solid" : "none"};
    --piep-cover-width: ${values.coverWidth}%;
    --piep-cover-radius: ${values.coverRadius}px;
    --piep-illustration-width: ${values.illustrationWidth}%;
    --piep-ruby-size: ${values.rubySize}em;

    /* 基本スタイルが使う変数も、ここでの選択に合わせておく */
    --text-color: var(--piep-text-color);
    --muted-color: var(--piep-muted-color);
    --accent-color: var(--piep-accent-color);
    /* 旧名。自分で書き換えたテンプレートがまだ参照していることがある */
    --tag-color: var(--piep-accent-color);
    --cover-radius: var(--piep-cover-radius);
    --cover-width: var(--piep-cover-width);
}

html {
    writing-mode: var(--piep-writing-mode);
}

body {
    font-family: var(--piep-font-family);
    font-size: var(--piep-font-size);
    line-height: var(--piep-line-height);
    padding: var(--piep-page-padding);
    color: var(--piep-text-color);
    background-color: var(--piep-background-color);
    text-align: var(--piep-text-align);
}

p {
    margin-bottom: var(--piep-paragraph-spacing);
    text-indent: var(--piep-text-indent);
}

/* 原文の空行は字下げしない。空の段落が右にずれて見えてしまう */
p.blank-line {
    text-indent: 0;
    margin-bottom: 0;
}

h1.title {
    text-align: var(--piep-title-align);
    font-size: var(--piep-title-size);
}

h2 {
    text-align: var(--piep-heading-align);
    font-size: var(--piep-heading-size);
    border-bottom-style: var(--piep-heading-rule);
}

.cover-container img {
    max-width: var(--piep-cover-width);
    border-radius: var(--piep-cover-radius);
}

.illustration img {
    max-width: var(--piep-illustration-width);
}

ruby rt {
    font-size: var(--piep-ruby-size);
}
`;
}
