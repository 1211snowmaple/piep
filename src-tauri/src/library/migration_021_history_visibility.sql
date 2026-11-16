-- 完了履歴の原本は変えず、一覧での表示だけを利用者の状態として持つ。
CREATE TABLE job_history_visibility (
    job_id BLOB PRIMARY KEY REFERENCES job_history(id),
    hidden INTEGER NOT NULL CHECK(hidden IN (0,1)),
    updated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE history_outbox_sequence(value INTEGER NOT NULL);
INSERT INTO history_outbox_sequence SELECT seq FROM sqlite_sequence WHERE name='outbox';
CREATE TABLE outbox_next (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    request_id BLOB NOT NULL UNIQUE CHECK(length(request_id)=16),
    entity_id BLOB REFERENCES entities(id),
    operation TEXT NOT NULL,
    payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
    created_at INTEGER NOT NULL,
    saved_search_id BLOB,
    user_id BLOB,
    watch_catalog_id INTEGER REFERENCES library_meta(singleton),
    history_catalog_id INTEGER REFERENCES library_meta(singleton),
    FOREIGN KEY(saved_search_id,user_id) REFERENCES saved_searches(id,user_id),
    CHECK((entity_id IS NOT NULL AND saved_search_id IS NULL AND user_id IS NULL AND watch_catalog_id IS NULL AND history_catalog_id IS NULL AND operation NOT IN ('saved_search','watch','job_history_visibility'))
       OR (entity_id IS NULL AND saved_search_id IS NOT NULL AND user_id IS NOT NULL AND watch_catalog_id IS NULL AND history_catalog_id IS NULL AND operation='saved_search')
       OR (entity_id IS NULL AND saved_search_id IS NULL AND user_id IS NULL AND watch_catalog_id IS NOT NULL AND watch_catalog_id=1 AND history_catalog_id IS NULL AND operation='watch')
       OR (entity_id IS NULL AND saved_search_id IS NULL AND user_id IS NULL AND watch_catalog_id IS NULL AND history_catalog_id IS NOT NULL AND history_catalog_id=1 AND operation='job_history_visibility'))
) STRICT;
INSERT INTO outbox_next(sequence,request_id,entity_id,operation,payload_json,created_at,saved_search_id,user_id,watch_catalog_id)
SELECT sequence,request_id,entity_id,operation,payload_json,created_at,saved_search_id,user_id,watch_catalog_id FROM outbox;
DROP TABLE outbox;
ALTER TABLE outbox_next RENAME TO outbox;
UPDATE sqlite_sequence SET seq=max(seq,coalesce((SELECT max(value) FROM history_outbox_sequence),0)) WHERE name='outbox';
INSERT INTO sqlite_sequence(name,seq)
SELECT 'outbox',max(value) FROM history_outbox_sequence HAVING count(*)>0 AND NOT EXISTS(SELECT 1 FROM sqlite_sequence WHERE name='outbox');
DROP TABLE history_outbox_sequence;
