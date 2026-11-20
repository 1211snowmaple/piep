-- 過去の版の本文の検索の索引（<棚>/derived/history/search-index/）に入れた文書の記録。
-- いまの版の索引（search_documents）と同じ作りで、文書の単位が作品でなく改稿になる。
-- 改稿は、いまの版の改稿でないもの。文書はその改稿の最後の保存版から作る（補完で増えた
-- 画像やシリーズの印も、その改稿の姿に含める）。利用者のタグは作品に付くので、どの改稿の
-- 文書にも入れる（「自分のタグ＋昔の本文の語」で引けるように）。
--
-- いまの版と別の索引にするのは、同じ索引に入れると語の珍しさ（関連度の重み）が変わり、
-- いまの版だけの検索の順位まで変わるため。
CREATE TABLE search_history_documents(
    doc_key INTEGER PRIMARY KEY AUTOINCREMENT,
    revision_id BLOB NOT NULL UNIQUE,
    work_id BLOB NOT NULL,
    snapshot_id BLOB,
    user_tags TEXT,
    recipe INTEGER NOT NULL DEFAULT 0
) STRICT;
CREATE INDEX search_history_work ON search_history_documents(work_id);

CREATE TABLE search_history_state(
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    format TEXT NOT NULL,
    generation BLOB NOT NULL
) STRICT;

-- 意味検索のベクトル（<棚>/derived/search/semantic-v2/index.sqlite）を作った文書の記録。
-- 作品ごとに一行で、状態の意味は search_documents と同じ。鍵は本文の検索の索引と別にする
-- （本文の検索の索引を作り直しても、時間のかかる埋め込みをやり直さないため）。
CREATE TABLE semantic_documents(
    doc_key INTEGER PRIMARY KEY AUTOINCREMENT,
    work_id BLOB NOT NULL UNIQUE,
    snapshot_id BLOB,
    edit_id BLOB,
    user_tags TEXT,
    recipe INTEGER NOT NULL DEFAULT 0
) STRICT;
