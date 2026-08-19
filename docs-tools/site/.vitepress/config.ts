// piep のドキュメントサイト。
//
// 手書きの方針書 (policy/) と、ソースから生成したリファレンス (reference/) を
// 一つのサイトへ束ねる。Rust の rustdoc だけは HTML なので public/backend/ に
// 置き、VitePress からは外部リンクとして参照する。
//
// `defineConfig` を取り込んでいないのは、VitePress を docs-tools/ に隔離して
// いるためである。この設定ファイルは docs/ にあり、Node は docs/ から上へ
// node_modules を探すので docs-tools/node_modules には届かない。素の
// オブジェクトを返せば読み込みは成功し、VitePress 本体は docs-tools 側から
// 動くので何も損なわれない。
export default {
  title: "piep",
  description: "pixiv と FANBOX の作品を手元に残して読むためのデスクトップアプリ",
  lang: "ja-JP",
  // GitHub Pages のプロジェクトサイト (https://1211snowmaple.github.io/piep/)。
  base: "/piep/",
  cleanUrls: true,
  // 生成物は docs:build を通さないと存在しない。方針書だけを直したいときに
  // リンク切れで止まらないようにする。実際の検査は contract:check が行う。
  ignoreDeadLinks: true,

  themeConfig: {
    search: { provider: "local" },
    nav: [
      { text: "方針", link: "/policy/01-what-piep-is" },
      { text: "リファレンス", link: "/reference/ipc" },
      { text: "リリース", link: "https://github.com/1211snowmaple/piep/releases" },
    ],
    sidebar: [
      {
        text: "方針",
        items: [
          { text: "piepとは何か", link: "/policy/01-what-piep-is" },
          { text: "設計原則", link: "/policy/02-principles" },
          { text: "アーキテクチャ", link: "/policy/03-architecture" },
          { text: "データの持ち方", link: "/policy/04-data" },
          { text: "画面の方針", link: "/policy/05-frontend" },
          { text: "Rust側の方針", link: "/policy/06-backend" },
          { text: "取得の方針", link: "/policy/07-acquisition" },
          { text: "EPUBの方針", link: "/policy/08-epub" },
          { text: "品質の担保", link: "/policy/09-quality" },
          { text: "ドキュメントの作り", link: "/policy/10-documentation" },
        ],
      },
      {
        text: "リファレンス（自動生成）",
        items: [
          { text: "IPC コマンド契約", link: "/reference/ipc" },
          { text: "イベント契約", link: "/reference/events" },
          { text: "スキーマ", link: "/reference/schema" },
          { text: "フロント API", link: "/frontend/index.html", target: "_blank" },
          { text: "Rust API", link: "/backend/piep_lib/index.html", target: "_blank" },
        ],
      },
    ],
    socialLinks: [{ icon: "github", link: "https://github.com/1211snowmaple/piep" }],
    outline: { level: [2, 3], label: "このページの内容" },
    docFooter: { prev: "前へ", next: "次へ" },
    darkModeSwitchLabel: "外観",
    returnToTopLabel: "先頭へ",
    lastUpdated: { text: "最終更新" },
  },

  markdown: {
    // 生成した Markdown には型の表記として `{` が現れる。Vue の展開と解釈
    // されないよう、コードとして扱われない波括弧をそのまま出す。
    config: (md) => {
      md.options.html = true;
    },
  },

  vite: {
    // 生成物が無い状態でも起動できるようにする。
    server: { fs: { allow: [".."] } },
  },
};
