/**
 * What the studio shows when there is no desktop backend behind it.
 *
 * The browser preview build has no library and no template directory, so every
 * panel would otherwise be an error state and the screen could not be reviewed
 * or captured at all.
 */

import type { TemplateFile, TemplateFileKind, TemplateInfo, TemplatePreview, TemplateSettings } from "@/types/epub";

const baseSettings: TemplateSettings = {
  label: "標準",
  description: "どの取得元でも破綻しない、素直な組版。",
  appliesTo: [],
  language: "ja",
  pageProgression: "ltr",
  includeCoverPage: true,
  includeInfoPage: true,
  includeNcx: true,
  chapterToc: true,
  coverInReadingOrder: true,
  infoFields: [
    { key: "series", label: "シリーズ", enabled: true },
    { key: "title", label: "タイトル", enabled: true },
    { key: "author", label: "作者", enabled: true },
    { key: "cover", label: "表紙", enabled: true },
    { key: "textLength", label: "文字数", enabled: true },
    { key: "description", label: "紹介文", enabled: true },
    { key: "tags", label: "タグ", enabled: true },
    { key: "datePublished", label: "公開日", enabled: true },
    { key: "source", label: "配信元URL", enabled: true },
  ],
  strings: { TOC_TITLE: "目次", COVER_TITLE: "表紙", INFO_TITLE: "作品情報", BODY_MATTER_TITLE: "本文" },
};

export const demoTemplates: TemplateInfo[] = [
  { name: "default", isBuiltin: true, fileCount: 8, settings: baseSettings },
  { name: "pixiv", isBuiltin: true, fileCount: 8, settings: { ...baseSettings, label: "pixiv 小説", description: "シリーズ名と文字数を表紙まわりに置く体裁。", appliesTo: ["pixiv"] } },
  { name: "fanbox", isBuiltin: true, fileCount: 8, settings: { ...baseSettings, label: "FANBOX 投稿", description: "投稿日と支援プランを添える体裁。", appliesTo: ["fanbox"] } },
];

export const demoFiles: TemplateFile[] = [
  { filename: "style.css.j2", sizeBytes: 2480, customized: false },
  { filename: "_base_style.css.j2", sizeBytes: 3120, customized: false },
  { filename: "cover_page.xhtml.j2", sizeBytes: 880, customized: false },
  { filename: "info_page.xhtml.j2", sizeBytes: 2960, customized: false },
  { filename: "page_wrapper.xhtml.j2", sizeBytes: 620, customized: false },
  { filename: "nav.xhtml.j2", sizeBytes: 1480, customized: false },
  { filename: "toc.ncx.j2", sizeBytes: 1090, customized: false },
  { filename: "content.opf.j2", sizeBytes: 3340, customized: false },
];

export const demoFileKinds: TemplateFileKind[] = [
  { filename: "style.css.j2", purpose: "本全体の組版。テーマ固有の指定はここに書く", language: "css" },
  { filename: "_base_style.css.j2", purpose: "すべてのテンプレートが土台にする基本スタイル", language: "css" },
  { filename: "cover_page.xhtml.j2", purpose: "表紙のページ", language: "xml" },
  { filename: "info_page.xhtml.j2", purpose: "作品情報のページ。並べる項目は設定で決まる", language: "xml" },
  { filename: "page_wrapper.xhtml.j2", purpose: "本文ページの外枠", language: "xml" },
  { filename: "nav.xhtml.j2", purpose: "目次 (EPUB 3 のナビゲーション文書)", language: "xml" },
  { filename: "toc.ncx.j2", purpose: "EPUB 2 互換の目次。古い端末向け", language: "xml" },
  { filename: "content.opf.j2", purpose: "書誌情報とファイル一覧 (パッケージ文書)", language: "xml" },
];

export function demoFileContent(filename: string): string {
  if (filename.endsWith(".css.j2")) {
    return '{% include "_base_style.css.j2" %}\n\n/* テーマ固有のスタイル */\n';
  }
  return `<?xml version="1.0" encoding="utf-8"?>\n<!-- ${filename} — デスクトップアプリで実際の中身を編集できます -->\n`;
}

const demoInfoPage = `<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xml:lang="ja" lang="ja">
<head><meta charset="utf-8" /><title>見本の作品</title></head>
<body class="info-page">
  <section class="main-content">
    <p class="series">見本のシリーズ 第2話</p>
    <h1 class="title">見本の作品 ― 雨と硝子</h1>
    <p class="author">作者：見本 作者</p>
    <p class="text-length"><span class="badge">1,234文字</span></p>
    <div class="description"><p>テンプレートの見え方を確かめるための紹介文です。</p></div>
    <p class="tags"><span class="tag">#見本</span> <span class="tag">#テンプレート</span></p>
    <p class="date">公開日：2024年4月1日</p>
  </section>
</body>
</html>`;

const demoBodyPage = `<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xml:lang="ja" lang="ja">
<head><meta charset="utf-8" /><title>第一章 はじまり</title></head>
<body>
  <section class="chapter">
    <h2>第一章 はじまり</h2>
    <p>雨の匂いがした。<ruby>硝子<rp>(</rp><rt>ガラス</rt><rp>)</rp></ruby>越しの街は、いつもより遠く見える。</p>
    <p class="blank-line"><br /></p>
    <p>「行こうか」と、彼女は言った。</p>
  </section>
</body>
</html>`;

const demoNav = `<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xml:lang="ja" lang="ja">
<head><meta charset="utf-8" /><title>目次</title></head>
<body>
  <nav><h1>目次</h1><ol><li><a href="#">作品情報</a></li><li><a href="#">第一章 はじまり</a></li></ol></nav>
</body>
</html>`;

export const demoPreview: TemplatePreview = {
  sampleTitle: "見本の作品 ― 雨と硝子",
  sampleSource: "sample",
  sampleDownloadId: null,
  css: "body { line-height: 1.8; padding: 16px; font-family: serif; } h1.title { font-size: 1.5em; } .tag { color: #0073bb; } .badge { background: rgba(0,0,0,.06); border-radius: 20px; padding: 2px 10px; font-size: .8em; } .series, .date { color: #767676; font-size: .85em; }",
  cover: null,
  info: demoInfoPage,
  page: demoBodyPage,
  nav: demoNav,
  opf: "<package />",
  ncx: null,
  issues: [],
  fields: [
    { path: "core.name", group: "core", label: "作品タイトル", sample: "見本の作品 ― 雨と硝子", available: true },
    { path: "core.author.name", group: "core", label: "作者名", sample: "見本 作者", available: true },
    { path: "core.isPartOf.name", group: "core", label: "シリーズ名", sample: "見本のシリーズ", available: true },
    { path: "stats.textLength", group: "stats", label: "文字数", sample: "1234", available: true },
    { path: "stats.likeCount", group: "stats", label: "いいね / ブックマーク", sample: "321", available: true },
    { path: "provider.source", group: "provider", label: "取得元 (pixiv / fanbox)", sample: "sample", available: true },
    { path: "content.pages", group: "content", label: "本文ページ（1件）", sample: "1件", available: true },
  ],
};
