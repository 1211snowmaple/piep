-- 一覧は派生物。保存状態・編集・利用者の状態から作り直せる値だけを持ち、並べ替えの鍵ごとに索引を張る。
-- 本文の文字数は本文ファイルだけで決まる事実なので、ファイルごとに一度だけ数えて残す。
CREATE TABLE body_text_stats (
    file_id BLOB PRIMARY KEY REFERENCES content_files(id),
    text_length INTEGER NOT NULL CHECK(text_length >= 0)
) STRICT, WITHOUT ROWID;
INSERT OR IGNORE INTO body_text_stats(file_id,text_length)
SELECT s.body_file_id,p.text_length
FROM work_current_projection p JOIN revision_snapshots s ON s.id=p.snapshot_id
WHERE s.body_file_id IS NOT NULL;
-- 取得元が一覧や更新確認で知らせる最終更新日。原本に載らない取得元がある。
ALTER TABLE source_entity_state ADD COLUMN source_updated_at INTEGER;
DROP INDEX work_projection_recent;
DROP INDEX work_projection_title;
DROP TABLE work_current_projection;
-- 空の一覧は、書き込みで開いたときに作り直す（recipe が古い行も同じ）。
CREATE TABLE work_current_projection (
    work_id BLOB PRIMARY KEY REFERENCES library_items(work_id),
    snapshot_id BLOB NOT NULL,
    edit_id BLOB,
    provider TEXT NOT NULL,
    external_key TEXT NOT NULL,
    content_type TEXT NOT NULL,
    title TEXT NOT NULL,
    author_name TEXT NOT NULL,
    saved_at INTEGER NOT NULL,
    published_at INTEGER,
    updated_at INTEGER,
    text_length INTEGER NOT NULL CHECK(text_length >= 0),
    logical_size INTEGER NOT NULL CHECK(logical_size >= 0),
    media_count INTEGER NOT NULL CHECK(media_count >= 0),
    version_number INTEGER NOT NULL CHECK(version_number > 0),
    favorite INTEGER NOT NULL CHECK(favorite IN (0,1)),
    recipe INTEGER NOT NULL CHECK(recipe > 0),
    FOREIGN KEY(work_id,snapshot_id) REFERENCES entity_current(entity_id,snapshot_id) DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;
CREATE INDEX work_list_saved ON work_current_projection(saved_at,work_id);
CREATE INDEX work_list_published ON work_current_projection(coalesce(published_at,saved_at),work_id);
CREATE INDEX work_list_updated ON work_current_projection(coalesce(updated_at,published_at,saved_at),work_id);
CREATE INDEX work_list_title ON work_current_projection(title COLLATE NOCASE,work_id);
CREATE INDEX work_list_author ON work_current_projection(author_name COLLATE NOCASE,work_id);
CREATE INDEX work_list_length ON work_current_projection(text_length,work_id);
CREATE INDEX work_list_size ON work_current_projection(logical_size,work_id);
CREATE INDEX work_list_media ON work_current_projection(media_count,work_id);
CREATE INDEX work_list_version ON work_current_projection(version_number,work_id);
