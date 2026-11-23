-- 棚を開いたときに一度だけ行う手入れ。SQL だけではできない（本文のファイルを読む）ものを、
-- 開いた棚が実体を読めるようになってから行う（revision_folds.rs）。終えたら行を消す。
-- ・fold_volatile_revisions: 本文の比較規則 2（埋め込みカードのいいね数などを除く）で前の版と
--   同じになる最後の改稿を畳む。作者の書いたものは変わっていないのに分かれた版。
CREATE TABLE library_tasks (
    name TEXT PRIMARY KEY CHECK(name IN ('fold_volatile_revisions')),
    created_at INTEGER NOT NULL
) STRICT;
INSERT INTO library_tasks(name,created_at)
VALUES('fold_volatile_revisions', CAST(unixepoch('subsec')*1000 AS INTEGER));
