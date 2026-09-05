// piep のドキュメントサイト。
//
// 読む人で三つに分ける。使う人（guide/）、次に手を入れる人（policy/）、境界を
// 触る人（reference/）。以前は「人が書いたもの／機械が生成したもの」で分けて
// いたが、その分類には使う人の居場所が無く、使い方の説明がサイトの外（README）
// へ出てしまっていた。
//
// Rust の rustdoc だけは HTML なので public/backend/ に置き、VitePress からは
// 外部リンクとして参照する。TypeDoc は読む人がいなかったので外した。
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
      { text: "使う", link: "/guide/01-what-it-does" },
      { text: "つくり", link: "/policy/01-what-piep-is" },
      { text: "契約", link: "/reference/ipc" },
      { text: "リリース", link: "https://github.com/1211snowmaple/piep/releases" },
    ],
    sidebar: [
      {
        text: "使う",
        items: [
          { text: "piep でできること", link: "/guide/01-what-it-does" },
          { text: "はじめる", link: "/guide/02-getting-started" },
          { text: "画面ごとの説明", link: "/guide/03-screens" },
          { text: "データの置き場所とバックアップ", link: "/guide/04-your-data" },
        ],
      },
      {
        text: "つくり（方針）",
        collapsed: false,
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
          { text: "AI補助の方針", link: "/policy/11-assist" },
        ],
      },
      {
        text: "契約（自動生成）",
        items: [
          { text: "IPC コマンド契約", link: "/reference/ipc" },
          { text: "イベント契約", link: "/reference/events" },
          { text: "スキーマ", link: "/reference/schema" },
          { text: "Rust API (rustdoc)", link: "/backend/piep_lib/index.html", target: "_blank" },
        ],
      },
      {
        // 未実装の設計。実装したら消し、残るものは方針書へ移す。
        text: "これから（未実装）",
        collapsed: true,
        items: [{ text: "まとまりの発見", link: "/plan/collection-discovery" }],
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
