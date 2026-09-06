# 続き物を見つける比較実験

公式シリーズに整理されていない続き物、シリーズ内の小さな前後編、pixiv/FANBOXを
またぐ続き物を調べるための道具。本番アルゴリズムの差し替えはしない。
採用方針は [まとまりの発見](../../docs/plan/collection-discovery.md) に置く。

## 再現

Python標準ライブラリと、リポジトリで固定したRust環境を使う。
実データの名前・本文・ID一覧・DBの写しは `scratch/` にのみ保存する。

アプリ起動中のDBは、DBファイルとWALを別々にコピーせずSQLite backup APIで写す。
PowerShellで次を実行する。既存の写しを置き換えないよう、出力先は新しい名前にする。

```powershell
@'
import os, sqlite3
from pathlib import Path
source = Path(os.environ['APPDATA']) / 'com.hiron.piep/piep.db'
target = Path('scratch/sequence-research/library.snapshot.db')
target.parent.mkdir(parents=True, exist_ok=True)
if target.exists():
    raise SystemExit('Snapshot already exists; choose a new filename.')
with sqlite3.connect(source.as_uri() + '?mode=ro', uri=True) as src:
    with sqlite3.connect(target) as dst:
        src.backup(dst)
'@ | python -X utf8 -

cargo build --manifest-path src-tauri/Cargo.toml --example collection_research_export
python -X utf8 tools/sequence-research/research.py --db scratch/sequence-research/library.snapshot.db --exporter src-tauri/target/debug/examples/collection_research_export.exe
python -X utf8 tools/sequence-research/link_probe.py --db scratch/sequence-research/library.snapshot.db --output scratch/sequence-research/link-results.json
python -X utf8 tools/sequence-research/edition_probe.py --db scratch/sequence-research/library.snapshot.db --pairs scratch/sequence-research/private-title-pairs.json --output scratch/sequence-research/edition-results.json
```

exporterは `SQLITE_OPEN_READ_ONLY` を指定し、`Database::open` や移行処理を呼ばない。
題名実験もSQLiteを読み取り専用で開き、実験の前後で写しのSHA-256を確認する。
通信や外部モデル推論は行わない。

## 題名の比較

- `baseline_title`: 現行Rustの正規化・話数・版・告知判定をexportし、作者＋語幹先頭26文字・9文字下限・2〜40作の題名族を再現する。
- `structured_exact`: 題名本体と話数を分け、短題・漢数字・丸数字・先頭連番・一部の副題を扱う。順序の異なる題名を候補にする。
- `broad_ngram`: 文字3-gramのDice類似度だけで候補にする。緩和の副作用を測る対照。
- `structured_ngram`: 話数整合と意味のある括弧を確認してから近似一致を使う。
- `structured_v2`: 実データで見つかった注記の誤読と「章→前後編→番号」を修正した第二案。既知の配布注記と字数注記を本題から分離する。
- `structured_v2_ngram`: 第二案の解析結果にも近似一致を加える比較。別作品への誤った橋が増えないか別に確認する。
- `union_retrieval`: 現行の題名候補と第二案の和集合。これは候補検索の入口であり、続編判定ではない。

近似版の探索範囲は、取得元内の作者IDと、異なる取得元にある作者表示名の正規化一致。
後者は本人確認済みを意味しない。表示名も異なる作者は本文リンクや確認済み別名から
探す必要があり、この題名実験だけでは網羅できない。

出力 `title-results.json` は集計、`private-title-pairs.json` は実データの確認用候補。
前者も自動でコミットしない。DBの写しと実験入力の版を合わせて比較する。

## 結果を読むとき

比較実験でいう「現行」は調査開始時の旧実装を指す。抽選や題名経路だけの比較と、
構造化題名・本文検査・安定した候補選抜を入れた実装全体の結果を混ぜない。
現在の走査を検証する手順は末尾の「実装の走査を実データで確認する」に分けている。

`fixtures.json` の44組は、作成者が関係を設定した匿名の合成例である。
実データから無作為に採った正解集でも、独立した固定テストでもない。
同じ題名だけでは判定不能な例も意図的に含む。正例24・負例20の混同行列は
規則の弱点を探すために使い、実ライブラリの適合率や再現率として報告しない。
`sources` と `series` は場面の注記で、この小さなペア試験では分類入力ではない。
`regressions.json` の4組は実データで見つかった失敗を匿名化して作った追加の開発用例。
第二案はこれらの失敗を見て修正しているので、独立テストに通ったという意味ではない。

実データ比較では現行の告知除外と版の代表選択を共通に適用する。その段階で消える
短文記事や告知語入り小説、別版の誤認は、この比較では回復できない。
題名経路だけの再現であり、全走査のリンク経路、合本処理、重複統合、却下、強さの閾値、
8件選択とUI表示は含まない。ペア数を連載数や正解数へ読み替えない。

本文リンク由来の `continues_*` は既存分類器の推測で、誤りもある。
`weak_link_coverage` はその参考集合に題名経路が届いた割合であり、真の再現率ではない。
公式シリーズの内側も管理名あり／それ以外で分けるが、普通の名前のシリーズも
複数物語を含み得る。サービス跨ぎはこれらと重なる別軸で数える。

実行時間にはdebugビルドの全ルールexportと複数手法の比較が含まれる。
`export_seconds` と `library_comparison_seconds` を分けて読み、本番走査の性能値として使わない。
類似度しきい値は比較用の仮置きで、採用値は作者単位で分けた別の正解集から決める。

## リンクと版の比較

`link_probe.py` は全関係、参照除外、続編のみの無向グラフを比較する。
告知・短文ハブ除外を現行相当に再現するが、版の畳み込み等を含む本番全体ではない。
同じ公式シリーズ内の真部分成分とサービス跨ぎの辺を別集計する。
合成のURL周辺文14ラベルで、現行相当の広い文脈、競合時棄権、URL局所文脈を比較する。
局所版はプレーンテキストの試作で、HTMLアンカー解析を実装したものではない。

`edition_probe.py` は第二案が新しく拾った跨サービスのペアだけを調べる。
保存済み本文から64文字の断片を最大約300個ずつ取り、相手の本文に含まれる割合を
両方向で測る。一方向だけ高い場合は抜粋・合本の確認対象にする。0.8は実験上の仮置き。
本文ファイルはDBの写しに含まれないため実行時に読み、ハッシュをローカル出力に残す。
未知の本文JSON形式・改稿・未保存部分を扱いきれず、一般的な版判定器ではない。


## 実装の走査を実データで確認する

比較実験とは別に `collection_sweep_probe` が本番の走査を呼び出す。これは候補を書き込むため、
上で作ったスナップショットをさらに別名へSQLite backupし、検証用DBだけを渡す。
`storage_dir` にも検証用の空ディレクトリを指定する。DB内の本文パスは元の保存本文を読み、
コレクションや候補の書き込みは検証用DB内に留まる。起動復旧ジャーナルが空であることを確認する。
意味索引をコピーしなければテーマ検証は行えず、その理由が結果の `note` に出る。

```powershell
$env:PROBE_JSON = "$PWD/scratch/sequence-research/implementation-results.json"
cargo run --manifest-path src-tauri/Cargo.toml --example collection_sweep_probe -- scratch/sequence-research/implementation-probe.db scratch/sequence-research/probe-storage
```

結果には私有の題名・作者・根拠が入るため `scratch/` に保管する。再走査のIDと順序の安定性、
未登録作・シリーズ内・跨サービスの件数、本文重複を除いた候補、矛盾を残した候補を点検する。
上限200件の結果数を精度として報告しない。匿名の回帰例はRustの `discovery_*` と
`work_link_evidence`、画面の契約は `SuggestionInbox.test.tsx` と `CollectionSweepModal.test.tsx` で固定する。
