-- 本文の検索の索引（<棚>/derived/search-index/）に入れた文書の記録。索引は派生物で、
-- 消えても棚から作り直せる。索引の文書は整数の鍵しか持てないので、作品との対応をここで持つ。
--
-- 行は「索引に確定した文書が、どの保存状態から作られたか」を表す。保存版・有効な編集・
-- 利用者のタグ・作り方のどれかが今と違えば、その作品の文書は古い。鍵は先に割り当て、
-- 索引への確定のあとで状態を書く。途中で落ちても、状態が古いままなので次に作り直される。
-- 鍵は使い回さない（索引に消し損ねた文書が残っていても、別の作品として引かれない）。
CREATE TABLE search_documents(
    doc_key INTEGER PRIMARY KEY AUTOINCREMENT,
    work_id BLOB NOT NULL UNIQUE,
    snapshot_id BLOB,
    edit_id BLOB,
    user_tags TEXT,
    recipe INTEGER NOT NULL DEFAULT 0
) STRICT;

-- 索引そのものの記録。形式が変わったか、索引の場所が空になったら、表を空にして作り直す。
CREATE TABLE search_index_state(
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    format TEXT NOT NULL,
    generation BLOB NOT NULL
) STRICT;
