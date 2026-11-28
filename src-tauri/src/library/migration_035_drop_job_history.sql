-- 旧棚から移した更新履歴と、その一覧の表示状態を取り除く。
-- 外部キーが参照する親の表は最後に落とす。
DROP TABLE job_history_visibility;
DROP TABLE job_history_logs;
DROP TABLE job_history_items;
DROP TABLE job_history;

-- 他の操作の控えと採番位置を引き継ぐ。削除した控えの番号も再利用しない。
CREATE TEMP TABLE retained_outbox_sequence(value INTEGER NOT NULL);
INSERT INTO retained_outbox_sequence SELECT seq FROM sqlite_sequence WHERE name='outbox';
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
    FOREIGN KEY(saved_search_id,user_id) REFERENCES saved_searches(id,user_id),
    CHECK((entity_id IS NOT NULL AND saved_search_id IS NULL AND user_id IS NULL AND watch_catalog_id IS NULL AND operation NOT IN ('saved_search','watch'))
       OR (entity_id IS NULL AND saved_search_id IS NOT NULL AND user_id IS NOT NULL AND watch_catalog_id IS NULL AND operation='saved_search')
       OR (entity_id IS NULL AND saved_search_id IS NULL AND user_id IS NULL AND watch_catalog_id IS NOT NULL AND watch_catalog_id=1 AND operation='watch'))
) STRICT;
INSERT INTO outbox_next(sequence,request_id,entity_id,operation,payload_json,created_at,saved_search_id,user_id,watch_catalog_id)
SELECT sequence,request_id,entity_id,operation,payload_json,created_at,saved_search_id,user_id,watch_catalog_id FROM outbox WHERE history_catalog_id IS NULL;
DROP TABLE outbox;
ALTER TABLE outbox_next RENAME TO outbox;
UPDATE sqlite_sequence SET seq=max(seq,coalesce((SELECT max(value) FROM retained_outbox_sequence),0)) WHERE name='outbox';
INSERT INTO sqlite_sequence(name,seq)
SELECT 'outbox',max(value) FROM retained_outbox_sequence HAVING count(*)>0 AND NOT EXISTS(SELECT 1 FROM sqlite_sequence WHERE name='outbox');
DROP TABLE retained_outbox_sequence;
