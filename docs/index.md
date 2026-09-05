---
layout: home

hero:
  name: piep
  text: 手元に残して読む
  tagline: pixiv と FANBOX の小説・記事を、自分のPCに丸ごと残して読むためのデスクトップアプリ
  image:
    src: /piep-icon.svg
    alt: piep
  actions:
    - theme: brand
      text: 使いはじめる
      link: /guide/01-what-it-does
    - theme: alt
      text: 画面を見る
      link: /guide/03-screens
    - theme: alt
      text: GitHub
      link: https://github.com/1211snowmaple/piep

features:
  - title: 使う
    icon: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M3 5.5A1.5 1.5 0 0 1 4.5 4H9a3 3 0 0 1 3 3v13a2.5 2.5 0 0 0-2.5-2.5H3z"/><path d="M21 5.5A1.5 1.5 0 0 0 19.5 4H15a3 3 0 0 0-3 3v13a2.5 2.5 0 0 1 2.5-2.5H21z"/></svg>'
    details: できること、インストール、画面ごとの説明、データの置き場所とバックアップ。piep を入れた人が読むところ。
    link: /guide/01-what-it-does
    linkText: 読む
  - title: つくり
    icon: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="m12 3 9 5-9 5-9-5 9-5Z"/><path d="m3 12 9 5 9-5"/><path d="m3 16.5 9 5 9-5"/></svg>'
    details: なぜこの形なのかを書いた方針書。人が書き、ソースの変更と同じコミットで更新する。日付は書かない。
    link: /policy/01-what-piep-is
    linkText: 読む
  - title: 契約
    icon: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M10 13.5a4.5 4.5 0 0 0 6.8.5l2.7-2.7a4.5 4.5 0 0 0-6.4-6.3l-1.5 1.5"/><path d="M14 10.5a4.5 4.5 0 0 0-6.8-.5L4.5 12.7a4.5 4.5 0 0 0 6.4 6.3l1.5-1.5"/></svg>'
    details: フロントと Rust をつなぐコマンド、イベント、テーブル。ソースから抽出し、両側の食い違いを CI で検査している。
    link: /reference/ipc
    linkText: 見る
---

## この文書について

**読む人で三つに分かれている。**

| | 誰のためか | 誰が書くか | 場所 |
|---|---|---|---|
| **使う** | piep を入れた人 | 人が書く | `docs/guide/` |
| **つくり** | 次に手を入れる人 | 人が書く。ここが唯一の一次資料 | `docs/policy/` |
| **契約** | 境界を触る人 | ソースから生成する。人は書かない | `docs/reference/` |

この三つ以外は置かない。まだ実装していない設計は `docs/plan/` に置き、実装した
時点で消す。残るものは方針書へ移す。

日付入りの調査レポートは置かない。方針書は「いつ調べたか」ではなく
「今どうなっているか」を書く現在形の文書で、更新はソースの変更と同じコミットで
行う。調査の経緯は git 履歴とコミットメッセージに残す。

→ [ドキュメントの作り](/policy/10-documentation)

## 今のところの大きさ

| | |
|---|---:|
| 画面 | <!--stat:screens.count-->9<!--/stat--> |
| IPC コマンド | <!--stat:commands.total-->159<!--/stat--> |
| うち説明のあるもの | <!--stat:commands.described-->61<!--/stat--> |
| イベント | <!--stat:events.total-->13<!--/stat--> |
| テーブル | <!--stat:tables.total-->30<!--/stat--> |

この表の数は手で書いていない。抽出器が数え、`docs:check` が古くなっていないかを
検査している。説明は Rust の `///`、モジュールの `//!`、呼び出す TS 関数の JSDoc
から拾う。**頁ではなくコードに書けば、そのまま出る。**

## 生成しなおす

```bash
npm --prefix docs-tools ci
npm --prefix docs-tools run docs:build
```
