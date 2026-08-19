---
layout: home

hero:
  name: piep
  text: 手元に残して読む
  tagline: pixiv と FANBOX の小説・記事を、自分のPCに丸ごと残して読むためのデスクトップアプリ
  actions:
    - theme: brand
      text: 方針を読む
      link: /policy/01-what-piep-is
    - theme: alt
      text: IPC 契約
      link: /reference/ipc
    - theme: alt
      text: GitHub
      link: https://github.com/1211snowmaple/piep

features:
  - title: 方針書
    details: なぜこうなっているかを書いた、唯一の一次資料。人が書き、ソースの変更と同じコミットで更新する。日付は書かない。
    link: /policy/01-what-piep-is
    linkText: 読む
  - title: 境界の契約
    details: フロントと Rust をつなぐ 140 のコマンド、イベント、28 のテーブル。ソースから抽出し、両側の食い違いを CI で検査している。
    link: /reference/ipc
    linkText: 見る
  - title: API リファレンス
    details: src/ の JSDoc と src-tauri/ の doc コメントから生成。LLM は使わず、構文解析による決定論的な変換のみ。
    link: /frontend/index.html
    linkText: 見る
---

## この文書について

このサイトには二種類しかない。

| | 誰が書くか | 場所 |
|---|---|---|
| **方針書** | 人が書く。ここが唯一の一次資料 | `docs/policy/` |
| **リファレンス** | ソースから生成する。人は書かない | `docs/reference/` |

日付入りの調査レポートは置かない。方針書は「いつ調べたか」ではなく
**「今どうなっているか」を書く**現在形の文書で、更新はソースの変更と同じ
コミットで行う。調査の経緯は git 履歴とコミットメッセージに残す。

→ [ドキュメントの作り](/policy/10-documentation)

## 生成しなおす

```bash
npm --prefix docs-tools ci
npm --prefix docs-tools run docs:build
```
