ALTER TABLE library_meta ADD COLUMN watch_generation INTEGER NOT NULL DEFAULT 0 CHECK(watch_generation>=0);

-- 監視グループを架空の作品にしない。監視の変更は棚のカタログに属する。
CREATE TABLE watch_outbox_sequence(value INTEGER NOT NULL);
INSERT INTO watch_outbox_sequence SELECT seq FROM sqlite_sequence WHERE name='outbox';
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
INSERT INTO outbox_next(sequence,request_id,entity_id,operation,payload_json,created_at,saved_search_id,user_id)
SELECT sequence,request_id,entity_id,operation,payload_json,created_at,saved_search_id,user_id FROM outbox;
DROP TABLE outbox;
ALTER TABLE outbox_next RENAME TO outbox;
UPDATE sqlite_sequence SET seq=max(seq,coalesce((SELECT max(value) FROM watch_outbox_sequence),0)) WHERE name='outbox';
INSERT INTO sqlite_sequence(name,seq)
SELECT 'outbox',max(value) FROM watch_outbox_sequence HAVING count(*)>0 AND NOT EXISTS(SELECT 1 FROM sqlite_sequence WHERE name='outbox');
DROP TABLE watch_outbox_sequence;
